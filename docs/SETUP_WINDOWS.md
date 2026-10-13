# Windows setup (development, free)

**Versions:** this project has not been compiled by its author, so no "known good" versions can be promised. Use the current stable releases and let
`scripts\verify-all.ps1` record exactly what you have in `verification-report.txt`. Minimums that the code relies on: Node.js 22.18 or newer (for `npm test`,
which runs TypeScript directly), a recent stable Rust toolchain (install with rustup; update with `rustup update`), Python 3.9+ for the Python SDK.

## 1. Install the tools (PowerShell, any one of the two methods)

With winget (Windows 10/11):

    winget install --id Git.Git
    winget install --id Rustlang.Rustup
    winget install --id OpenJS.NodeJS.LTS
    winget install --id Python.Python.3.12
    winget install --id Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
    winget install --id Docker.DockerDesktop        # optional: only for the deployment rehearsal
    winget install --id Microsoft.VisualStudioCode  # optional

Or download from the official sites: git-scm.com, rustup.rs (choose the default MSVC toolchain), nodejs.org (LTS), python.org, visualstudio.microsoft.com/visual-cpp-build-tools
(workload "Desktop development with C++"), docker.com.

Open a NEW PowerShell window and check:

    git --version ; rustc --version ; cargo --version ; node --version ; npm --version ; python --version

## 2. Get the project and verify it

    cd D:\
    # unzip rajeshdb-complete.zip here, giving D:\rajeshdb
    cd D:\rajeshdb
    powershell -ExecutionPolicy Bypass -File scripts\verify-all.ps1

Read the summary table. Anything FAIL: open `verification-report.txt` (it contains the last 60 lines of each failed step) and fix or ask for help with that file.
`-Quick` skips downloads; `-Bench` also runs the small benchmark.

## 3. Folder structure

    rajeshdb\
      engine\                    Rust workspace (cargo test / cargo build here)
        crates\rajeshdb-core      storage engine (+ crash_writer test helper)
        crates\rajeshdb-docs      document layer
        crates\rajeshdb-server    Phase 4 single-database server
        crates\rajeshdb-platform  multi-tenant platform server  <- the one you run
        crates\rajeshdb-cli       local command line
        crates\rajeshdb-bench     benchmarks
      dashboard\                 React + TypeScript console
      sdk\js  sdk\python         client libraries
      examples\                  Node backend, React component, Python script
      loadtest\                  HTTP load-test tool (Node)
      deploy\                    Docker, Compose, Caddy, off-site backup script
      scripts\                   verify-all.ps1 / .sh
      docs\                      architecture, limits, deployment, operations, security, performance, readiness, roadmap

## 4. Run everything locally (two PowerShell windows)

Window 1 - the server (data goes to `engine\rajeshdb-platform-data`, nothing leaves your machine):

    cd D:\rajeshdb\engine
    $env:RAJESHDB_OPERATOR_TOKEN = "choose-a-long-random-secret-here-123"   # optional: enables the Operations page
    cargo run --release -p rajeshdb-platform -- serve

Window 2 - the dashboard:

    cd D:\rajeshdb\dashboard
    npm install
    npm run dev                      # open http://localhost:5173 , Register, then create an organization and a project

Stop with Ctrl+C (the server flushes its data on shutdown). It works without internet. Backups: dashboard -> environment -> Backups.
Environment variables are listed by `cargo run --release -p rajeshdb-platform -- help`; for an `.env`-style example see `deploy\.env.example`.

## 5. Raw engine from the command line (no accounts, local only)

    cd D:\rajeshdb\engine
    cargo build --release
    .\target\release\rajeshdb.exe put name Rajesh
    .\target\release\rajeshdb.exe get name
    .\target\release\rajeshdb.exe --docs shell        # document mode, own data folder
