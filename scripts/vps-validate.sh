#!/usr/bin/env bash
# One-shot validation on the machine that hosts your vLLM:
#   1. build the Rust binary and run the harness self-tests (mock server, no model needed)
#   2. record the environment (GPU, vLLM version and launch flags with secrets redacted)
#   3. vLLM measurements (tool calls, MTP, prefix cache, long context, concurrency, cancellation)
#   4. eval corpus: Frankenstein Harness vs plain Qwen Code on the same model (baseline optional)
#   5. web UI smoke test over real HTTP
#   6. SUMMARY.md graded against the success criteria
#
# Usage:
#   FH_ENDPOINT=http://127.0.0.1:8000/v1 FH_MODEL=<served-model-name> FH_API_KEY=<token> \
#     scripts/vps-validate.sh [--quick] [--no-baseline] [--repeat N] [--tasks DIR] [--skip-tests]
#
# The API key is read from the environment (or prompted, hidden). It is never printed or written to a report.
set -uo pipefail
cd "$(dirname "$0")/.."

QUICK=""; BASELINE=1; REPEAT=3; TASKS="eval/tasks"; SKIP_TESTS=0
while [ $# -gt 0 ]; do
  case "$1" in
    --quick) QUICK="--quick"; REPEAT=1 ;;
    --no-baseline) BASELINE=0 ;;
    --skip-tests) SKIP_TESTS=1 ;;
    --repeat) REPEAT="${2:-3}"; shift ;;
    --tasks) TASKS="${2:-eval/tasks}"; shift ;;
    -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
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
step() { printf '\n==> %s\n' "$*"; }
FAILED=()

step "environment"
[ -x "$HOME/.cargo/bin/cargo" ] && export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust toolchain not found. Install it (user-space, no root) with:" >&2
  echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && . \"\$HOME/.cargo/env\"" >&2
  exit 2
fi
command -v git >/dev/null 2>&1 || { echo "git is required" >&2; exit 2; }
command -v gcc >/dev/null 2>&1 || command -v cc >/dev/null 2>&1 || { echo "a C compiler is required (apt install build-essential)" >&2; exit 2; }
echo "rust $(rustc --version | cut -d' ' -f2), $(uname -sr), $(nproc 2>/dev/null || echo ?) cpus, python3: $(python3 --version 2>&1 | head -1)"
{
  echo "rust: $(rustc --version)"; uname -a; git rev-parse HEAD 2>/dev/null
  echo "--- GPU"
  if command -v nvidia-smi >/dev/null 2>&1; then
    nvidia-smi --query-gpu=name,memory.total,driver_version,compute_cap --format=csv,noheader 2>&1
    nvidia-smi -L 2>&1 | head -4
  else echo "nvidia-smi not found on this host (is vLLM running elsewhere?)"; fi
  echo "--- vLLM (from the endpoint)"
  ORIGIN="$(printf '%s' "$FH_ENDPOINT" | sed -E 's#^(https?://[^/]+).*#\1#')"
  AUTH=(); [ -n "${FH_API_KEY:-}" ] && [ "${FH_AUTH_SCHEME:-bearer}" != none ] && AUTH=(-H "Authorization: Bearer $FH_API_KEY")
  echo "version: $(curl -s --max-time 5 "${AUTH[@]}" "$ORIGIN/version" 2>/dev/null | head -c 200)"
  curl -s --max-time 5 "${AUTH[@]}" "$FH_ENDPOINT/models" 2>/dev/null | head -c 600; echo
  echo "--- vLLM launch flags (secrets redacted)"
  ps -eo args 2>/dev/null | grep -i '[v]llm' | head -3 | sed -E 's/(--api-key[= ])[^ ]+/\1«redacted»/g; s/(token|secret|password|key)=([^ ]+)/\1=«redacted»/Ig'
  echo "--- python vllm/torch"
  python3 -c "import vllm,torch;print('vllm',vllm.__version__,'torch',torch.__version__,'cuda',torch.version.cuda)" 2>&1 | tail -1
} > "$OUT/environment.txt" 2>&1
sed -n '1,12p' "$OUT/environment.txt"

step "build (release)"
if cargo build --release >"$OUT/build.log" 2>&1; then echo "built target/release/fh"; else echo "BUILD FAILED, see $OUT/build.log"; tail -20 "$OUT/build.log"; FAILED+=("build"); exit 1; fi
FH="./target/release/fh"

if [ "$SKIP_TESTS" = 0 ]; then
  step "harness self-tests (mock server, no model needed)"
  cargo test --release >"$OUT/unit.log" 2>&1; UNIT=$?
  grep -E '^test result' "$OUT/unit.log" | sed 's/finished in.*//' | tr '\n' ' '; echo
  [ $UNIT -eq 0 ] || { FAILED+=("unit tests"); grep -E 'FAILED|panicked' "$OUT/unit.log" | head -10; }
  cargo test --release --test web >"$OUT/web-tests.log" 2>&1 || true
fi

step "doctor"
$FH doctor 2>&1 | tee "$OUT/doctor.log"

step "vLLM measurements (this is the long part)"
$FH validate-vllm $QUICK --out "$OUT" 2>&1 | tee "$OUT/validate-vllm.log"
[ "${PIPESTATUS[0]}" -eq 0 ] || FAILED+=("validate-vllm")

RUNNER=fh
if [ "$BASELINE" = 1 ]; then
  if command -v qwen >/dev/null 2>&1; then RUNNER=both
  elif command -v npm >/dev/null 2>&1 && npm install -g @qwen-code/qwen-code --loglevel=error >"$OUT/qwen-install.log" 2>&1; then RUNNER=both
  else echo "plain Qwen Code baseline unavailable (needs Node/npm and \`npm i -g @qwen-code/qwen-code\`); running fh only"; fi
  [ "$RUNNER" = both ] && qwen --version 2>&1 | head -1 | sed 's/^/baseline qwen-code /'
fi
command -v python3 >/dev/null 2>&1 || echo "WARNING: python3 not found; the built-in eval tasks need it (or pass --tasks with your own)"
step "eval: Frankenstein Harness$([ "$RUNNER" = both ] && echo " vs plain Qwen Code") (repeat $REPEAT)"
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
  if command -v ss >/dev/null 2>&1; then echo "listening on: $(ss -ltn 2>/dev/null | grep ":$PORT " | awk '{print $4}' | tr '\n' ' ') (expect 127.0.0.1 only)"
  elif command -v lsof >/dev/null 2>&1; then echo "listening on: $(lsof -nP -iTCP:$PORT -sTCP:LISTEN 2>/dev/null | awk 'NR>1{print $9}' | tr '\n' ' ') (expect 127.0.0.1 only)"
  else echo "listening on: (ss/lsof not available, skipped)"; fi
} | tee "$OUT/web-smoke.txt"
kill "$SRV" 2>/dev/null; wait "$SRV" 2>/dev/null

step "summary"
$FH summarize "$OUT" "${FAILED[*]:-}" | tee "$OUT/SUMMARY.md"
echo
echo "All artifacts: $OUT   (no secrets are stored in them)"
[ ${#FAILED[@]} -eq 0 ]
