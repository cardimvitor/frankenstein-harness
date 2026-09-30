# One prompt to validate everything on the VPS

Paste the prompt below into a coding agent (for example Claude Code) running **on the VPS that hosts your vLLM**, or run `scripts/vps-validate.sh` yourself: the prompt just drives that script and interprets the result.

Before pasting, fill the three values in the first block. Do not paste the API key into the prompt; export it in the shell that starts the agent (`export FH_API_KEY=...`), so it never enters the conversation.

---

```text
You are validating Frankenstein Harness ("fh") on the VPS that hosts my vLLM. Do the whole job end to end and report at the end. Work autonomously; only stop to ask me if something below says to.

SETTINGS
- Repo: https://github.com/cardimvitor/frankstein-harness  branch: ccr-3bc51f62-prkdq0
- vLLM endpoint (OpenAI-compatible, includes /v1): http://127.0.0.1:8000/v1
- Served model name: <FILL: exact id from `curl -s $FH_ENDPOINT/models`>
- The API key, if any, is already exported as FH_API_KEY in this shell. Never print it, never write it to a file, never include it in your reply. If the endpoint answers 401, tell me instead of guessing credentials.

WHAT TO DO
1. Get the code: clone the repo (use the git credentials already on this machine; do not ask me for tokens) into ~/frankstein-harness and check out the branch. If a clone exists, `git pull` instead.
2. Prerequisites: need Node >= 22.18 and git. If Node is missing or older, install Node 22 LTS or newer the normal way for this OS (nvm/fnm or the distro/NodeSource package). Do not touch vLLM, its service, GPU drivers or its flags.
3. Set FH_ENDPOINT and FH_MODEL to the settings above (export them), then run from the repo root:
       scripts/vps-validate.sh --repeat 3
   This runs, in order: typecheck and the harness self-tests; `fh doctor`; the vLLM measurements (tool-call reliability, streamed vs non-streamed tool calls, thinking leakage, MTP acceptance per draft position, prefix-cache behaviour, long-context retrieval, concurrency/KV pressure, cancellation, error handling); an eval of fh against plain Qwen Code (`@qwen-code/qwen-code`, installed globally by the script) on the same model; a live web UI security smoke test; and it writes reports/vps-<timestamp>/SUMMARY.md.
   It can take a long time (the long-context and concurrency probes are heavy). Let it finish. If it is killed or times out, re-run with `--quick`.
4. If a step fails for an environment reason (missing tool, port in use, npm registry unreachable), fix that reason and re-run the script. If it fails because of the harness code (a failing test, a crash, a bad assumption about vLLM output), do NOT hide it: reproduce it, find the root cause, and either fix it in the repo (small, minimal changes, add or adjust a test, rerun `npm test` and `npx tsc -p tsconfig.json`) or report it precisely. Never skip, delete or weaken a test to get green.
5. Read reports/vps-*/SUMMARY.md, the vLLM report (report.md) and the eval summary. Do not invent numbers; quote the reports.

WHAT TO TELL ME (final reply, short)
A. Verdict table for the success criteria, copied from SUMMARY.md: malformed tool calls < 1%; pass rate vs plain Qwen Code; median time no worse; verifier false positives < 10%; skill gate adds no LLM call; web UI security checklist.
B. The vLLM findings that matter, with the numbers: MTP acceptance overall and per draft position (structured vs prose), prefix-cache hit rate with a stable prefix vs variable-first, tool-call repair/malformed/leak rates, whether streamed and non-streamed tool calls match, long-context retrieval up to which size, the recommended maxConcurrency, and whether cancellation released the request.
C. Anything that failed or looked wrong, with the likely cause. For vLLM-side problems (for example thinking not returned as reasoning_content, tool calls left in content, prefix caching off, no /metrics) list the vLLM flag or setting I should change, but DO NOT change my vLLM configuration yourself.
D. If you changed any code in the repo, list the commits. Commit to the branch but do not push unless I say so.
E. Where the artifacts are (path only). Do not paste logs unless a step failed.

RULES
- No secrets in output, logs, commits or reports. If you see one in a log, redact it before quoting.
- Do not run anything destructive outside ~/frankstein-harness and the temp directories the tests create.
- The eval creates and edits temporary repos only; it does not touch other directories.
- Be honest about what was not measured (for example only the built-in smoke corpus ran, not SWE-bench or Terminal-Bench).
```

---

## What "both tests" covers

| Test | Where it runs | What it proves |
|---|---|---|
| Harness self-tests | `npm test` (mock vLLM, no model) | loop, tools, verification rounds, rollback, skill store and gate, orchestration, web UI security controls |
| vLLM validation | `fh validate-vllm` | your real server and model behave the way the harness assumes (tool-call parser, reasoning parser, MTP, prefix cache, long context, concurrency, abort) |
| Eval | `fh eval --runner both` | fh vs plain Qwen Code on the same model: pass rate, time, malformed calls, verifier accuracy |
| Web smoke | `fh serve` + curl | loopback bind, Host/Origin checks, cookie auth, CSP on the real process |

## Running it by hand

```bash
export FH_ENDPOINT=http://127.0.0.1:8000/v1
export FH_MODEL=<served model name>
export FH_API_KEY=<token>            # omit if the endpoint is open
scripts/vps-validate.sh --repeat 3   # add --quick for a shorter pass, --no-baseline to skip plain Qwen Code
```

Use the real comparison corpus by pointing `--tasks` at a directory of tasks (see [EVAL.md](EVAL.md)).
