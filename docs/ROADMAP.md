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
2. (done) `fh doctor` reports Landlock ABI, trust, hooks, language servers and MCP tools.
3. An `fh mcp list` and `fh hooks list` for inspecting what is active.
4. Per-repo `.fh/config.json` schema documentation (the options are listed in README and SECURITY only in part).
5. Web UI: search in the file viewer, keyboard navigation in the tree.

## 4. Decisions so far (your answers, 2026-09-30)

| Question | Answer | Consequence |
|---|---|---|
| .NET Framework 4.8 on Windows a launch requirement? | **Yes** | Windows moves from "later" to launch-blocking: classic projects are now built with MSBuild and tested with vstest.console (located through vswhere; **written, not yet run on Windows**). Still needed before launch: run it on a real Windows host with a .NET Framework solution, job objects, a Windows sandbox (or a documented VM requirement), portable integration tests so the Windows CI job can block. |
| MCP servers | **All** | The client is generic, so any stdio or streamable-HTTP server works. Remote servers that need OAuth (GitHub, Sentry, Linear, Atlassian, ...) do **not** work yet: OAuth 2.1 with PKCE and dynamic client registration is the missing piece. Ranked first-tier list to test: filesystem, git, fetch, GitHub, Playwright, a SQL server, docs (Context7), memory. |
| Language servers | **C#, TypeScript, JavaScript, Python, Rust** | Python (pyright), TypeScript/JavaScript (TypeScript 7 native server, or typescript-language-server for older projects) and Rust (rust-analyzer) are tested against the real servers. C# (`csharp-ls`) is wired but unrun, and classic .NET Framework projects will need MSBuild on the machine. |
| Token cost of fan-out | open: see 5 | |
| Auth beyond bearer | open: see 6 | |
| First eval baseline | your one repository later, plus famous benchmarks | Adapters exist for the Aider polyglot benchmark (all six languages verified) and SWE-bench Verified; see 7. |
| Trust step per repository | **Yes** | already how it works (`fh trust`) |

## 5. Thinking about fan-out cost

Your model runs on your own GPU, so a token has no invoice: what fan-out spends is **wall time, GPU queue and KV-cache room**, and it earns **speed and isolation**. Count what actually costs GPU work: *uncached prompt tokens + completion tokens* (a cached prompt token is nearly free because prefix caching reuses it). Workers share the stable system prompt, so most of their prompt tokens are cached; a worker's real cost is its own context plus its output.

A rule you can adopt and the eval will check:

1. **Quality first**: fan-out must not lower the pass rate versus `fh-single` on the multi-file tasks.
2. **Then it must pay for itself**: wall time at most 0.8x of single-agent, *or* pass rate at least 5 points higher.
3. **Cost ceiling**: uncached-plus-completion tokens per solved task at most 2x single-agent (1.5x if you share the GPU with other work).
4. **Stay narrow**: fan out only when the planner declares two or more subtasks with disjoint files (already so); everything else runs as one agent.

The harness already caps a task (`maxTaskTokens`, workers get 60% of it). What is missing is the measurement: the eval report should show *uncached tokens per solved task* and wall time per runner, so the rule above is a computed pass/fail line rather than a judgement. That is a small change I can make next.

## 6. Auth options beyond bearer (for a vLLM on a VPS)

| Option | Good when | Effort | State |
|---|---|---|---|
| **Private network (Tailscale or WireGuard) + bearer key** | one person or a small team; you do not want vLLM on the public internet | lowest | works today: bind vLLM to the tailnet address, keep `--api-key` |
| **SSH tunnel + bearer** | one person, occasional use | very low | works today (`ssh -L 8000:127.0.0.1:8000`) |
| **Reverse proxy with TLS + mTLS** (Caddy or nginx) | you must expose it publicly and want device-bound access | medium (certificates) | client side implemented (`clientCert`, `clientKey`, `caCert`) |
| **Reverse proxy with OIDC/JWT** (Cloudflare Access, oauth2-proxy, Keycloak) | several users, revocation, audit | medium-high | client side works through `authTokenCmd` (any CLI that prints a fresh token, refreshed after a 401) |
| **API gateway with per-user keys** (LiteLLM, Envoy) | usage limits and per-person accounting | medium | works as bearer; nothing to build |

Recommendation for a single RTX 6000 VPS used by you: **Tailscale + the vLLM `--api-key`**. It is the smallest attack surface for the least work, and the harness needs no change. Choose mTLS only if the port must be public.

## 7. Benchmarks, in the order I would run them

| Benchmark | What it tells you | Runnable here | Notes |
|---|---|---|---|
| **Aider polyglot** (225 Exercism tasks: C++, Go, Java, JavaScript, Python, Rust) | multi-language editing quality; the most-quoted agent number for open models | **yes**: `scripts/polyglot_to_tasks.py --verify`, all six languages verified | start here; fast, hundreds of tasks, no containers |
| **SWE-bench Verified** (500 real GitHub issues) | real repository bug fixing | adapter exists; needs each repo's dependencies | the official runs use one Docker image per instance. Planned fix: mount the static musl `fh` binary into those images and use their own test scripts as the oracle |
| **Terminal-Bench** | terminal and environment tasks | no adapter | needs Docker on the VPS |
| **Your repository** | what you actually care about, including .NET | `fh record` | send it when ready; also gives the C#/.NET signal no public benchmark provides |

Suggested first baseline: about 60 polyglot tasks (10 per language) with `--repeat 3`, plus 20 to 30 SWE-bench Verified instances from one or two repositories, plus your own tasks. Run `fh`, `fh-single` and plain Qwen Code on the same set, and let the rule in section 5 decide.

## 8. Still open

1. Approve the fan-out rule in section 5 (or change the numbers).
2. Pick the auth option in section 6.
3. Say whether OAuth for remote MCP servers is worth building now, or whether local stdio servers cover you for the launch.
4. Send the test repository whenever it is ready.
