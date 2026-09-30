#!/usr/bin/env bash
# One-shot validation on the machine that hosts your vLLM:
#   1. harness self-tests (unit + web security + e2e against a mock server)
#   2. vLLM measurements (tool calls, MTP, prefix cache, long context, concurrency, cancellation)
#   3. eval corpus: Frankenstein Harness vs plain Qwen Code on the same model (optional baseline)
#   4. web UI smoke test over real HTTP
#   5. SUMMARY.md against the success criteria
#
# Usage:
#   FH_ENDPOINT=http://127.0.0.1:8000/v1 FH_MODEL=<served-model-name> FH_API_KEY=<token> \
#     scripts/vps-validate.sh [--quick] [--no-baseline] [--repeat N] [--tasks DIR]
#
# The API key is read from the environment (or prompted, hidden). It is never printed or written to a report.
set -uo pipefail
cd "$(dirname "$0")/.."

QUICK=""; BASELINE=1; REPEAT=3; TASKS="eval/tasks"
while [ $# -gt 0 ]; do
  case "$1" in
    --quick) QUICK="--quick"; REPEAT=1 ;;
    --no-baseline) BASELINE=0 ;;
    --repeat) REPEAT="${2:-3}"; shift ;;
    --tasks) TASKS="${2:-eval/tasks}"; shift ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

export FH_ENDPOINT="${FH_ENDPOINT:-http://127.0.0.1:8000/v1}"
: "${FH_MODEL:?set FH_MODEL to the served model name (curl \$FH_ENDPOINT/models to list it)}"
if [ -z "${FH_API_KEY:-}" ]; then
  if [ -t 0 ]; then read -r -s -p "API key for $FH_ENDPOINT (Enter for none): " FH_API_KEY; echo; export FH_API_KEY; fi
  [ -z "${FH_API_KEY:-}" ] && export FH_AUTH_SCHEME=none
fi

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="reports/vps-$STAMP"; mkdir -p "$OUT"
FH="node --disable-warning=ExperimentalWarning bin/fh.ts"
step() { printf '\n==> %s\n' "$*"; }
FAILED=()

step "environment"
NODE_V="$(node -v 2>/dev/null || echo none)"
echo "node $NODE_V, $(uname -sr), $(nproc 2>/dev/null || echo ?) cpus"
node -e 'const [a,b]=process.versions.node.split(".").map(Number); process.exit(a>22||(a===22&&b>=18)?0:1)' \
  || { echo "Node >= 22.18 is required (found $NODE_V). Install Node 22 LTS or newer and re-run." >&2; exit 2; }
{ echo "node $NODE_V"; uname -a; git rev-parse HEAD 2>/dev/null; command -v nvidia-smi >/dev/null && nvidia-smi --query-gpu=name,memory.total --format=csv,noheader; } > "$OUT/environment.txt" 2>&1

step "install dependencies (dev only: typescript, @types/node)"
npm install --no-audit --no-fund --loglevel=error >"$OUT/install.log" 2>&1 || { echo "npm install failed, see $OUT/install.log"; FAILED+=("install"); }

step "typecheck + harness self-tests (mock server, no model needed)"
npx tsc -p tsconfig.json >"$OUT/typecheck.log" 2>&1 && echo "typecheck ok" || { echo "typecheck FAILED"; FAILED+=("typecheck"); }
npm test >"$OUT/unit.log" 2>&1; UNIT=$?
grep -E '^# (tests|pass|fail)' "$OUT/unit.log" | tr '\n' ' '; echo
[ $UNIT -eq 0 ] || FAILED+=("unit tests")
grep -E '^(not )?ok .*web' "$OUT/unit.log" > "$OUT/web-security-tests.txt" || true

step "doctor"
$FH doctor 2>&1 | tee "$OUT/doctor.log"

step "vLLM measurements (this is the long part)"
$FH validate-vllm $QUICK --out "$OUT" 2>&1 | tee "$OUT/validate-vllm.log"
[ "${PIPESTATUS[0]}" -eq 0 ] || FAILED+=("validate-vllm")

step "eval: Frankenstein Harness${BASELINE:+ vs plain Qwen Code} (repeat $REPEAT)"
RUNNER=fh
if [ "$BASELINE" = 1 ]; then
  if command -v qwen >/dev/null 2>&1 || npm install -g @qwen-code/qwen-code --loglevel=error >"$OUT/qwen-install.log" 2>&1; then
    RUNNER=both; qwen --version 2>&1 | head -1 | sed 's/^/baseline qwen-code /'
  else
    echo "could not install @qwen-code/qwen-code (see $OUT/qwen-install.log); running fh only"
  fi
fi
$FH eval --tasks "$TASKS" --runner "$RUNNER" --repeat "$REPEAT" --out "$OUT" 2>&1 | tee "$OUT/eval.log"

step "web UI smoke test (real HTTP, loopback only)"
PORT=$((20000 + RANDOM % 20000))
$FH serve --port "$PORT" >"$OUT/serve.log" 2>&1 &
SRV=$!
for _ in $(seq 1 30); do curl -s -o /dev/null "http://127.0.0.1:$PORT/" && break; sleep 0.3; done
{
  echo "GET / -> $(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/") (expect 200)"
  echo "CSP header: $(curl -sI "http://127.0.0.1:$PORT/" | tr -d '\r' | grep -i '^content-security-policy' | cut -c1-80)"
  echo "forged Host -> $(curl -s -o /dev/null -w '%{http_code}' -H 'Host: evil.example' "http://127.0.0.1:$PORT/") (expect 403)"
  echo "API without cookie -> $(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/api/state") (expect 401)"
  echo "foreign Origin POST -> $(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' -H 'X-FH: 1' -H 'Origin: http://evil.example' -d '{}' "http://127.0.0.1:$PORT/api/auth") (expect 403)"
  echo "bound to loopback only: $(ss -ltn 2>/dev/null | grep ":$PORT " | awk '{print $4}' | tr '\n' ' ')"
} | tee "$OUT/web-smoke.txt"
kill "$SRV" 2>/dev/null; wait "$SRV" 2>/dev/null

step "summary"
node --disable-warning=ExperimentalWarning scripts/summarize.ts "$OUT" "${FAILED[*]:-}" | tee "$OUT/SUMMARY.md"
echo
echo "All artifacts: $OUT   (no secrets are stored in them)"
[ ${#FAILED[@]} -eq 0 ]
