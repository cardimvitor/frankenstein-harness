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

## Suggested vLLM launch shape (verify against your version's docs)

```bash
vllm serve <model> \
  --served-model-name <name> \
  --max-model-len 131072 \
  --gpu-memory-utilization 0.90 \
  --enable-prefix-caching \
  --reasoning-parser qwen3 \
  --enable-auto-tool-choice --tool-call-parser qwen3_coder \
  --speculative-config '{"method":"mtp","num_speculative_tokens":3}' \
  --api-key "$VLLM_API_KEY"
```

Flag names change between vLLM releases; the harness never edits your service, it only reports what it measured.
