use rajeshdb_docs::{DocDb, DocError, Options, Query, Value};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("rajeshdb-docs-test-{}-{}-{}", std::process::id(), n, nanos));
        fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn opts() -> Options {
    Options { memtable_limit_bytes: 64 * 1024, sync_writes: false }
}

fn setup() -> (TempDir, DocDb) {
    let t = TempDir::new();
    let db = DocDb::open(t.path(), opts()).unwrap();
    (t, db)
}

fn ids(docs: &[Value]) -> Vec<String> {
    docs.iter().map(|d| d["_id"].as_str().unwrap().to_string()).collect()
}

fn find(db: &DocDb, coll: &str, filter: Value) -> Vec<String> {
    ids(&db.find(coll, &Query::new(filter)).unwrap())
}

fn seed(db: &mut DocDb, coll: &str) {
    db.create_collection(coll, None).unwrap();
    let people = [
        ("1", "Asha", "Delhi", 30),
        ("2", "Ben", "Mumbai", 25),
        ("3", "Chen", "Delhi", 41),
        ("4", "Dev", "Pune", 19),
        ("5", "Esha", "Mumbai", -3),
    ];
    for (id, name, city, age) in people {
        db.insert(coll, json!({"_id": id, "name": name, "city": city, "age": age, "addr": {"zip": id}})).unwrap();
    }
    db.insert(coll, json!({"_id": "6", "name": "Fay", "tags": ["x"]})).unwrap();
}

#[test]
fn crud_roundtrip() {
    let (_t, mut db) = setup();
    db.create_collection("users", None).unwrap();
    let id = db.insert("users", json!({"name": "A"})).unwrap();
    assert!(!id.is_empty());
    let doc = db.get("users", &id).unwrap().unwrap();
    assert_eq!(doc["name"], "A");
    assert_eq!(doc["_id"], id.as_str());

    db.replace("users", &id, json!({"name": "B", "x": 1})).unwrap();
    assert_eq!(db.get("users", &id).unwrap().unwrap()["name"], "B");

    assert!(db.delete("users", &id).unwrap());
    assert!(!db.delete("users", &id).unwrap());
    assert!(db.get("users", &id).unwrap().is_none());
}

#[test]
fn duplicate_id_and_bad_documents_rejected() {
    let (_t, mut db) = setup();
    db.create_collection("c", None).unwrap();
    db.insert("c", json!({"_id": "a"})).unwrap();
    assert!(matches!(db.insert("c", json!({"_id": "a"})), Err(DocError::AlreadyExists(_))));
    assert!(matches!(db.insert("c", json!([1, 2])), Err(DocError::InvalidArgument(_))));
    assert!(matches!(db.insert("c", json!({"_id": 5})), Err(DocError::InvalidArgument(_))));
    assert!(matches!(db.replace("c", "zzz", json!({})), Err(DocError::NotFound(_))));
    assert!(matches!(db.replace("c", "a", json!({"_id": "other"})), Err(DocError::InvalidArgument(_))));
}

#[test]
fn collection_management_and_names() {
    let (_t, mut db) = setup();
    assert!(matches!(db.create_collection("bad/name", None), Err(DocError::InvalidArgument(_))));
    assert!(matches!(db.create_collection("", None), Err(DocError::InvalidArgument(_))));
    db.create_collection("a", None).unwrap();
    db.create_collection("b", None).unwrap();
    assert!(matches!(db.create_collection("a", None), Err(DocError::AlreadyExists(_))));
    assert_eq!(db.list_collections().unwrap(), vec!["a".to_string(), "b".to_string()]);
    assert!(matches!(db.insert("nope", json!({})), Err(DocError::NotFound(_))));
}

#[test]
fn filters_without_index() {
    let (_t, mut db) = setup();
    seed(&mut db, "p");
    assert_eq!(find(&db, "p", json!({})), ["1", "2", "3", "4", "5", "6"]);
    assert_eq!(find(&db, "p", json!({"city": "Delhi"})), ["1", "3"]);
    assert_eq!(find(&db, "p", json!({"age": {"$gt": 25}})), ["1", "3"]);
    assert_eq!(find(&db, "p", json!({"age": {"$gte": 19, "$lt": 30}})), ["2", "4"]);
    assert_eq!(find(&db, "p", json!({"age": {"$lt": 0}})), ["5"]);
    assert_eq!(find(&db, "p", json!({"city": {"$in": ["Pune", "Mumbai"]}})), ["2", "4", "5"]);
    assert_eq!(find(&db, "p", json!({"city": {"$nin": ["Pune", "Mumbai"]}})), ["1", "3", "6"]);
    assert_eq!(find(&db, "p", json!({"city": {"$ne": "Delhi"}})), ["2", "4", "5", "6"]);
    assert_eq!(find(&db, "p", json!({"age": {"$exists": false}})), ["6"]);
    assert_eq!(find(&db, "p", json!({"addr.zip": "3"})), ["3"]);
    assert_eq!(find(&db, "p", json!({"$or": [{"city": "Pune"}, {"age": {"$lt": 0}}]})), ["4", "5"]);
    assert_eq!(find(&db, "p", json!({"$and": [{"city": "Delhi"}, {"age": {"$gt": 35}}]})), ["3"]);
    assert_eq!(find(&db, "p", json!({"city": "Delhi", "age": {"$gt": 35}})), ["3"]);
    assert_eq!(db.count("p", &json!({"city": "Mumbai"})).unwrap(), 2);
}

#[test]
fn index_results_equal_full_scan_results() {
    let (_t, mut db) = setup();
    seed(&mut db, "plain");
    seed(&mut db, "indexed_after");
    db.create_index("indexed_after", "city").unwrap();
    db.create_index("indexed_after", "age").unwrap();
    db.create_collection("indexed_first", None).unwrap();
    db.create_index("indexed_first", "city").unwrap();
    db.create_index("indexed_first", "age").unwrap();
    for (id, name, city, age) in [
        ("1", "Asha", "Delhi", 30),
        ("2", "Ben", "Mumbai", 25),
        ("3", "Chen", "Delhi", 41),
        ("4", "Dev", "Pune", 19),
        ("5", "Esha", "Mumbai", -3),
    ] {
        db.insert("indexed_first", json!({"_id": id, "name": name, "city": city, "age": age})).unwrap();
    }
    db.insert("indexed_first", json!({"_id": "6", "name": "Fay", "tags": ["x"]})).unwrap();

    let filters = vec![
        json!({"city": "Delhi"}),
        json!({"city": "Nowhere"}),
        json!({"city": {"$in": ["Pune", "Mumbai"]}}),
        json!({"age": {"$gt": 25}}),
        json!({"age": {"$gte": 19, "$lt": 30}}),
        json!({"age": {"$lt": 0}}),
        json!({"age": {"$lte": 25}}),
        json!({"age": 30}),
        json!({"age": {"$gt": 100}}),
        json!({"city": "Delhi", "age": {"$gt": 35}}),
        json!({"age": {"$gt": "a"}}),
        json!({"city": {"$gte": "Mumbai"}}),
        json!({"city": {"$gt": "Delhi", "$lt": "Pune"}}),
    ];
    for f in filters {
        let expect = find(&db, "plain", f.clone());
        assert_eq!(find(&db, "indexed_after", f.clone()), expect, "indexed_after, filter {}", f);
        let first = find(&db, "indexed_first", f.clone());
        assert_eq!(first, expect, "indexed_first, filter {}", f);
    }
}

#[test]
fn explain_shows_planner_choice() {
    let (_t, mut db) = setup();
    seed(&mut db, "p");
    db.create_index("p", "city").unwrap();
    db.create_index("p", "age").unwrap();
    let q = |f: Value| Query::new(f);
    assert_eq!(db.explain("p", &q(json!({"city": "Delhi"}))).unwrap(), "IndexScan(field=city, op=eq)");
    assert_eq!(db.explain("p", &q(json!({"age": {"$gt": 3}}))).unwrap(), "IndexScan(field=age, op=range)");
    assert_eq!(
        db.explain("p", &q(json!({"city": {"$in": ["a"]}, "age": {"$gt": 3}}))).unwrap(),
        "IndexScan(field=city, op=in)"
    );
    assert_eq!(db.explain("p", &q(json!({"name": "Asha"}))).unwrap(), "FullScan");
    assert_eq!(db.explain("p", &q(json!({"$or": [{"city": "x"}]}))).unwrap(), "FullScan");
    db.drop_index("p", "city").unwrap();
    assert_eq!(db.explain("p", &q(json!({"city": "Delhi"}))).unwrap(), "FullScan");
    assert_eq!(find(&db, "p", json!({"city": "Delhi"})), ["1", "3"]);
    assert_eq!(db.list_indexes("p").unwrap(), vec!["age".to_string()]);
}

#[test]
fn indexes_follow_updates_and_deletes() {
    let (_t, mut db) = setup();
    seed(&mut db, "p");
    db.create_index("p", "city").unwrap();
    assert_eq!(db.update_where("p", &json!({"city": "Delhi"}), &json!({"city": "Goa"})).unwrap(), 2);
    assert_eq!(find(&db, "p", json!({"city": "Delhi"})), Vec::<String>::new());
    assert_eq!(find(&db, "p", json!({"city": "Goa"})), ["1", "3"]);
    assert!(db.delete("p", "1").unwrap());
    assert_eq!(find(&db, "p", json!({"city": "Goa"})), ["3"]);
    db.replace("p", "3", json!({"name": "Chen", "city": "Pune"})).unwrap();
    assert_eq!(find(&db, "p", json!({"city": "Goa"})), Vec::<String>::new());
    assert_eq!(find(&db, "p", json!({"city": "Pune"})), ["3", "4"]);
    assert_eq!(db.delete_where("p", &json!({"city": "Pune"})).unwrap(), 2);
    assert_eq!(find(&db, "p", json!({"city": "Pune"})), Vec::<String>::new());
    // Field removed from document -> entry removed.
    db.replace("p", "2", json!({"name": "Ben"})).unwrap();
    assert_eq!(find(&db, "p", json!({"city": "Mumbai"})), ["5"]);
}

#[test]
fn everything_survives_reopen_including_indexes() {
    let t = TempDir::new();
    {
        let mut db = DocDb::open(t.path(), opts()).unwrap();
        seed(&mut db, "p");
        db.create_index("p", "city").unwrap();
        db.update_where("p", &json!({"city": "Delhi"}), &json!({"age": 99})).unwrap();
    }
    let mut db = DocDb::open(t.path(), opts()).unwrap();
    assert_eq!(db.list_indexes("p").unwrap(), vec!["city".to_string()]);
    assert_eq!(db.explain("p", &Query::new(json!({"city": "Delhi"}))).unwrap(), "IndexScan(field=city, op=eq)");
    assert_eq!(find(&db, "p", json!({"city": "Delhi"})), ["1", "3"]);
    assert_eq!(find(&db, "p", json!({"age": 99})), ["1", "3"]);
    db.flush().unwrap();
    db.compact().unwrap();
    assert_eq!(find(&db, "p", json!({"city": "Delhi"})), ["1", "3"]);
}

#[test]
fn sort_skip_limit() {
    let (_t, mut db) = setup();
    seed(&mut db, "p");
    let q = Query::from_json(&json!({"sort": {"field": "age", "order": "desc"}, "skip": 1, "limit": 2})).unwrap();
    assert_eq!(ids(&db.find("p", &q).unwrap()), ["1", "2"]); // order: 3,1,2,4,5,6(no age)
    let q = Query::from_json(&json!({"sort": "age"})).unwrap();
    assert_eq!(ids(&db.find("p", &q).unwrap()), ["6", "5", "4", "2", "1", "3"]);
    let q = Query::from_json(&json!({"filter": {"city": "Mumbai"}, "sort": "name", "limit": 1})).unwrap();
    assert_eq!(ids(&db.find("p", &q).unwrap()), ["2"]);
    assert!(Query::from_json(&json!({"bogus": 1})).is_err());
    assert!(Query::from_json(&json!({"limit": -1})).is_err());
    assert!(Query::from_json(&json!({"sort": {"field": "a", "order": "sideways"}})).is_err());
}

#[test]
fn numeric_and_string_range_ordering_in_index() {
    let (_t, mut db) = setup();
    db.create_collection("n", None).unwrap();
    db.create_index("n", "x").unwrap();
    let vals = [("a", -10.5), ("b", -1.0), ("c", 0.0), ("d", 0.5), ("e", 2.0), ("f", 100.0)];
    for (id, v) in vals {
        db.insert("n", json!({"_id": id, "x": v})).unwrap();
    }
    db.insert("n", json!({"_id": "s", "x": "text"})).unwrap();
    assert_eq!(find(&db, "n", json!({"x": {"$gt": -1}})), ["c", "d", "e", "f"]);
    assert_eq!(find(&db, "n", json!({"x": {"$lte": 0.5, "$gt": -10.5}})), ["b", "c", "d"]);
    assert_eq!(find(&db, "n", json!({"x": {"$gte": 0}})), ["c", "d", "e", "f"]);
    assert_eq!(find(&db, "n", json!({"x": 0})), ["c"]);
    assert_eq!(find(&db, "n", json!({"x": {"$gt": "a"}})), ["s"]);

    db.create_collection("s", None).unwrap();
    db.create_index("s", "w").unwrap();
    for (id, w) in [("1", "apple"), ("2", "banana"), ("3", "blueberry"), ("4", "cherry")] {
        db.insert("s", json!({"_id": id, "w": w})).unwrap();
    }
    assert_eq!(find(&db, "s", json!({"w": {"$gte": "b", "$lt": "c"}})), ["2", "3"]);
    assert_eq!(find(&db, "s", json!({"w": "banana"})), ["2"]);
}

#[test]
fn schema_is_enforced() {
    let (_t, mut db) = setup();
    let schema = json!({
        "fields": {"name": {"type": "string", "required": true}, "age": {"type": "integer"}},
        "additionalProperties": false
    });
    db.create_collection("s", Some(schema)).unwrap();
    db.insert("s", json!({"_id": "ok", "name": "A", "age": 3})).unwrap();
    for bad in [json!({"age": 3}), json!({"name": "A", "age": "x"}), json!({"name": "A", "age": 1.5}), json!({"name": "A", "extra": 1})] {
        assert!(matches!(db.insert("s", bad), Err(DocError::SchemaViolation(_))));
    }
    assert!(matches!(
        db.update_where("s", &json!({"_id": "ok"}), &json!({"age": "old"})),
        Err(DocError::SchemaViolation(_))
    ));
    assert_eq!(db.get("s", "ok").unwrap().unwrap()["age"], 3);
    assert!(matches!(
        db.create_collection("t", Some(json!({"fields": {"a": {"type": "banana"}}}))),
        Err(DocError::InvalidArgument(_))
    ));
}

#[test]
fn invalid_queries_are_errors_not_panics() {
    let (_t, mut db) = setup();
    seed(&mut db, "p");
    for f in [json!({"$bogus": 1}), json!({"a": {"$foo": 1}}), json!({"a": {"$in": 5}}), json!([1]), json!({"$or": []}), json!({"a": {"$gt": [1]}})] {
        assert!(matches!(db.find("p", &Query::new(f)), Err(DocError::InvalidArgument(_))));
    }
    assert!(matches!(db.find("missing", &Query::new(json!({}))), Err(DocError::NotFound(_))));
    assert!(matches!(db.create_index("p", "a/b"), Err(DocError::InvalidArgument(_))));
    assert!(matches!(db.create_index("p", "a..b"), Err(DocError::InvalidArgument(_))));
    assert!(matches!(db.create_index("p", "x"), Ok(())));
    assert!(matches!(db.create_index("p", "x"), Err(DocError::AlreadyExists(_))));
    assert!(matches!(db.drop_index("p", "nope"), Err(DocError::NotFound(_))));
}

#[test]
fn too_long_indexed_string_is_rejected() {
    let (_t, mut db) = setup();
    db.create_collection("c", None).unwrap();
    db.insert("c", json!({"_id": "pre", "s": "x".repeat(5000)})).unwrap();
    // Backfill refuses, because the existing document cannot be indexed.
    assert!(db.create_index("c", "s").is_err());
    assert!(db.list_indexes("c").unwrap().is_empty());
    db.delete("c", "pre").unwrap();
    db.create_index("c", "s").unwrap();
    assert!(matches!(db.insert("c", json!({"s": "y".repeat(2000)})), Err(DocError::InvalidArgument(_))));
    assert!(db.insert("c", json!({"s": "y".repeat(1000)})).is_ok());
}

#[test]
fn drop_collection_removes_everything() {
    let (_t, mut db) = setup();
    seed(&mut db, "p");
    db.create_index("p", "city").unwrap();
    db.drop_collection("p").unwrap();
    assert!(matches!(db.find("p", &Query::new(json!({}))), Err(DocError::NotFound(_))));
    assert!(db.list_collections().unwrap().is_empty());
    db.create_collection("p", None).unwrap();
    assert!(find(&db, "p", json!({})).is_empty());
    assert!(db.list_indexes("p").unwrap().is_empty());
    db.create_index("p", "city").unwrap();
    assert!(find(&db, "p", json!({"city": "Delhi"})).is_empty());
}

#[test]
fn collections_are_isolated_from_each_other() {
    let (_t, mut db) = setup();
    db.create_collection("a", None).unwrap();
    db.create_collection("ab", None).unwrap();
    db.create_index("a", "k").unwrap();
    db.create_index("ab", "k").unwrap();
    db.insert("a", json!({"_id": "1", "k": "v"})).unwrap();
    db.insert("ab", json!({"_id": "2", "k": "v"})).unwrap();
    assert_eq!(find(&db, "a", json!({"k": "v"})), ["1"]);
    assert_eq!(find(&db, "ab", json!({"k": "v"})), ["2"]);
    db.drop_collection("a").unwrap();
    assert_eq!(find(&db, "ab", json!({"k": "v"})), ["2"]);
}

/// Random operations mirrored against a simple in-memory model; the index must always agree.
#[test]
fn randomized_index_consistency() {
    let (_t, mut db) = setup();
    db.create_collection("r", None).unwrap();
    db.create_index("r", "n").unwrap();
    let mut model: BTreeMap<String, i64> = BTreeMap::new();
    let mut seed: u64 = 0x2545F4914F6CDD1D;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for step in 0..600 {
        let id = format!("id{:02}", next() % 40);
        let n = (next() % 10) as i64;
        match next() % 4 {
            0 | 1 => {
                if model.contains_key(&id) {
                    db.replace("r", &id, json!({"n": n})).unwrap();
                } else {
                    db.insert("r", json!({"_id": id.clone(), "n": n})).unwrap();
                }
                model.insert(id, n);
            }
            2 => {
                let existed = model.remove(&id).is_some();
                assert_eq!(db.delete("r", &id).unwrap(), existed);
            }
            _ => {
                let touched = db.update_where("r", &json!({"n": n}), &json!({"n": (n + 1) % 10})).unwrap();
                let expect = model.values().filter(|v| **v == n).count() as u64;
                assert_eq!(touched, expect);
                for v in model.values_mut() {
                    if *v == n {
                        *v = (n + 1) % 10;
                    }
                }
            }
        }
        if step % 100 == 99 {
            db.flush().unwrap();
        }
    }
    for v in 0..10i64 {
        let expect: Vec<String> = model.iter().filter(|(_, x)| **x == v).map(|(k, _)| k.clone()).collect();
        assert_eq!(find(&db, "r", json!({"n": v})), expect, "n == {}", v);
    }
    let expect: Vec<String> = model.iter().filter(|(_, x)| **x >= 3 && **x < 7).map(|(k, _)| k.clone()).collect();
    assert_eq!(find(&db, "r", json!({"n": {"$gte": 3, "$lt": 7}})), expect);
    assert_eq!(db.count("r", &json!({})).unwrap(), model.len());
}
