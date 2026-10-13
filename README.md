# RajeshDB Cloud

Own database platform, built in verifiable phases. See `docs/STATUS.md` for what works today.

## Phase 2 — local storage engine (Rust)

### Windows setup (all free)

1. Install **Git**: https://git-scm.com
2. Install **Visual Studio Build Tools** (workload: _Desktop development with C++_): https://visualstudio.microsoft.com/visual-cpp-build-tools/
3. Install **Rust** from https://rustup.rs (choose the default `stable` toolchain, MSVC).
4. Open a **new** PowerShell and verify:
   ```powershell
   rustc --version
   cargo --version
   ```

### Build and test

```powershell
cd rajeshdb\engine
cargo test
cargo build --release
```

### Try the CLI

```powershell
.\target\release\rajeshdb.exe put name Rajesh
.\target\release\rajeshdb.exe get name
.\target\release\rajeshdb.exe scan
.\target\release\rajeshdb.exe stats
.\target\release\rajeshdb.exe shell
```

Data is stored in `.\rajeshdb-data` (change with `--data D:\mydata`).

### Crash test by hand

Run `shell`, `put a 1`, then close the window with the X button (no `exit`). Start again and `get a`.

## Phase 3 — documents, indexes, queries

New crate `rajeshdb-docs` (collections of JSON documents on top of the engine) and an
atomic `write_batch` in `rajeshdb-core`.

```powershell
cd rajeshdb\engine
cargo test            # runs core + docs tests
cargo build --release
```

Try it (document mode uses its own folder, `.\rajeshdb-docs-data`):

```powershell
.\target\release\rajeshdb.exe --docs shell
```

```
create-collection users
create-index users city
insert users {"name":"Asha","city":"Delhi","age":30}
insert users {"name":"Ben","city":"Pune","age":25}
find users {"city":"Delhi"}
find users {"age":{"$gte":20,"$lt":30}}
query users {"filter":{},"sort":{"field":"age","order":"desc"},"limit":1}
explain users {"city":"Delhi"}
```

Query language: equality, `$eq $ne $gt $gte $lt $lte $in $nin $exists`, `$and`, `$or`,
dotted paths (`"address.city"`). See `docs/STATUS.md` for limits.

## Phase 4 — HTTP server and secure API

```powershell
cd rajeshdb\engine
cargo test
cargo build --release

# 1. create an admin key (shown ONCE; only its hash is stored in rajeshdb-keys.json)
.\target\release\rajeshdb-server.exe keygen my-admin admin

# 2. start the server (listens on 127.0.0.1:8080 only)
.\target\release\rajeshdb-server.exe serve
```

In a second PowerShell window (replace the key):

```powershell
$h = @{ Authorization = "Bearer rdb_PASTE_YOUR_KEY_HERE" }
Invoke-RestMethod http://127.0.0.1:8080/health
Invoke-RestMethod -Method Post -Headers $h -ContentType "application/json" `
  -Uri http://127.0.0.1:8080/v1/collections -Body '{"name":"users"}'
Invoke-RestMethod -Method Post -Headers $h -ContentType "application/json" `
  -Uri http://127.0.0.1:8080/v1/collections/users/documents -Body '{"name":"Asha","city":"Delhi"}'
Invoke-RestMethod -Method Post -Headers $h -ContentType "application/json" `
  -Uri http://127.0.0.1:8080/v1/collections/users/query -Body '{"filter":{"city":"Delhi"}}'
Invoke-RestMethod -Headers $h http://127.0.0.1:8080/v1/admin/metrics
```

Create a read-only key for apps that only read: `rajeshdb-server keygen my-app read`.
Revoke: `rajeshdb-server keys revoke <id>` then restart the server.
Config via environment variables: run `rajeshdb-server help`. API reference: `docs/openapi.yaml`.
