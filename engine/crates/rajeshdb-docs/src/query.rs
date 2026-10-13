//! Filters, queries, and the (very small) query planner.
//!
//! Filter language (MongoDB-like subset):
//!   {"city": "Delhi"}                         equality (implicit $eq)
//!   {"age": {"$gt": 18, "$lte": 60}}          $eq $ne $gt $gte $lt $lte $in $nin $exists
//!   {"$or": [ {...}, {...} ]}, {"$and": [...]} top-level boolean combinators
//!   {"address.city": "Pune"}                  dotted paths into nested objects
//! All top-level entries are ANDed. Comparisons only match values of the same kind
//! (number vs number, string vs string, bool vs bool).

use crate::error::{invalid, Result};
use crate::keys::{validate_field_path, MAX_INDEXED_STRING};
use serde_json::{Map, Value};
use std::cmp::Ordering;

pub fn get_path<'a>(doc: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = doc;
    for seg in path.split('.') {
        cur = cur.as_object()?.get(seg)?;
    }
    Some(cur)
}

pub fn compare_values(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64()?.partial_cmp(&y.as_f64()?),
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(_), Value::Number(_)) => compare_values(a, b) == Some(Ordering::Equal),
        _ => a == b,
    }
}

fn is_operator_object(m: &Map<String, Value>) -> bool {
    !m.is_empty() && m.keys().all(|k| k.starts_with('$'))
}

/// Limits that keep a single query from burning unbounded CPU (a filter is evaluated against every document).
pub const MAX_FILTER_DEPTH: usize = 8;
pub const MAX_FILTER_NODES: usize = 500;
pub const MAX_LIST_LEN: usize = 1000;

pub fn validate_filter(f: &Value) -> Result<()> {
    let mut budget = MAX_FILTER_NODES;
    validate_at(f, 0, &mut budget)
}

fn validate_at(f: &Value, depth: usize, budget: &mut usize) -> Result<()> {
    if depth > MAX_FILTER_DEPTH {
        return Err(invalid(format!("filter nesting is too deep (maximum {})", MAX_FILTER_DEPTH)));
    }
    let obj = f.as_object().ok_or_else(|| invalid("filter must be a JSON object"))?;
    for (k, v) in obj {
        if *budget == 0 {
            return Err(invalid(format!("filter is too complex (maximum {} conditions)", MAX_FILTER_NODES)));
        }
        *budget -= 1;
        if k.starts_with('$') {
            match k.as_str() {
                "$and" | "$or" => {
                    let arr = v
                        .as_array()
                        .filter(|a| !a.is_empty())
                        .ok_or_else(|| invalid(format!("{} needs a non-empty array of filters", k)))?;
                    for sub in arr {
                        validate_at(sub, depth + 1, budget)?;
                    }
                }
                _ => return Err(invalid(format!("unknown top-level operator {}", k))),
            }
            continue;
        }
        if k.is_empty() {
            return Err(invalid("empty field name in filter"));
        }
        if let Some(m) = v.as_object() {
            if m.keys().any(|x| x.starts_with('$')) {
                if !is_operator_object(m) {
                    return Err(invalid(format!("field '{}' mixes operators and plain keys", k)));
                }
                for (op, arg) in m {
                    let ok = match op.as_str() {
                        "$eq" | "$ne" => true,
                        "$gt" | "$gte" | "$lt" | "$lte" => {
                            matches!(arg, Value::Number(_) | Value::String(_) | Value::Bool(_))
                        }
                        "$in" | "$nin" => match arg.as_array() {
                            Some(a) if a.len() > MAX_LIST_LEN => {
                                return Err(invalid(format!("{} lists may hold at most {} values", op, MAX_LIST_LEN)))
                            }
                            Some(_) => true,
                            None => false,
                        },
                        "$exists" => arg.is_boolean(),
                        _ => return Err(invalid(format!("unknown operator {}", op))),
                    };
                    if !ok {
                        return Err(invalid(format!("bad argument for {} on field '{}'", op, k)));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Assumes `validate_filter` already succeeded.
pub fn doc_matches(doc: &Value, f: &Value) -> bool {
    let obj = match f.as_object() {
        Some(o) => o,
        None => return false,
    };
    obj.iter().all(|(k, v)| match k.as_str() {
        "$and" => v.as_array().map_or(false, |a| a.iter().all(|s| doc_matches(doc, s))),
        "$or" => v.as_array().map_or(false, |a| a.iter().any(|s| doc_matches(doc, s))),
        _ => {
            let field = get_path(doc, k);
            match v.as_object() {
                Some(m) if is_operator_object(m) => m.iter().all(|(op, arg)| apply_op(field, op, arg)),
                _ => field.map_or(false, |x| values_equal(x, v)),
            }
        }
    })
}

fn apply_op(field: Option<&Value>, op: &str, arg: &Value) -> bool {
    let in_list = |x: &Value| arg.as_array().map_or(false, |a| a.iter().any(|y| values_equal(x, y)));
    match op {
        "$exists" => arg.as_bool().map_or(false, |b| b == field.is_some()),
        "$eq" => field.map_or(false, |x| values_equal(x, arg)),
        "$ne" => field.map_or(true, |x| !values_equal(x, arg)),
        "$in" => field.map_or(false, in_list),
        "$nin" => field.map_or(true, |x| !in_list(x)),
        "$gt" | "$gte" | "$lt" | "$lte" => match field.and_then(|x| compare_values(x, arg)) {
            Some(o) => match op {
                "$gt" => o == Ordering::Greater,
                "$gte" => o != Ordering::Less,
                "$lt" => o == Ordering::Less,
                _ => o != Ordering::Greater,
            },
            None => false,
        },
        _ => false,
    }
}

fn type_rank(v: Option<&Value>) -> u8 {
    match v {
        None => 0,
        Some(Value::Null) => 1,
        Some(Value::Bool(_)) => 2,
        Some(Value::Number(_)) => 3,
        Some(Value::String(_)) => 4,
        Some(Value::Array(_)) => 5,
        Some(Value::Object(_)) => 6,
    }
}

/// Total order used by sort: missing < null < bool < number < string < array < object.
pub fn sort_cmp(a: Option<&Value>, b: Option<&Value>) -> Ordering {
    let (ra, rb) = (type_rank(a), type_rank(b));
    if ra != rb {
        return ra.cmp(&rb);
    }
    match (a, b) {
        (Some(x), Some(y)) => compare_values(x, y).unwrap_or(Ordering::Equal),
        _ => Ordering::Equal,
    }
}

#[derive(Debug, Clone)]
pub struct Query {
    pub filter: Value,
    /// (field path, ascending?)
    pub sort: Option<(String, bool)>,
    pub skip: usize,
    pub limit: Option<usize>,
}

impl Query {
    pub fn new(filter: Value) -> Query {
        Query { filter, sort: None, skip: 0, limit: None }
    }

    /// {"filter": {...}, "sort": "age" | {"field": "age", "order": "desc"}, "skip": 0, "limit": 10}
    pub fn from_json(v: &Value) -> Result<Query> {
        let obj = v.as_object().ok_or_else(|| invalid("query must be a JSON object"))?;
        let mut q = Query::new(Value::Object(Map::new()));
        for (k, val) in obj {
            match k.as_str() {
                "filter" => {
                    if !val.is_object() {
                        return Err(invalid("filter must be an object"));
                    }
                    q.filter = val.clone();
                }
                "sort" => {
                    q.sort = Some(match val {
                        Value::String(s) => (s.clone(), true),
                        Value::Object(o) => {
                            let field = o
                                .get("field")
                                .and_then(|f| f.as_str())
                                .ok_or_else(|| invalid("sort.field must be a string"))?
                                .to_string();
                            let asc = match o.get("order").and_then(|x| x.as_str()) {
                                None | Some("asc") => true,
                                Some("desc") => false,
                                Some(_) => return Err(invalid("sort.order must be 'asc' or 'desc'")),
                            };
                            (field, asc)
                        }
                        _ => return Err(invalid("sort must be a string or an object")),
                    });
                }
                "skip" => q.skip = val.as_u64().ok_or_else(|| invalid("skip must be a non-negative integer"))? as usize,
                "limit" => {
                    q.limit = Some(val.as_u64().ok_or_else(|| invalid("limit must be a non-negative integer"))? as usize)
                }
                other => return Err(invalid(format!("unknown query key '{}'", other))),
            }
        }
        if let Some((f, _)) = &q.sort {
            validate_field_path(f)?;
        }
        Ok(q)
    }
}

#[derive(Debug, Clone)]
pub enum Plan {
    Full,
    IndexEq { field: String, value: Value },
    IndexIn { field: String, values: Vec<Value> },
    /// Each bound is (value, inclusive).
    IndexRange { field: String, lo: Option<(Value, bool)>, hi: Option<(Value, bool)> },
}

impl Plan {
    pub fn describe(&self) -> String {
        match self {
            Plan::Full => "FullScan".to_string(),
            Plan::IndexEq { field, .. } => format!("IndexScan(field={}, op=eq)", field),
            Plan::IndexIn { field, .. } => format!("IndexScan(field={}, op=in)", field),
            Plan::IndexRange { field, .. } => format!("IndexScan(field={}, op=range)", field),
        }
    }
}

fn indexable(v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::String(s) => s.len() <= MAX_INDEXED_STRING,
        _ => false,
    }
}

fn candidate(field: &str, v: &Value) -> Option<(u8, Plan)> {
    match v.as_object() {
        Some(m) if is_operator_object(m) => {
            if let Some(x) = m.get("$eq") {
                if indexable(x) {
                    return Some((0, Plan::IndexEq { field: field.to_string(), value: x.clone() }));
                }
            }
            if let Some(Value::Array(a)) = m.get("$in") {
                if a.iter().all(indexable) {
                    return Some((1, Plan::IndexIn { field: field.to_string(), values: a.clone() }));
                }
            }
            let mut lo: Option<(Value, bool)> = None;
            let mut hi: Option<(Value, bool)> = None;
            for (op, arg) in m {
                if !matches!(arg, Value::Number(_) | Value::String(_)) || !indexable(arg) {
                    continue;
                }
                match op.as_str() {
                    "$gt" if lo.is_none() => lo = Some((arg.clone(), false)),
                    "$gte" if lo.is_none() => lo = Some((arg.clone(), true)),
                    "$lt" if hi.is_none() => hi = Some((arg.clone(), false)),
                    "$lte" if hi.is_none() => hi = Some((arg.clone(), true)),
                    _ => {}
                }
            }
            if lo.is_some() || hi.is_some() {
                Some((2, Plan::IndexRange { field: field.to_string(), lo, hi }))
            } else {
                None
            }
        }
        Some(_) => None,
        None => {
            if indexable(v) {
                Some((0, Plan::IndexEq { field: field.to_string(), value: v.clone() }))
            } else {
                None
            }
        }
    }
}

/// Pick the cheapest usable index among the top-level (ANDed) conditions: eq < in < range.
/// The executor ALWAYS re-checks the full filter on every candidate document, so an index
/// only has to return a superset of the true answer.
pub fn choose_plan(filter: &Value, indexed: &[String]) -> Plan {
    let obj = match filter.as_object() {
        Some(o) => o,
        None => return Plan::Full,
    };
    let mut best: Option<(u8, Plan)> = None;
    for (k, v) in obj {
        if k.starts_with('$') || !indexed.iter().any(|i| i == k) {
            continue;
        }
        if let Some((rank, plan)) = candidate(k, v) {
            if best.as_ref().map_or(true, |(r, _)| rank < *r) {
                best = Some((rank, plan));
            }
        }
    }
    best.map(|(_, p)| p).unwrap_or(Plan::Full)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matching_basics() {
        let d = json!({"a": 1, "b": {"c": "x"}, "n": null});
        assert!(doc_matches(&d, &json!({"a": 1.0})));
        assert!(doc_matches(&d, &json!({"b.c": "x"})));
        assert!(doc_matches(&d, &json!({"a": {"$gte": 1, "$lt": 2}})));
        assert!(!doc_matches(&d, &json!({"a": {"$gt": 1}})));
        assert!(doc_matches(&d, &json!({"zzz": {"$exists": false}})));
        assert!(doc_matches(&d, &json!({"zzz": {"$ne": 5}})));
        assert!(!doc_matches(&d, &json!({"a": {"$gt": "0"}}))); // number vs string never matches
        assert!(doc_matches(&d, &json!({"n": null})));
        assert!(!doc_matches(&d, &json!({"zzz": null}))); // missing != null
        assert!(doc_matches(&d, &json!({"$or": [{"a": 2}, {"b.c": "x"}]})));
    }

    #[test]
    fn validation_errors() {
        assert!(validate_filter(&json!({"$nope": 1})).is_err());
        assert!(validate_filter(&json!({"a": {"$wat": 1}})).is_err());
        assert!(validate_filter(&json!({"a": {"$in": 1}})).is_err());
        assert!(validate_filter(&json!({"a": {"$gt": 1, "b": 2}})).is_err());
        assert!(validate_filter(&json!({"$or": []})).is_err());
        assert!(validate_filter(&json!([1])).is_err());
        assert!(validate_filter(&json!({})).is_ok());
    }

    #[test]
    fn planner_prefers_eq_and_ignores_unindexed() {
        let idx = vec!["city".to_string(), "age".to_string()];
        let p = choose_plan(&json!({"age": {"$gt": 5}, "city": "x"}), &idx);
        assert_eq!(p.describe(), "IndexScan(field=city, op=eq)");
        let p = choose_plan(&json!({"age": {"$gt": 5}}), &idx);
        assert_eq!(p.describe(), "IndexScan(field=age, op=range)");
        assert_eq!(choose_plan(&json!({"name": "x"}), &idx).describe(), "FullScan");
        assert_eq!(choose_plan(&json!({"$or": [{"city": "x"}]}), &idx).describe(), "FullScan");
        assert_eq!(choose_plan(&json!({"city": {"$ne": "x"}}), &idx).describe(), "FullScan");
    }

    #[test]
    fn expensive_filters_are_rejected() {
        // deep nesting
        let mut f = json!({"a": 1});
        for _ in 0..(MAX_FILTER_DEPTH + 2) {
            f = json!({"$and": [f]});
        }
        assert!(validate_filter(&f).is_err());
        // moderate nesting is fine
        let mut ok = json!({"a": 1});
        for _ in 0..3 {
            ok = json!({"$or": [ok, {"b": 2}]});
        }
        assert!(validate_filter(&ok).is_ok());
        // huge $in list
        let big: Vec<i64> = (0..(MAX_LIST_LEN as i64 + 1)).collect();
        assert!(validate_filter(&json!({"a": {"$in": big}})).is_err());
        let fine: Vec<i64> = (0..(MAX_LIST_LEN as i64)).collect();
        assert!(validate_filter(&json!({"a": {"$in": fine}})).is_ok());
        // many conditions in total (even if each level is small): 10 x 100 > 500
        let clause = json!({"$or": (0..100).map(|i| json!({"f": i})).collect::<Vec<_>>()});
        let many = json!({"$and": (0..10).map(|_| clause.clone()).collect::<Vec<_>>()});
        assert!(validate_filter(&many).is_err());
    }
}
