# Using LiteLLM in front of vLLM

**Chosen authentication (2026-09-30): LiteLLM.** vLLM listens only on the VPS itself (or a private address) with its own `--api-key`; people and tools reach it through the LiteLLM proxy, which issues per-user keys. `fh` needs nothing new: it sends the LiteLLM key as a bearer token.

Yes, it works. Verified 2026-09-30 with LiteLLM 1.103.1 (`litellm[proxy]`) in front of a capture server that speaks vLLM's OpenAI API (not a real vLLM). What was checked, through the proxy, with the exact request `fh` sends:

| What `fh` relies on | Through LiteLLM |
|---|---|
| `chat_template_kwargs: {enable_thinking: ...}` | passed to the backend unchanged |
| `top_k`, `temperature`, `top_p`, `max_tokens`, `stream_options.include_usage` | passed unchanged |
| `response_format` (json_schema, strict) | passed unchanged |
| `tools` and `tool_choice`, streamed tool-call deltas | passed and streamed back correctly |
| `reasoning_content` in the stream | preserved |
| usage with `prompt_tokens_details.cached_tokens` | preserved (the eval cost metric uses it) |
| bearer auth | the client's key is checked by LiteLLM; LiteLLM uses its own `api_key` toward vLLM |

## Config

```yaml
# litellm.yaml
model_list:
  - model_name: qwen                      # the name fh will use (FH_MODEL)
    litellm_params:
      model: hosted_vllm/<served-model-name>   # the provider prefix keeps vLLM-specific parameters
      api_base: http://127.0.0.1:8000/v1
      api_key: os.environ/VLLM_API_KEY         # the key vLLM was started with (--api-key)
      request_timeout: 900
      num_retries: 0                           # fh already retries; do not multiply retries
general_settings:
  master_key: os.environ/LITELLM_MASTER_KEY
litellm_settings:
  drop_params: false                      # keep top_k and chat_template_kwargs
```

```bash
litellm --config litellm.yaml --port 4000 --num_workers 4
export FH_ENDPOINT=http://<host>:4000/v1 FH_MODEL=qwen FH_API_KEY=<a LiteLLM key>
export FH_METRICS_URL=http://127.0.0.1:8000/metrics   # see below
```

## What changes

- **`/metrics` is not proxied.** The adaptive governor, the MTP acceptance and prefix-cache measurements, and `fh doctor` read vLLM's Prometheus endpoint. Point `FH_METRICS_URL` (or `metricsUrl` in `.fh/config.json`) at vLLM directly; through LiteLLM it answers 404. Without it the harness still works, but concurrency stops adapting to KV pressure.
- **Measure vLLM directly.** `fh validate-vllm` and the eval measure whatever is behind the endpoint. Run `scripts/vps-validate.sh` against vLLM itself for the tuning numbers, and use the LiteLLM endpoint for daily work. The proxy adds a little latency per request.
- **Throughput.** LiteLLM is a Python process. With several workers in parallel plus a reviewer, use `--num_workers` and watch its CPU; it should not be the bottleneck for a single GPU.
- **Per-user keys and budgets** need LiteLLM's database (`general_settings.database_url`) and virtual keys. `fh` just sends the key as a bearer token.
- Prefix caching, MTP and the KV cache are vLLM's business and are unaffected, as long as the stable system prompt reaches vLLM unchanged (LiteLLM does not rewrite messages).
- Not verified: behaviour under sustained load, LiteLLM's own rate limiting, and a real vLLM (only the request/stream shapes above).

## Setting it up as the authentication layer
1. Start vLLM bound to `127.0.0.1` with `--api-key $VLLM_API_KEY` (`deploy/vllm/serve.sh` already reads the key from the environment).
2. Run LiteLLM with the config above on a public or private address, behind TLS (Caddy or nginx) if it is reachable from the internet. Set a strong `LITELLM_MASTER_KEY`.
3. For per-user keys, budgets and revocation, give LiteLLM a database (`general_settings.database_url`) and create virtual keys with its key-management API or UI; the master key stays with you.
4. On each machine that runs `fh`: `FH_ENDPOINT=https://<litellm-host>/v1`, `FH_MODEL=qwen` (the LiteLLM model name), `FH_API_KEY=<the person's key>` or store it once with `fh auth set`.
5. Keep `FH_METRICS_URL` pointed at vLLM's `/metrics` for the machine that can reach it (or leave it unset; the governor then does not adapt).

Rotating or revoking a key is done in LiteLLM; vLLM's key never leaves the server.
