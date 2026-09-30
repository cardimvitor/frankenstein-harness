# Roadmap: what is left, and how to do it

Status date: 2026-09-30. This is the plan for everything not yet built or not yet proven. Each item has a size (S ≈ a day, M ≈ several days, L ≈ a week or more), an acceptance test, and the decision I need from you. Edit freely; I will follow the version you settle on.

## 0. Actions only you can take

| Action | Why | How |
|---|---|---|
| Rename the repo to `frankenstein-harness` (or create a new one) | The GitHub repo is `frankstein-harness`; the code, Cargo package and docs say `frankenstein-harness`. My GitHub integration cannot rename or create repositories (403). | GitHub → repo → Settings → General → Repository name. GitHub redirects the old URL, so nothing breaks. Then tell me and I run `git remote set-url origin https://github.com/cardimvitor/frankenstein-harness`. If you prefer a fresh repo: create it empty, give the session access, and I push the branch. |
| Run `scripts/vps-validate.sh` on the VPS | Nothing has been measured on the real Qwen 3.8 / Blackwell / vLLM setup. Every number below depends on it. | Paste `docs/VPS_VALIDATION_PROMPT.md` into an agent on the VPS, or run the script. Launch vLLM first with `deploy/vllm/serve.sh`. |
| Provide the real eval corpus | The pass-rate margin over plain Qwen Code is set from a baseline on real tasks. | Your recorded .NET/React/Angular tasks (see `docs/EVAL.md`), plus a SWE-bench Verified subset (item N). |
| Confirm the vLLM auth scheme with the service owner | Bearer is implemented; anything else (mTLS, OIDC) needs to know what the service will accept. | One line: which scheme, which header. |

## 1. Gate decisions that the first VPS run makes (no code needed)

1. **Orchestration: keep, tune, or switch off.** Run `fh eval --runner all --repeat 3`. It compares plain Qwen Code, `fh`, and `fh-single` (the same harness with worker fan-out disabled). Rule: fan-out stays on by default only if `fh` beats `fh-single` on pass rate and is not slower on multi-file tasks. Otherwise `maxConcurrency` defaults to 1 and workers become opt-in. Reason: Qwen Code's subagents are tuned by its authors on their own prompts; mine are unproven on this model, so the eval decides, not the design.
2. **MTP 3 vs 2**, **KV cache dtype**, **prefix caching**, **max-num-seqs**: read "Suggested vLLM and harness changes" in `SUMMARY.md`; A/B plan in `docs/BLACKWELL.md`.
3. **Sampling defaults**: the Qwen3-family values are placeholders. If tool-call repair rate is above 1% or acceptance is low, tune temperature/top-p per step type (tool turns vs planning vs review) and re-run.
4. **Reasoning on reviewers**: if structured output + thinking only works through the fallback, decide whether the reviewer keeps thinking (needs a newer vLLM) or runs thinking-off.

## 2. Feature plans

### A. Qwen-specific quality of the multi-agent path (M) — depends on VPS run
- **Goal:** worker output as good as a single, well-prompted Qwen agent.
- **Plan:** (1) tune the worker prompt and the subtask contract on real runs; (2) let a worker report "blocked by ownership" so the master re-plans instead of failing; (3) let the master spawn an extra worker for unowned failures (today it uses one fixer); (4) add a per-task token budget split across workers (the task-level budget exists).
- **Acceptance:** `fh` ≥ `fh-single` on the multi-file eval tasks, no more tokens per solved task than a fixed factor you choose.
- **Decision for you:** the acceptable cost factor (for example ≤ 1.5× tokens for a faster wall time).

### B. MCP client (L)
- **Goal:** use external tools through the Model Context Protocol, compatible with the config people already have.
- **Plan:** stdio and streamable-HTTP transports; config in `.fh/mcp.json` using the same `mcpServers` shape as Claude Code/Qwen Code so files can be reused; tools appear as `mcp__<server>__<tool>`; treated as mutating unless the server marks them read-only; same approval modes, output clipping and timeouts; secrets from the keychain, never from the file.
- **Acceptance:** a mock MCP server test (list tools, call, error, timeout, cancellation); permission tests (plan mode blocks a mutating MCP tool).
- **Decision for you:** which MCP servers you actually need first (for example a browser, a database, issue tracker).

### C. Hooks (M)
- **Plan:** events `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Stop`; command hooks receive JSON on stdin, exit code 2 blocks with the stderr as the reason; configured in user and project files; project hooks only run in a trusted workspace (explicit `fh trust`); per-hook timeout.
- **Acceptance:** tests for allow, block, timeout, and that an untrusted workspace never runs project hooks.

### D. LSP diagnostics as a verifier input (L)
- **Plan:** a new deterministic check "diagnostics": start the language server for the stack (TypeScript language server, pyright, Roslyn via `csharp-ls`), open the changed files, collect diagnostics, and count only diagnostics that are new relative to the base checkpoint. Build/test output stays the arbiter when no server is installed.
- **Acceptance:** fixture repos with an introduced type error in TS and Python are caught and an unchanged pre-existing error is not.
- **Decision for you:** priority order of stacks (I suggest TypeScript, then C#, then Python).

### E. Session resume (M)
- **Plan:** append-only JSONL transcript per session under the data dir (messages, plan, checkpoint id, verdicts); `fh resume [id]` and `/resume` in the TUI and web UI; on resume the checkpoint is verified to still match the working tree.
- **Acceptance:** kill the process mid-task, resume, and continue with the same plan.

### F. Windows support (L)
- **Plan:** (1) cross-platform test helpers instead of `sh`/`python3` assumptions so CI runs green on `windows-latest`; (2) job objects for process-tree kill and memory limits; (3) restricted-token write confinement modeled on dsh's Windows ACL sandbox (I read that design); (4) PowerShell quoting review for the shell tool; (5) Windows Credential Manager for the API key.
- **Acceptance:** CI green on all three OS; a .NET Framework solution builds and verifies through MSBuild on a Windows runner.
- **Decision for you:** whether .NET Framework 4.8 verification on Windows is a launch requirement (I assume yes).

### G. Native sandbox without bubblewrap (L)
- **Plan:** Linux Landlock + seccomp implemented in-process (no external binary), keeping bwrap as an option; verify the macOS Seatbelt profile on a macOS runner.
- **Acceptance:** tests that a confined command cannot write outside the workspace or read `~/.ssh`, on each OS in CI.

### H. Authentication beyond bearer (M)
- **Plan:** mTLS via a client identity file; OIDC/JWT with token refresh and per-user tokens; the client-identifier header already exists; keychain on Windows.
- **Acceptance:** a local TLS test server that requires a client certificate.
- **Decision for you:** which scheme the service owner will require.

### I. Embeddings for skill retrieval (M) — only if needed
- **Plan:** BM25 stays the default. Add an optional embedding lookup if the eval shows the gate missing relevant skills (measure "skill would have applied but was not selected"). Uses an embedding model served next to the main one or a small local model; the gate stays at zero extra LLM calls.
- **Acceptance:** recall improves on a labelled set of tasks without pushing p50 above 50 ms.

### J. Thin-skill research (M)
- **Plan:** a skill is "thin" when it was used at least three times and is short, or is quarantined after failures. A background read-only pass over the repo (README, docs, configs, neighbouring code) proposes an enriched version through the same data-only validator and version history. Works offline; no web.
- **Acceptance:** a thin project skill grows a verifiable convention from repo content and its A/B record improves or it is rolled back.

### K. Local telemetry, opt-in (S)
- **Plan:** JSONL of task outcomes and timings in the data dir, off by default, `fh stats` reads it; nothing leaves the machine.

### L. LLM-based context compaction (M)
- **Plan:** when deterministic pruning is not enough, summarize dropped steps with a thinking-off call and keep the summary in the stable-prefix-friendly position.
- **Acceptance:** a long synthetic session stays under budget and still completes a task that needs an early fact.

### M. Better deterministic arbiters and false-positive tracking (M)
- **Plan:** autodetect formatters/linters (`dotnet format --verify-no-changes`, ESLint/Prettier, ruff, `cargo fmt --check`); treat "user kept a rolled-back patch" and "user undid a passing patch" as runtime false-positive/negative signals stored in the task log and shown in `fh stats`.
- **Acceptance:** stats show the two signals; eval false-positive rate stays under 10%.

### N. Eval corpus adapters (M)
- **Plan:** a script that turns SWE-bench Verified instances into task directories (checkout at `base_commit`, apply the test patch, oracle = FAIL_TO_PASS + PASS_TO_PASS); a Terminal-Bench adapter for container-free tasks; `fh record` to snapshot a real task (before/after, prompt, oracle) from your own work.
- **Decision for you:** subset size and language mix for the first baseline.

### O. Version packs (S each)
- **Plan:** each pack is checked against official sources before merge (see `docs/VERSIONS.md`). Next: .NET 11 when it ships (RC1 exists, GA expected November 2026), then Vue, Java/Spring, Python/Django by your usage. The pack process is in `docs/VERSIONS.md`.

### P. Web UI depth (M)
- **Plan:** session list and resume (needs E), per-file diff view with syntax highlighting (no external scripts: the CSP forbids them, so a small built-in tokenizer), file viewer from the workspace tree, markdown renderer coverage (tables, links as plain text).
- **Acceptance:** Chromium smoke test extended; CSP and the no-`innerHTML` rule still enforced by tests.

### Q. Discovery still owed (S)
- **Plan:** read Codex CLI (Rust) for its sandbox and TUI patterns, and write `docs/STUDY.md` with keep/avoid notes; write the "what not to copy" note from Hermes. Both are read-only research that feeds F, G and the TUI.

### R. Release engineering (S)
- **Plan:** CI builds release binaries for Linux (glibc and musl), macOS (arm64, x64) and Windows, attaches them to GitHub Releases, and `cargo install --git` documented. Version from `Cargo.toml`, changelog from commits.

## 3. Suggested order

1. **Now:** rename the repo, run the VPS validation, launch vLLM with `deploy/vllm/serve.sh` (section 0).
2. **After the first run:** gate decisions (section 1), then A and the tuning they imply.
3. **Then, in this order:** M (arbiters), D (LSP), C (hooks), B (MCP), E (resume), N (corpus), P (web depth).
4. **Platform track, in parallel:** F (Windows), G (native sandbox), H (auth), R (releases), Q (study).
5. **Only if measurements ask for it:** I (embeddings), J (research), L (LLM compaction). K whenever convenient.

## 4. Questions for you to settle

1. Is .NET Framework 4.8 on Windows a launch requirement, or can Windows follow Linux/macOS?
2. Which MCP servers matter first?
3. Which language servers matter first (TypeScript, C#, Python)?
4. Acceptable token cost for multi-agent fan-out relative to a single agent?
5. Which auth scheme will the vLLM service require beyond bearer?
6. First eval baseline: how many tasks, and what mix of your own tasks vs SWE-bench?
7. Should project hooks and MCP servers require an explicit trust step per repository (my recommendation: yes)?
