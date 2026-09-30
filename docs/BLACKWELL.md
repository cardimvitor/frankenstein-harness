# Running on an RTX 6000 (Blackwell) with Qwen and MTP=3

The harness only talks to vLLM over HTTP, so the GPU does not change the harness. What the GPU and the model build change is **what the validation must confirm on your server**. This page lists what is known in general and what could not be verified from here.

## What I could not verify

- The exact model you called "Qwen 3.8 27B, version related to Blackwell". I have no reliable information about that release, its architecture, its MTP head, or whether the Blackwell build is a quantized variant (for example FP8 or NVFP4). Treat everything about the model as unverified until `fh validate-vllm` has run against it.
- Which vLLM version and flags your service uses. The validator reads them from the running server and records them; see below.

## Generally true and worth checking

- **Compute capability.** Blackwell workstation/consumer GPUs (RTX PRO 6000 Blackwell, RTX 50-series) report compute capability 12.0 (`sm_120`); data-center Blackwell (B200/GB200) is 10.0. Both need CUDA 12.8 or newer and a PyTorch and vLLM build that includes those architectures. If vLLM was installed from an older wheel, it can fail to start or silently fall back to slow kernels. `nvidia-smi --query-gpu=name,compute_cap,driver_version --format=csv` shows what you have.
- **Memory.** An RTX PRO 6000 Blackwell has 96 GB. A 27B model in BF16 needs roughly 54 GB for weights, leaving the remainder for KV cache; FP8 roughly halves the weights and leaves much more KV room, which is what allows longer contexts and more parallel workers. The harness's `maxConcurrency` should follow the KV headroom, which is exactly what the concurrency probe measures.
- **MTP=3.** Speculative decoding with 3 draft tokens pays off only if the acceptance rate is high enough. Acceptance is usually higher on predictable output (tool-call JSON, diffs) than on free prose, which is why the harness shapes its output that way. The validator reports acceptance per draft position and for prose, JSON and code separately, so you can see whether position 2 and 3 still earn their cost. If the later positions accept rarely, MTP=2 may be faster.
- **Prefix caching.** Make sure `--enable-prefix-caching` is on; the harness keeps its system prompt and tool schemas byte-stable so repeated turns hit the cache. The validator compares a stable prefix against a variable-first prompt and warns if caching is not visibly effective.
- **Reasoning and tool parsers.** `--reasoning-parser qwen3` and `--tool-call-parser qwen3_coder` (with `--enable-auto-tool-choice`) are what the harness expects. Newer Qwen generations may need a different tool-call parser name; the validator's tool-call reliability, think-leak and streamed-vs-non-streamed probes are there to catch a mismatch.
- **Structured output plus thinking.** Some vLLM versions apply guided decoding from the first generated token, which conflicts with the reasoning block. The harness detects this and retries without the schema and with thinking off; the "structured JSON output with thinking on" probe tells you which behavior your server has.

## What the VPS run records for you

`scripts/vps-validate.sh` writes `environment.txt` with the GPU name, memory, driver and compute capability, the vLLM version from `GET /version`, the served model list (with `max_model_len`), the vLLM process flags with secrets redacted, and the Python `vllm`/`torch`/CUDA versions when importable. Send that file along with `SUMMARY.md` when you ask for tuning advice; it is what settles the open questions above.

## Launch profiles (`deploy/vllm/serve.sh`)

```bash
MODEL=<checkpoint> SERVED_NAME=qwen VLLM_API_KEY=<token> deploy/vllm/serve.sh          # PROFILE=balanced
PROFILE=long-context MODEL=... deploy/vllm/serve.sh                                     # 256K window, fp8 KV, fewer sequences
PROFILE=throughput   MODEL=... deploy/vllm/serve.sh                                     # more parallel workers, 64K window
PROFILE=safe         MODEL=... deploy/vllm/serve.sh                                     # smaller everything, MTP 2, for first bring-up
deploy/vllm/serve.sh --print                                                            # show the command without running it
```

| Setting | balanced | Why |
|---|---|---|
| `--max-model-len` | 131072 | Enough for large repos with room for tool output; the long-context probe tells you where retrieval actually degrades, then lower it to that. |
| `--gpu-memory-utilization` | 0.92 | Leaves headroom for CUDA graphs and activations on a 96 GB card; raise only if the KV usage probe shows pressure and there is no OOM. |
| `--kv-cache-dtype` | auto | Unquantized KV is the accuracy-safe start. `fp8` roughly doubles KV capacity: A/B it (below). |
| `--max-num-seqs` | 16 | A few parallel workers + reviewer + the user's own turn; the concurrency probe recommends the harness-side `maxConcurrency`. |
| `--max-num-batched-tokens` | 16384 | Chunked prefill size: large enough for fast prefill of long repo context, small enough that decode for other requests is not starved. |
| `--enable-prefix-caching`, `--enable-chunked-prefill` | on | The harness keeps a byte-stable system prompt and tool schema for cache hits. |
| `--speculative-config` | `mtp`, 3 tokens | As you specified; the validator reports per-position acceptance so 3 vs 2 is a measurement, not a guess. |
| `--reasoning-parser qwen3`, `--tool-call-parser qwen3_coder`, `--enable-auto-tool-choice` | on | What the harness expects; probes verify them. |
| `--generation-config vllm` | on | The harness sends its own sampling parameters every request, so the checkpoint's defaults are ignored. |
| API key | `VLLM_API_KEY` env | Not on the command line, where `ps` would show it. Bind to `127.0.0.1` unless firewalled. |

**Not set on purpose:** `--dtype` and `--quantization` (a pre-quantized FP8/NVFP4 checkpoint declares them itself), attention-backend environment variables (leave the default; if Blackwell kernels misbehave on your vLLM build, that is the first thing to try, and it is a version-specific fix I cannot verify from here), and `--async-scheduling` (available in newer releases; try it via `EXTRA="--async-scheduling"` and measure).

## A/B plan (each step: change one thing, re-run `scripts/vps-validate.sh --quick`, compare)

1. **MTP tokens 3 vs 2** (`MTP_TOKENS=2`): keep 3 only if position 3 is accepted often enough that end-to-end eval time improves.
2. **KV dtype auto vs fp8** (`KV_DTYPE=fp8`): keep fp8 only if the long-context retrieval probe stays clean at your target size and the concurrency probe shows headroom gained.
3. **`max-num-batched-tokens` 8192 / 16384 / 32768**: watch time-to-first-token on long prompts against decode speed under parallel load.
4. **`max-num-seqs` 8 / 16 / 32** together with the harness's `maxConcurrency`: the point where p95 latency starts to climb is your ceiling.
5. **Context length**: lower `--max-model-len` to the largest size the retrieval probe passes; a smaller window frees KV for concurrency.

`SUMMARY.md` ends with a "Suggested vLLM and harness changes" section that turns these measurements into concrete flag changes.

## Choosing the checkpoint on Blackwell

If the checkpoint you call "the Blackwell version" is an FP8 or NVFP4 build, use it as shipped and do not pass quantization flags; the probes then measure it as it will run. Compare against a BF16 build only if you suspect a quality regression (tool-call malformed rate and the eval pass rate are the tell).
