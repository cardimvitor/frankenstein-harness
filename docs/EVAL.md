# Eval harness

`fh eval` runs a corpus of tasks against the harness and, optionally, against plain Qwen Code on the same vLLM, then reports the numbers that the success criteria in [DESIGN.md](DESIGN.md) use.

```bash
fh eval --runner fh            # Frankenstein Harness only
fh eval --runner qwen          # plain Qwen Code baseline (needs `qwen` on PATH)
fh eval --runner both --repeat 3 --tasks eval/tasks --out reports   # fh vs plain Qwen Code
fh eval --runner orch                                                # fh vs fh-single (worker fan-out off): does orchestration help?
fh eval --runner all --repeat 3                                      # all three
```

## What is measured per run

| Field | Meaning |
|---|---|
| `solved` | the task's **oracle** command exits 0 on the final working tree (the oracle is never shown to the agent) |
| `seconds` | wall time including planning, verification rounds and rollback |
| `verdict`, `rounds` | the harness's own verdict and number of verification rounds |
| `toolCalls`, `repaired`, `malformed`, `thinkLeaks` | tool-call quality as seen by the client (repaired = server output needed repair) |
| `tokensIn`, `tokensOut`, `requests` | cost of the run (autonomous mode cost = tokens across all rounds) |
| `acceptance`, `prefixHit` | MTP acceptance and prefix-cache hit rate during the run (from `/metrics`) |
| `verifierFalseNegative` | verdict `pass` while the oracle fails |
| `gateMs` | skill gate time; the gate makes no LLM call |

Verifier false positives (verdict `fail` while the oracle passes the final tree) and the reviewer's dropped/uncheckable findings are reported in `summary.md`.

## Built-in smoke corpus

Small dependency-free tasks (`src/eval/corpus.rs`), Python-based so a bare VPS with `python3` can run them: an off-by-one, a stub to implement, an order-losing bug, a whitespace bug and a two-module task that exercises the parallel workers. Three JavaScript variants are added when `node` is installed. Each fails before the change and passes after (a unit test enforces that). It is a smoke test, not a benchmark.

## Your own tasks, SWE-bench and Terminal-Bench

Point `--tasks` at a directory. Each task is a folder:

```
my-tasks/
  fix-invoice-rounding/
    task.json        {"id": "...", "prompt": "...", "oracle": "dotnet test", "timeoutS": 900, "tags": ["dotnet"]}
    repo/            the starting state of the repository
```

- **Recorded tasks from your own work** (.NET, React, Angular): snapshot the repo before the change into `repo/`, put the prompt you used in `task.json`, and use the tests (or a script) that proved the change as the `oracle`.
- **SWE-bench Verified subset**: for each instance, check out `base_commit` into `repo/`, apply the instance's `test_patch` to `repo/`, and use the `FAIL_TO_PASS` and `PASS_TO_PASS` tests as the oracle command. Build a subset that fits your language and time budget.
- **Terminal-Bench**: wrap each task's container-free steps as `repo/` plus an oracle script; tasks needing their own containers are out of scope for this runner.

The margin by which fh must beat plain Qwen Code is set from the baseline run on your corpus (phase P0 in the roadmap), not guessed in advance.

## Baseline fairness

`--runner qwen` calls `qwen -y -p "<prompt>"` with `OPENAI_BASE_URL`, `OPENAI_MODEL` and `OPENAI_API_KEY` set from the same configuration, in a fresh copy of the same repository. Check that the installed Qwen Code version accepts those variables and flags (`qwen --help`), and record the version with the results.
