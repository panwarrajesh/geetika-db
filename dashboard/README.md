# RajeshDB Console (Phase 6)

React + TypeScript + Vite dashboard for the Phase 5 platform API. No UI framework: plain CSS, so the
only runtime dependencies are react and react-dom.

## Run (Windows)
Needs Node.js LTS (https://nodejs.org, version 22.18 or newer, needed only for the "npm test" command).

Terminal 1 - the API server:
    cd rajeshdb\engine
    cargo run --release -p rajeshdb-platform -- serve

Terminal 2 - the dashboard:
    cd rajeshdb\dashboard
    npm install
    npm run dev
Open http://localhost:5173 , click "Register", then create an organization -> project -> API key.

In development Vite forwards /api/* to http://127.0.0.1:8081 (override with RAJESHDB_API), so no CORS setup is needed.

## Commands
- npm run dev        development server
- npm run typecheck  strict TypeScript check
- npm test           unit tests of the pure logic in src/lib.ts (Node built-in test runner, no extra packages)
- npm run build      production build in dist/ (serve it from the same origin as the API, e.g. behind the Phase 8 reverse proxy)

## What each page shows (all numbers come from the API, nothing is hard-coded)
- Organizations / members / audit log
- Projects with dev, staging, prod storage meters (bytes measured on disk vs quota)
- API keys: create (shown once), rotate with grace period, revoke
- Data browser: collections, quick or JSON filter, server-side sort (click a column), pagination, create/edit/delete documents, indexes
- Query editor: JSON queries with the chosen plan (IndexScan or FullScan)
- Server liveness indicator (GET /health every 15 s)
- "Operations" page lists honestly what is NOT available yet (backup/restore, metrics, logs, billing, SQL)

## Security notes
- The session token is kept in sessionStorage (this tab only). Any script running on the page could read it, so the console loads no third-party scripts.
  Add a Content-Security-Policy header in the production reverse proxy.
- React escapes all values; there is no dangerouslySetInnerHTML.
- The UI hides actions your role cannot use, but the server enforces every permission.
