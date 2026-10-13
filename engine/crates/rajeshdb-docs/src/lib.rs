//! RajeshDB document layer (Phase 3): collections of JSON documents on top of the
//! key/value engine, with optional schemas, secondary indexes and a small query engine.
//!
//! Every logical write (document + its index entries) is ONE atomic engine batch, so a crash
//! can never leave an index disagreeing with its documents.

mod error;
mod keys;
mod query;
mod schema;

pub use error::{DocError, Result};
pub use query::{Plan, Query};
pub use rajeshdb_core::{Options, Stats};
pub use serde_json::Value;

use error::{corrupt, invalid};
use keys::*;
use query::{choose_plan, doc_matches, get_path, sort_cmp, validate_filter};
use rajeshdb_core::{BatchOp, Db};
use schema::{validate_doc, validate_schema_def};
use serde_json::Map;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

struct Meta {
    indexes: Vec<String>,
    schema: Option<Value>,
}

fn meta_to_bytes(m: &Meta) -> Vec<u8> {
    let mut o = Map::new();
    o.insert("indexes".into(), Value::Array(m.indexes.iter().map(|s| Value::String(s.clone())).collect()));
    if let Some(s) = &m.schema {
        o.insert("schema".into(), s.clone());
    }
    serde_json::to_vec(&Value::Object(o)).unwrap_or_default()
}

fn meta_from_bytes(b: &[u8]) -> Option<Meta> {
    let v: Value = serde_json::from_slice(b).ok()?;
    let o = v.as_object()?;
    let indexes = o
        .get("indexes")?
        .as_array()?
        .iter()
        .map(|x| x.as_str().map(String::from))
        .collect::<Option<Vec<String>>>()?;
    Some(Meta { indexes, schema: o.get("schema").cloned() })
}

fn parse_doc(bytes: &[u8]) -> Result<Value> {
    serde_json::from_slice(bytes).map_err(|e| corrupt(format!("stored document is not valid JSON: {}", e)))
}

fn id_of(doc: &Value) -> &str {
    doc.get("_id").and_then(|v| v.as_str()).unwrap_or("")
}

fn index_entries(coll: &str, indexes: &[String], id: &str, doc: Option<&Value>) -> Result<BTreeSet<Vec<u8>>> {
    let mut set = BTreeSet::new();
    if let Some(d) = doc {
        for field in indexes {
            if let Some(v) = get_path(d, field) {
                if let Some(enc) = encode_scalar(v)? {
                    set.insert(index_key(coll, field, &enc, id));
                }
            }
        }
    }
    Ok(set)
}

/// Build the engine operations for changing one document from `old` to `new`
/// (None = absent). Index entries are diffed so unchanged ones are not rewritten.
fn write_ops(
    coll: &str,
    indexes: &[String],
    id: &str,
    old: Option<&Value>,
    new: Option<&Value>,
) -> Result<Vec<BatchOp>> {
    let old_e = index_entries(coll, indexes, id, old)?;
    let new_e = index_entries(coll, indexes, id, new)?;
    let mut ops: Vec<BatchOp> = Vec::new();
    match new {
        Some(d) => ops.push((doc_key(coll, id), Some(serde_json::to_vec(d)?))),
        None => ops.push((doc_key(coll, id), None)),
    }
    for k in old_e.difference(&new_e) {
        ops.push((k.clone(), None));
    }
    for k in new_e.difference(&old_e) {
        ops.push((k.clone(), Some(Vec::new())));
    }
    Ok(ops)
}

pub struct DocDb {
    db: Db,
    counter: u64,
}

impl DocDb {
    pub fn open(dir: impl AsRef<Path>, opts: Options) -> Result<DocDb> {
        Ok(DocDb { db: Db::open(dir, opts)?, counter: 0 })
    }

    fn load_meta(&self, coll: &str) -> Result<Meta> {
        validate_collection_name(coll)?;
        let raw = self
            .db
            .get(&catalog_key(coll))?
            .ok_or_else(|| DocError::NotFound(format!("collection '{}' does not exist", coll)))?;
        meta_from_bytes(&raw).ok_or_else(|| corrupt(format!("metadata of collection '{}' is damaged", coll)))
    }

    /// Write many operations in several atomic batches (bounded by count and bytes).
    fn write_chunked(&mut self, ops: Vec<BatchOp>) -> Result<()> {
        let mut chunk: Vec<BatchOp> = Vec::new();
        let mut bytes = 0usize;
        for op in ops {
            bytes += op.0.len() + op.1.as_ref().map_or(0, |v| v.len()) + 16;
            chunk.push(op);
            if chunk.len() >= 1000 || bytes >= 8 * 1024 * 1024 {
                self.db.write_batch(&chunk)?;
                chunk.clear();
                bytes = 0;
            }
        }
        if !chunk.is_empty() {
            self.db.write_batch(&chunk)?;
        }
        Ok(())
    }

    fn delete_prefix(&mut self, prefix: &[u8]) -> Result<()> {
        let ops: Vec<BatchOp> = self.db.scan_prefix(prefix)?.into_iter().map(|(k, _)| (k, None)).collect();
        self.write_chunked(ops)
    }

    // ---------- collections ----------

    pub fn create_collection(&mut self, name: &str, schema: Option<Value>) -> Result<()> {
        validate_collection_name(name)?;
        if let Some(s) = &schema {
            validate_schema_def(s)?;
        }
        if self.db.get(&catalog_key(name))?.is_some() {
            return Err(DocError::AlreadyExists(format!("collection '{}' already exists", name)));
        }
        // Remove leftovers of a previously interrupted drop_collection.
        self.delete_prefix(&doc_prefix(name))?;
        self.delete_prefix(&index_coll_prefix(name))?;
        let meta = Meta { indexes: vec![], schema };
        self.db.write_batch(&[(catalog_key(name), Some(meta_to_bytes(&meta)))])?;
        Ok(())
    }

    pub fn drop_collection(&mut self, name: &str) -> Result<()> {
        self.load_meta(name)?;
        // Catalog entry goes first: once it is gone the collection is gone, even if we crash
        // while cleaning up (create_collection purges leftovers).
        self.db.write_batch(&[(catalog_key(name), None)])?;
        self.delete_prefix(&doc_prefix(name))?;
        self.delete_prefix(&index_coll_prefix(name))?;
        Ok(())
    }

    pub fn list_collections(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for (k, _) in self.db.scan_prefix(b"m/")? {
            out.push(String::from_utf8_lossy(&k[2..]).into_owned());
        }
        Ok(out)
    }

    pub fn list_indexes(&self, coll: &str) -> Result<Vec<String>> {
        Ok(self.load_meta(coll)?.indexes)
    }

    // ---------- indexes ----------

    /// Create an index on a (dotted) field and backfill it from existing documents.
    /// Only scalar values (null, bool, number, string <= 1024 bytes) are indexed.
    /// Numbers are compared as f64, so integers above 2^53 are not distinguished.
    pub fn create_index(&mut self, coll: &str, field: &str) -> Result<()> {
        validate_field_path(field)?;
        let mut meta = self.load_meta(coll)?;
        if meta.indexes.iter().any(|f| f == field) {
            return Err(DocError::AlreadyExists(format!("index on '{}' already exists", field)));
        }
        // Drop stale entries from an earlier interrupted drop_index, then backfill.
        self.delete_prefix(&index_field_prefix(coll, field))?;
        let only = vec![field.to_string()];
        let mut ops: Vec<BatchOp> = Vec::new();
        for (_, raw) in self.db.scan_prefix(&doc_prefix(coll))? {
            let d = parse_doc(&raw)?;
            for k in index_entries(coll, &only, id_of(&d), Some(&d))? {
                ops.push((k, Some(Vec::new())));
            }
        }
        self.write_chunked(ops)?;
        // The index becomes visible to queries only after it is complete.
        meta.indexes.push(field.to_string());
        self.db.write_batch(&[(catalog_key(coll), Some(meta_to_bytes(&meta)))])?;
        Ok(())
    }

    pub fn drop_index(&mut self, coll: &str, field: &str) -> Result<()> {
        let mut meta = self.load_meta(coll)?;
        if !meta.indexes.iter().any(|f| f == field) {
            return Err(DocError::NotFound(format!("no index on '{}'", field)));
        }
        meta.indexes.retain(|f| f != field);
        self.db.write_batch(&[(catalog_key(coll), Some(meta_to_bytes(&meta)))])?;
        self.delete_prefix(&index_field_prefix(coll, field))?;
        Ok(())
    }

    // ---------- documents ----------

    fn generate_id(&mut self) -> String {
        self.counter += 1;
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        format!("{:x}{:04x}", nanos, self.counter & 0xFFFF)
    }

    /// Insert a document (must be a JSON object). Returns its `_id`; one is generated if absent.
    pub fn insert(&mut self, coll: &str, doc: Value) -> Result<String> {
        let meta = self.load_meta(coll)?;
        let mut map = match doc {
            Value::Object(m) => m,
            _ => return Err(invalid("document must be a JSON object")),
        };
        let id = match map.get("_id") {
            Some(Value::String(s)) => {
                validate_id(s)?;
                s.clone()
            }
            Some(_) => return Err(invalid("_id must be a string")),
            None => {
                let mut id = self.generate_id();
                while self.db.get(&doc_key(coll, &id))?.is_some() {
                    id = self.generate_id();
                }
                map.insert("_id".into(), Value::String(id.clone()));
                id
            }
        };
        if self.db.get(&doc_key(coll, &id))?.is_some() {
            return Err(DocError::AlreadyExists(format!("document '{}' already exists", id)));
        }
        if let Some(s) = &meta.schema {
            validate_doc(s, &map)?;
        }
        let doc = Value::Object(map);
        let ops = write_ops(coll, &meta.indexes, &id, None, Some(&doc))?;
        self.db.write_batch(&ops)?;
        Ok(id)
    }

    pub fn get(&self, coll: &str, id: &str) -> Result<Option<Value>> {
        self.load_meta(coll)?;
        match self.db.get(&doc_key(coll, id))? {
            Some(b) => Ok(Some(parse_doc(&b)?)),
            None => Ok(None),
        }
    }

    /// Replace the whole document with the given id (which must exist).
    pub fn replace(&mut self, coll: &str, id: &str, doc: Value) -> Result<()> {
        let meta = self.load_meta(coll)?;
        let mut map = match doc {
            Value::Object(m) => m,
            _ => return Err(invalid("document must be a JSON object")),
        };
        match map.get("_id") {
            None => {
                map.insert("_id".into(), Value::String(id.to_string()));
            }
            Some(Value::String(s)) if s == id => {}
            Some(_) => return Err(invalid("_id cannot be changed")),
        }
        let old = match self.db.get(&doc_key(coll, id))? {
            Some(b) => parse_doc(&b)?,
            None => return Err(DocError::NotFound(format!("document '{}' does not exist", id))),
        };
        if let Some(s) = &meta.schema {
            validate_doc(s, &map)?;
        }
        let new = Value::Object(map);
        let ops = write_ops(coll, &meta.indexes, id, Some(&old), Some(&new))?;
        self.db.write_batch(&ops)?;
        Ok(())
    }

    pub fn delete(&mut self, coll: &str, id: &str) -> Result<bool> {
        let meta = self.load_meta(coll)?;
        let old = match self.db.get(&doc_key(coll, id))? {
            Some(b) => parse_doc(&b)?,
            None => return Ok(false),
        };
        let ops = write_ops(coll, &meta.indexes, id, Some(&old), None)?;
        self.db.write_batch(&ops)?;
        Ok(true)
    }

    /// Shallow-merge `set` (top-level fields only) into every document matching `filter`.
    /// All new versions are validated before anything is written. Each document is updated
    /// atomically; the set of documents as a whole is NOT atomic across a crash.
    pub fn update_where(&mut self, coll: &str, filter: &Value, set: &Value) -> Result<u64> {
        validate_filter(filter)?;
        let meta = self.load_meta(coll)?;
        let set = set.as_object().ok_or_else(|| invalid("update must be a JSON object"))?;
        if set.is_empty() {
            return Err(invalid("update must not be empty"));
        }
        for k in set.keys() {
            if k == "_id" {
                return Err(invalid("_id cannot be updated"));
            }
            if k.is_empty() || k.contains('.') {
                return Err(invalid("update keys must be top-level field names without '.'"));
            }
        }
        let docs = self.find_docs(coll, &meta, filter)?;
        let mut batches: Vec<Vec<BatchOp>> = Vec::new();
        for old in docs {
            let mut new = old.clone();
            if let Some(m) = new.as_object_mut() {
                for (k, v) in set {
                    m.insert(k.clone(), v.clone());
                }
            }
            if let (Some(s), Some(m)) = (&meta.schema, new.as_object()) {
                validate_doc(s, m)?;
            }
            batches.push(write_ops(coll, &meta.indexes, id_of(&old), Some(&old), Some(&new))?);
        }
        let n = batches.len() as u64;
        for ops in batches {
            self.db.write_batch(&ops)?;
        }
        Ok(n)
    }

    pub fn delete_where(&mut self, coll: &str, filter: &Value) -> Result<u64> {
        validate_filter(filter)?;
        let meta = self.load_meta(coll)?;
        let docs = self.find_docs(coll, &meta, filter)?;
        let mut n = 0;
        for old in docs {
            let ops = write_ops(coll, &meta.indexes, id_of(&old), Some(&old), None)?;
            self.db.write_batch(&ops)?;
            n += 1;
        }
        Ok(n)
    }

    // ---------- queries ----------

    pub fn explain(&self, coll: &str, q: &Query) -> Result<String> {
        validate_filter(&q.filter)?;
        let meta = self.load_meta(coll)?;
        Ok(choose_plan(&q.filter, &meta.indexes).describe())
    }

    pub fn find(&self, coll: &str, q: &Query) -> Result<Vec<Value>> {
        validate_filter(&q.filter)?;
        if let Some((f, _)) = &q.sort {
            validate_field_path(f)?;
        }
        let meta = self.load_meta(coll)?;
        let mut docs = self.find_docs(coll, &meta, &q.filter)?;
        docs.sort_by(|a, b| id_of(a).cmp(id_of(b))); // deterministic base order / tie-break
        if let Some((field, asc)) = &q.sort {
            docs.sort_by(|a, b| {
                let o = sort_cmp(get_path(a, field), get_path(b, field));
                if *asc {
                    o
                } else {
                    o.reverse()
                }
            });
        }
        let it = docs.into_iter().skip(q.skip);
        Ok(match q.limit {
            Some(n) => it.take(n).collect(),
            None => it.collect(),
        })
    }

    pub fn count(&self, coll: &str, filter: &Value) -> Result<usize> {
        validate_filter(filter)?;
        let meta = self.load_meta(coll)?;
        Ok(self.find_docs(coll, &meta, filter)?.len())
    }

    fn find_docs(&self, coll: &str, meta: &Meta, filter: &Value) -> Result<Vec<Value>> {
        let plan = choose_plan(filter, &meta.indexes);
        let mut out = Vec::new();
        match plan {
            Plan::Full => {
                for (_, raw) in self.db.scan_prefix(&doc_prefix(coll))? {
                    let d = parse_doc(&raw)?;
                    if doc_matches(&d, filter) {
                        out.push(d);
                    }
                }
            }
            other => {
                for id in self.index_ids(coll, &other)? {
                    if let Some(raw) = self.db.get(&doc_key(coll, &id))? {
                        let d = parse_doc(&raw)?;
                        if doc_matches(&d, filter) {
                            out.push(d);
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    fn id_from(&self, bytes: &[u8]) -> Result<String> {
        String::from_utf8(bytes.to_vec()).map_err(|_| corrupt("index entry holds a non-UTF-8 id"))
    }

    fn ids_for_value(&self, coll: &str, field: &str, v: &Value, ids: &mut BTreeSet<String>) -> Result<()> {
        if let Ok(Some(enc)) = encode_scalar(v) {
            let mut prefix = index_field_prefix(coll, field);
            prefix.extend_from_slice(&enc);
            for (k, _) in self.db.scan_prefix(&prefix)? {
                ids.insert(self.id_from(&k[prefix.len()..])?);
            }
        }
        Ok(())
    }

    fn index_ids(&self, coll: &str, plan: &Plan) -> Result<BTreeSet<String>> {
        let mut ids = BTreeSet::new();
        match plan {
            Plan::Full => {}
            Plan::IndexEq { field, value } => self.ids_for_value(coll, field, value, &mut ids)?,
            Plan::IndexIn { field, values } => {
                for v in values {
                    self.ids_for_value(coll, field, v, &mut ids)?;
                }
            }
            Plan::IndexRange { field, lo, hi } => {
                let base = index_field_prefix(coll, field);
                let enc_of = |v: &Value| encode_scalar(v).ok().flatten();
                let tag_of = |v: &Value| enc_of(v).map(|e| e[0]);

                let start = match lo {
                    Some((v, incl)) => {
                        let mut k = base.clone();
                        k.extend_from_slice(&enc_of(v).unwrap_or_default());
                        if *incl {
                            k
                        } else {
                            match prefix_successor(&k) {
                                Some(s) => s,
                                None => return Ok(ids),
                            }
                        }
                    }
                    None => {
                        let mut k = base.clone();
                        if let Some(t) = hi.as_ref().and_then(|(v, _)| tag_of(v)) {
                            k.push(t);
                        }
                        k
                    }
                };
                let end: Option<Vec<u8>> = match hi {
                    Some((v, incl)) => {
                        let mut k = base.clone();
                        k.extend_from_slice(&enc_of(v).unwrap_or_default());
                        if *incl {
                            prefix_successor(&k)
                        } else {
                            Some(k)
                        }
                    }
                    None => {
                        let mut k = base.clone();
                        if let Some(t) = lo.as_ref().and_then(|(v, _)| tag_of(v)) {
                            k.push(t);
                        }
                        prefix_successor(&k)
                    }
                };
                for (k, _) in self.db.scan_range(&start, end.as_deref())? {
                    let rest = &k[base.len()..];
                    let id = decode_id(rest).ok_or_else(|| corrupt("malformed index entry"))?;
                    ids.insert(self.id_from(id)?);
                }
            }
        }
        Ok(ids)
    }

    // ---------- maintenance ----------

    pub fn flush(&mut self) -> Result<()> {
        Ok(self.db.flush()?)
    }

    pub fn compact(&mut self) -> Result<()> {
        Ok(self.db.compact()?)
    }

    /// Consistent copy of the whole database into an empty directory (see `Db::checkpoint`).
    pub fn checkpoint(&mut self, dest: &Path) -> Result<Vec<String>> {
        Ok(self.db.checkpoint(dest)?)
    }

    /// Verify the checksum of every stored record; returns the record count.
    pub fn verify_integrity(&self) -> Result<u64> {
        Ok(self.db.verify_integrity()?)
    }

    pub fn stats(&self) -> Stats {
        self.db.stats()
    }
}
