# fh adapters for Harbor and Pier

Run Frankenstein Harness (`fh`) on Harbor-format benchmarks (any Harbor dataset) and on Pier (DeepSWE v1.1). The full run plan, including the order (fh first, in both modes) and the per-benchmark caveats, is in `docs/VALIDACAO_JG_ENG_TESTS_V2.md`, section 8.

## Build a static Linux fh

```bash
rustup target add x86_64-unknown-linux-musl     # plus: apt-get install musl-tools
CC_x86_64_unknown_linux_musl=musl-gcc cargo build --release --target x86_64-unknown-linux-musl
# -> target/x86_64-unknown-linux-musl/release/fh (runs in any x86_64 Linux image)
```

## Harbor

```bash
export PYTHONPATH=$PWD/integrations/harbor
cat > host-gateway.yaml <<'Y'
services:
  main:
    extra_hosts: ["host.docker.internal:host-gateway"]
Y
harbor run -d <dataset>@<versão> \
  --agent fh_harbor.agent:FrankensteinHarness -m openai/frankenstein-v2 \
  --ak binary=target/x86_64-unknown-linux-musl/release/fh \
  --ak max_concurrency=1 --ak state_dir=$PWD/fh-state/dataset \
  --ae FH_ENDPOINT=http://host.docker.internal:8001/v1 \
  --extra-docker-compose host-gateway.yaml --allow-agent-host host.docker.internal -n 4
```

Agent kwargs (`--ak`), or the host variable in brackets:

| kwarg | meaning |
|---|---|
| `binary` [`FH_BINARY`] | local path of the static fh build, uploaded into each container |
| `endpoint` [`FH_ENDPOINT`] | OpenAI-compatible base URL reachable *from the container* |
| `model` [`FH_MODEL`] | served model name (default: the part of `-m` after `/`) |
| `api_key_env` [`FH_API_KEY_ENV`] | host variable holding the key (default `FH_API_KEY`) |
| `context_window` [`FH_CONTEXT_WINDOW`] | should equal vLLM `--max-model-len` |
| `max_concurrency` [`FH_MAX_CONCURRENCY`] | `1` = single agent; `16` = up to 16 workers |
| `memory` [`FH_MEMORY`] | `off` = no-memory mode: learned skills are still created and improved but never put in the prompt |
| `consolidate` [`FH_CONSOLIDATE`] | `true` = max-parallel mode (with `max_concurrency` > 1): read-only scouts or file-partitioned workers in parallel, then one agent consolidates all findings and solutions |
| `mode` | `run` (default, the full harness) or `direct` (`fh direct`: the model alone, no harness; the baseline) |
| `commit` | `true` to commit the verified result (graders that diff `HEAD`) |
| `state_dir` [`FH_STATE_DIR`] | host dir with fh's learned skills, shared by all trials (merged under a lock) |

Each trial writes `agent/fh-result.json` (fh `--json` output) and `agent/fh.log`. Tokens go into Harbor's agent result, and fh's verdict, rounds and workers go into `metadata.fh`. fh exit codes 1 (verification failed) and 3 (unverified) are task outcomes, so the adapter does not raise on them. The task's verifier decides.

## Pier (DeepSWE)

Pier bakes the agent into the image at build time and runs it with no internet. Two consequences:

- the binary comes from a URL: serve it, e.g. `python3 -m http.server 8090 --bind 172.17.0.1`, and pass `FH_BINARY_URL` (plus `FH_BINARY_SHA256`);
- at run time the only way out is Pier's Squid proxy, which allows ports **80 and 443 only**: expose the model endpoint on one of them.

```bash
pier run -p deep-swe/tasks \
  --agent-import-path fh_harbor.pier_agent:FrankensteinHarness -m openai/frankenstein-v2 \
  --ae FH_ENDPOINT=http://172.17.0.1/v1 --ae FH_BINARY_URL=http://172.17.0.1:8090/fh \
  --ak commit=true --ak max_concurrency=1 --ak state_dir=$PWD/fh-state/deepswe -n 4
```

## What was verified

These checks used the scripted mock model (`fh mock-server`), Docker, Harbor 0.23.0 and Pier from source:

- **Harbor:** a real trial scored reward 1.0 from the official verifier, and 3 concurrent trials scored 1.0 each, with the shared skill store merging all of them.
- **Pier:** the build-time install and the no-internet run worked, but the model call was refused by Squid because the endpoint was on a non-80/443 port. The full Pier path still has to be confirmed in a pilot on the real machine.

## Sharing learned skills safely

`state_dir` (Harbor and Pier adapters) already follows the safe pattern: every trial works on its own copy and the adapter merges it back under a file lock. For runs you orchestrate yourself (one container per exercise), use the same pattern:

```bash
python3 -m fh_harbor.sync snapshot --from fh-state/shared --to fh-state/tasks/ex1   # then run the task with FH_HOME=fh-state/tasks/ex1
python3 -m fh_harbor.sync merge --into fh-state/shared fh-state/tasks/ex1           # when it ends
```

Do not let several users or containers write one SQLite file: the first writer's file mode (0640 by default) locks the others out. If you must share a directory anyway, run fh with `FH_SHARED_STORE=1` (group-writable files and a setgid directory, local disk only).
