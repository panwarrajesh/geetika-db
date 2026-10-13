#!/usr/bin/env bash
# Runs every check in order and writes verification-report.txt (paste its contents when asking for help).
#   scripts/verify-all.sh            full run (needs internet for npm install; takes a while on the first Rust build)
#   scripts/verify-all.sh --quick    only checks that need no downloads (Rust tests if cargo exists, JS and Python logic tests)
#   scripts/verify-all.sh --bench    additionally run the small engine benchmark
# Missing tools are reported as SKIPPED, never as success.
set -u
cd "$(dirname "$0")/.."
ROOT="$(pwd)"
QUICK=0; BENCH=0
for a in "$@"; do case "$a" in --quick) QUICK=1;; --bench) BENCH=1;; *) echo "unknown option $a"; exit 2;; esac; done
LOGDIR="$(mktemp -d)"
REPORT="$ROOT/verification-report.txt"
NAMES=(); RESULTS=(); LOGS=()

step() { # name dir tool command...
  local name="$1" dir="$2" tool="$3"; shift 3
  local log="$LOGDIR/${#NAMES[@]}.log"
  NAMES+=("$name"); LOGS+=("$log")
  if ! command -v "$tool" >/dev/null 2>&1; then RESULTS+=("SKIPPED (no $tool)"); echo "-- $name: SKIPPED ($tool not installed)"; return; fi
  echo "-- $name ..."
  ( cd "$ROOT/$dir" && "$@" ) >"$log" 2>&1
  local code=$?
  if [ $code -eq 0 ]; then RESULTS+=("PASS"); else RESULTS+=("FAIL (exit $code)"); fi
  echo "   ${RESULTS[${#RESULTS[@]}-1]}"
}
skipped() { NAMES+=("$1"); RESULTS+=("SKIPPED ($2)"); LOGS+=("/dev/null"); echo "-- $1: SKIPPED ($2)"; }

PY=python3; command -v python3 >/dev/null 2>&1 || PY=python

# ---- Rust
step "rust: core tests (engine, crash-kill)" engine cargo cargo test -p rajeshdb-core
step "rust: docs layer tests"              engine cargo cargo test -p rajeshdb-docs
step "rust: server (phase 4) tests"        engine cargo cargo test -p rajeshdb-server
step "rust: platform tests"                engine cargo cargo test -p rajeshdb-platform
if [ $QUICK -eq 0 ]; then step "rust: release build of all crates" engine cargo cargo build --release --workspace; else skipped "rust: release build" "--quick"; fi
# ---- JavaScript / Python logic (no downloads needed)
step "dashboard: pure-logic tests" dashboard node node --test src/lib.test.ts
step "loadtest: stats tests"       loadtest  node node --test loadtest.test.mjs
step "sdk js: tests"               sdk/js    node node --test test/sdk.test.ts
step "sdk python: tests"           sdk/python "$PY" "$PY" -m unittest discover -s tests -v
# ---- needs internet / docker
if [ $QUICK -eq 0 ]; then
  step "dashboard: npm install"    dashboard npm npm install
  step "dashboard: typecheck"      dashboard npm npm run typecheck
  step "dashboard: production build" dashboard npm npm run build
  if [ ! -f deploy/.env ]; then cp deploy/.env.example deploy/.env; TMPENV=1; fi
  step "deploy: docker compose config" deploy docker docker compose config -q
  [ "${TMPENV:-0}" = 1 ] && rm -f deploy/.env
else
  skipped "dashboard: npm install/typecheck/build" "--quick"; skipped "deploy: docker compose config" "--quick"
fi
if [ $BENCH -eq 1 ]; then step "engine benchmark (small)" engine cargo cargo run --release -p rajeshdb-bench -- --size small; fi

# ---- report
{
  echo "RajeshDB verification report"
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)   host: $(uname -srm 2>/dev/null)"
  for t in "rustc --version" "cargo --version" "node --version" "npm --version" "$PY --version" "docker --version"; do
    tool="${t%% *}"
    if command -v "$tool" >/dev/null 2>&1; then printf '%s: ' "$t"; $t 2>&1 | head -1; else echo "$tool: not installed"; fi
  done
  echo
  echo "RESULTS"
  for i in "${!NAMES[@]}"; do printf '  %-44s %s\n' "${NAMES[$i]}" "${RESULTS[$i]}"; done
  echo
  for i in "${!NAMES[@]}"; do
    case "${RESULTS[$i]}" in FAIL*) echo "===== ${NAMES[$i]}: last 60 lines ====="; tail -n 60 "${LOGS[$i]}"; echo;; esac
    if [ "${NAMES[$i]}" = "engine benchmark (small)" ] && [ "${RESULTS[$i]}" = PASS ]; then echo "===== benchmark output ====="; cat "${LOGS[$i]}"; echo; fi
  done
} > "$REPORT"
echo; echo "Report written to: $REPORT"
sed -n '/^RESULTS/,/^$/p' "$REPORT"
if grep -q "FAIL" "$REPORT"; then echo "RESULT: at least one step FAILED"; exit 1; fi
if grep -q "SKIPPED (no " "$REPORT"; then echo "RESULT: no failures, but some tools are missing: SKIPPED is NOT a pass"; exit 3; fi
echo "RESULT: all executed steps passed"; exit 0
