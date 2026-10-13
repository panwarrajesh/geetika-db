# Local-to-cloud synchronization: design (NOT implemented)

Goal: work offline against a local RajeshDB, later exchange changes with a cloud project without silent data loss.
Copying database files between machines is NOT a safe way to do this (two machines would each have their own WAL/manifest history), so sync works at the level of documents.

## Building blocks
1. **Per-document version:** every document gets a monotonically increasing `_v` (integer) and a `_updated` hybrid-logical-clock timestamp, set by the database on each write. Deletes leave a small tombstone `{_id, _v, _deleted: true}` for a retention period.
2. **Change feed per collection:** an append-only log of `(seq, _id, _v, op)` written in the same atomic batch as the document change; `seq` is a per-database counter. A client asks "changes since cursor N".
3. **Replica identity:** each local database has a random replica id; the cloud project records for each replica the last cursor it has received and the last cursor it has sent.

## Protocol (two phases, both resumable)
* **Pull:** local asks cloud for changes after `cloud_cursor`; applies each. Cursor is stored AFTER the batch is applied, so an interruption repeats at most one batch, and applying is idempotent (same `_id` + `_v` = no-op).
* **Push:** local sends its changes after `local_cursor` together with the `_v` each document had when it was last synchronized (`base_v`). The cloud accepts a change only if the cloud's current `_v` equals `base_v` (compare-and-set). Otherwise it is a **conflict**.

## Conflicts
* A conflict is never resolved by silently picking a winner. Both versions are kept: the cloud stores the rejected local version in a `_conflicts` collection (`_id`, both versions, both timestamps, replica id) and the sync result lists them.
* Resolution policies the application may choose per collection: `manual` (default), `server-wins`, `local-wins`, `last-writer-wins by _updated` (explicitly lossy), or a field-level merge function supplied by the app.
* Deletes conflict with edits the same way (edit-after-delete and delete-after-edit are both reported).

## Safety rules
* Authentication: a sync session uses a project API key with the `write` role; sync respects the same quotas and audit log.
* Schema/index differences are not synchronized; collections must exist on both sides.
* The change feed has a retention window; a replica that stays offline longer must do a full resync (export the cloud collection, then pull).
* Clocks: ordering uses `_v` and the change-feed `seq`, never wall-clock time, except for the optional last-writer-wins policy.

## Tests required before anyone may call it supported
Interrupted pull/push at every step (kill the client between writes), duplicate delivery, conflicting edits from two replicas, delete/edit races, clock skew, quota exhaustion mid-sync, large backlogs, schema violations on the receiving side.
