#!/usr/bin/env bash
# Launch vLLM for Frankenstein Harness: Qwen 27B with MTP (3 draft tokens) on one RTX PRO 6000 Blackwell (96 GB).
#
#   MODEL=<hf-id-or-local-path> SERVED_NAME=qwen VLLM_API_KEY=<token> deploy/vllm/serve.sh [--print]
#
# PROFILE=balanced (default) | long-context | throughput | safe
# Every value below can be overridden with an environment variable of the same name.
# These are starting points, not measured optima: run `scripts/vps-validate.sh` after any change and read
# "Suggested vLLM and harness changes" in SUMMARY.md. See docs/BLACKWELL.md for the reasoning and the A/B plan.
set -euo pipefail

MODEL="${MODEL:?set MODEL to the checkpoint (Hugging Face id or a local path)}"
SERVED_NAME="${SERVED_NAME:-qwen}"
PROFILE="${PROFILE:-balanced}"
HOST="${HOST:-127.0.0.1}"          # keep loopback unless you firewall the port; the API key alone is a weak boundary
PORT="${PORT:-8000}"
MTP_TOKENS="${MTP_TOKENS:-3}"

case "$PROFILE" in
  balanced)     : "${MAX_LEN:=131072}"; : "${GPU_UTIL:=0.92}"; : "${KV_DTYPE:=auto}"; : "${MAX_SEQS:=16}"; : "${BATCHED:=16384}" ;;
  long-context) : "${MAX_LEN:=262144}"; : "${GPU_UTIL:=0.94}"; : "${KV_DTYPE:=fp8}";  : "${MAX_SEQS:=6}";  : "${BATCHED:=16384}" ;;
  throughput)   : "${MAX_LEN:=65536}";  : "${GPU_UTIL:=0.92}"; : "${KV_DTYPE:=fp8}";  : "${MAX_SEQS:=32}"; : "${BATCHED:=32768}" ;;
  safe)         : "${MAX_LEN:=65536}";  : "${GPU_UTIL:=0.88}"; : "${KV_DTYPE:=auto}"; : "${MAX_SEQS:=8}";  : "${BATCHED:=8192}"; MTP_TOKENS="${MTP_TOKENS_SAFE:-2}" ;;
  *) echo "unknown PROFILE=$PROFILE" >&2; exit 2 ;;
esac

ARGS=(
  serve "$MODEL"
  --served-model-name "$SERVED_NAME"
  --host "$HOST" --port "$PORT"
  --max-model-len "$MAX_LEN"
  --gpu-memory-utilization "$GPU_UTIL"
  --kv-cache-dtype "$KV_DTYPE"
  --max-num-seqs "$MAX_SEQS"
  --max-num-batched-tokens "$BATCHED"
  --enable-prefix-caching
  --enable-chunked-prefill
  --reasoning-parser qwen3
  --enable-auto-tool-choice --tool-call-parser qwen3_coder
  --speculative-config "{\"method\":\"mtp\",\"num_speculative_tokens\":${MTP_TOKENS}}"
  # The harness sends its own sampling parameters on every request; ignore the checkpoint's generation_config defaults.
  --generation-config vllm
)
# Extra flags for experiments, e.g. EXTRA="--async-scheduling"
if [ -n "${EXTRA:-}" ]; then read -r -a X <<<"$EXTRA"; ARGS+=("${X[@]}"); fi
# Do NOT pass --dtype/--quantization for a pre-quantized (FP8/NVFP4) checkpoint: vLLM reads it from the model config.
# The API key is read from the VLLM_API_KEY environment variable (never put it on the command line: it shows up in `ps`).

if [ "${1:-}" = "--print" ]; then
  printf 'VLLM_API_KEY=<from env> vllm'; printf ' %q' "${ARGS[@]}"; echo
  exit 0
fi
command -v vllm >/dev/null 2>&1 || { echo "vllm not found on PATH (activate your environment)" >&2; exit 2; }
exec vllm "${ARGS[@]}"
