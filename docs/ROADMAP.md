# Roadmap: what is left

Status date: 2026-09-30. The feature work that does not need the real model is done (see DESIGN.md section 19). What remains is either **blocked on the VPS**, **blocked on a platform I cannot run here**, or a **decision for you**. Edit freely.

## 1. Blocked on the VPS (the real Qwen 3.8 27B, MTP 3, RTX 6000 Blackwell, vLLM)

Nothing below can be settled by reading code; each needs `scripts/vps-validate.sh` and an eval run.

| Item | What to decide or tune | How |
|---|---|---|
| Fan-out on or off | keep worker fan-out only if `fh` beats `fh-single` on pass rate and is not slower on multi-file tasks | `fh eval --runner all --repeat 3` on your recorded tasks (`fh record`) plus a SWE-bench subset |
| Worker prompts | the `request_edit` flow and the worker contract are tested against a mock, not against Qwen's actual behaviour | read the eval transcripts, tune `worker_task` in `src/orchestrator/master.rs` |
| Token cost of fan-out | your acceptable factor (question 4 below) | compare `tokensIn/Out` per solved task, `fh` vs `fh-single` |
| MTP 3 vs 2, KV dtype, batched tokens, max-num-seqs | flags in `deploy/vllm/serve.sh` | "Suggested vLLM and harness changes" in `SUMMARY.md`; A/B plan in `docs/BLACKWELL.md` |
| Sampling per step type | defaults are Qwen3-family placeholders | tool-call repair rate above 1% or low MTP acceptance means tune temperature/top-p for tool turns vs planning vs review |
| Reviewer with thinking | works only through the fallback if vLLM applies guided decoding from the first token | the "structured output with thinking" probe |
| Verifier false positives | target under 10% | `verifierFalseNegative` and the fp line in the eval `summary.md`; `fh stats` shows the user-action signals over time |
| Compaction threshold | summaries trigger at 72% of the context window | check `context summarized` notices on long tasks; lower `contextWindow` if quality drops earlier |
| Embedding recall | worth enabling only if the eval shows relevant skills being missed | set `embeddingModel`, compare `embed_recall` activity against outcomes |

## 2. Blocked on a platform or resource I could not use

| Item | State | What is needed |
|---|---|---|
| Windows | compiles for `x86_64-pc-windows-gnu`; Credential Manager implemented but never run; CI runs the suite on `windows-latest` (non-blocking) | read the CI result; still missing: job objects (memory limits, guaranteed tree kill), a restricted-token sandbox, portable test helpers (tests use `sh`/`python3`) |
| macOS | CI runs the suite on `macos-latest`; Seatbelt profile never exercised | run `fh run ... --sandbox` on a Mac |
| `csharp-ls`, `gopls` diagnostics | wired, not run (only pyright and TypeScript 7 were tested against real servers) | a machine with .NET and Go projects |
| Release workflow | packaging script tested locally on the musl build; the workflow never ran | push a `v0.1.0` tag |
| Codex CLI / Hermes study | not done: GitHub API was unreachable (403 through the proxy) | see `docs/STUDY.md` |
| Terminal-Bench adapter | not built: its tasks need containers | decide whether you need it |
| MCP OAuth, resources, prompts | not built | say which servers you use (question 2) |

## 3. Small follow-ups I would do next

1. Windows: job objects for process-tree kill and memory limits; make the test helpers portable so the Windows job can become blocking.
2. `fh doctor` should report Landlock ABI, the language servers found, formatter availability, hooks and MCP servers loaded, and trust status.
3. An `fh mcp list` and `fh hooks list` for inspecting what is active.
4. Per-repo `.fh/config.json` schema documentation (the options are listed in README and SECURITY only in part).
5. Web UI: search in the file viewer, keyboard navigation in the tree.

## 4. Questions for you

1. Is .NET Framework 4.8 on Windows a launch requirement, or can Windows follow Linux/macOS?
2. Which MCP servers matter first?
3. Which language servers matter most (TypeScript and Python are verified; C# and Go are not)?
4. Acceptable token cost for multi-agent fan-out relative to a single agent (for example at most 1.5x tokens for a faster wall time)?
5. Which auth scheme will the vLLM service require? (bearer, mTLS and a token command are implemented)
6. First eval baseline: how many tasks, and what mix of your own tasks vs SWE-bench?
7. Project hooks, MCP servers and LSP commands already require `fh trust` per repository; confirm that is what you want.
