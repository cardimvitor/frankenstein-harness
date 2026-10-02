# Fan-out (master and workers): how it works and when it stays on

Decision (2026-09-30): fan-out follows the rule below, with the numbers proposed in `docs/ROADMAP.md` section 5 (80% wall time, +5 points pass rate, 2x cost). It is on by default (`maxConcurrency` 3) and the eval decides whether it stays on.

## What it is
For a task the planner may declare **two or more subtasks that touch different files** (for example `server/**` and `client/**`). Each subtask goes to a worker agent and the workers run in parallel, under a governor that reads vLLM `/metrics` (KV-cache use, queue depth) and holds back new workers when the server is under pressure. Afterwards the normal verification runs on the combined result.

- **When it happens:** only when the declared file sets do not overlap. Dependent or overlapping subtasks run in later waves; a malformed plan (unknown dependency, cycle, no files, duplicate ids) falls back to fully serial execution. A task with one subtask, or none, runs as a single agent.
- **Ownership:** a worker can only write files matching its globs (enforced by the write tool). If it needs a change elsewhere it calls `request_edit`; after the wave the master routes each request to the worker that owns the file, or to an extra worker when nobody owns it (one round).
- **Fix rounds:** verification failures are routed to the worker that owns the failing file; failures in unowned files get one worker per directory.
- **Budget:** workers share 60% of the task token budget (`maxTaskTokens`); the rest stays for verification and fixes.
- **Nothing a worker says is shown to the user**; only the master's verified result is.

## What it buys and what it costs
| Buys | Costs |
|---|---|
| shorter wall time on multi-file work (workers overlap) | more requests and more prompt tokens overall |
| each worker reads a smaller context | more KV-cache room and queue load on the GPU (the governor throttles this) |
| | risk of inconsistent assumptions between workers, caught only by verification |

The GPU is yours, so the cost that matters is **GPU work and wall time**, not money. GPU work is *uncached prompt tokens + completion tokens*: cached prompt tokens are nearly free, and workers share the same stable system prompt, so most of their prompt is cached. When the server does not report cached tokens per request, the measured prefix-cache hit rate is used to estimate them.

## The rule
`fh eval --runner orch` (or `--runner all`) runs the same tasks with fan-out (`fh`) and without it (`fh-single`, `maxConcurrency` 1). Fan-out stays on only if **all** of these pass:

1. **Pass rate** with fan-out is not lower than a single agent's.
2. **It pays for itself:** wall time is at most **80%** of the single agent's, **or** the pass rate is at least **5 points** higher.
3. **Cost:** uncached-plus-completion tokens per solved task are at most **2x** the single agent's.

The eval report prints each line as PASS or FAIL with the numbers, and ends with either "keep fan-out on" or "set maxConcurrency to 1 (fan-out off)". The VPS summary (`scripts/vps-validate.sh`) grades the same rule. The constants are `FANOUT_TIME_FACTOR`, `FANOUT_RATE_MARGIN` and `FANOUT_COST_FACTOR` in `src/eval/runner.rs`.

## Consolidation mode (max parallel)
With `--consolidate` (or `"consolidate": true`) and `maxConcurrency` > 1 the fan-out above gains a single consolidating agent, and tasks that do not split are parallelised too:

- **The plan splits into disjoint parts:** the workers run as before, then ONE consolidating agent receives the task and every worker's report (files touched, edits they asked for in files they did not own), reviews `git diff`, reconciles shared types, imports and names, makes the requested cross-file edits, runs the project checks and finishes. The verification rounds then run as usual.
- **The plan does not split** (the common case for one issue): up to four scouts run in parallel in plan mode, so they cannot edit. Each takes one angle (Locate, Tests, Impact, Solution) and reports findings and a proposed solution in at most 350 words. The main agent receives all reports, picks the best-supported root cause and solution, implements one coherent change and verifies it. A scout that fails or runs out of budget is left out.

Scouts share the governor with workers (so they respect the `/metrics` backpressure) and a quarter of the task token budget. The result JSON reports `consolidation: {scouts, consolidated}`. Off by default; the rule above decides whether it earns its tokens.

## Turning it off
`"maxConcurrency": 1` in `.fh/config.json` (or the user config) disables worker fan-out; everything else is unchanged.

## What is not yet measured
The rule has been tested against a mock, not against Qwen on your GPU. The worker prompts (`worker_task` in `src/orchestrator/master.rs`) and the `request_edit` flow are unproven on the real model until the first VPS eval.
