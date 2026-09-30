# Roadmap: what is left

Status date: 2026-09-30 (fourth pass). The feature work that does not need the real model is done (DESIGN.md sections 19 and 20). What remains is **blocked on the VPS**, **blocked on a platform I could not verify from here**, or a **decision for you**. Edit freely.

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

## 2. Blocked on a platform or resource I could not verify

| Item | State | What is needed |
|---|---|---|
| **Windows (launch requirement)** | Written and cross-compiled: job objects (process-tree kill, optional memory cap), PowerShell 7 preferred, `python`/`py -3` instead of the Store stub, PowerShell-safe quoting, Credential Manager, MSBuild + vstest verification of classic .NET Framework 4.8 solutions, and a real-Windows test suite (`tests/windows.rs`, including a full msbuild/vstest run of a fixture solution). **None of it has been run on Windows yet**: the GitHub Actions runs I could observe started failing instantly (looks like a billing or minutes limit; the job log is empty) and this integration may not dispatch workflows. | Run `cargo test --test windows` on a Windows machine (Visual Studio Build Tools + .NET Framework 4.8 targeting pack), or fix the Actions quota and push a commit containing `[ci-full]`. Still not built: a Windows **filesystem sandbox** (the Codex design is dedicated low-privilege accounts + ACLs set up with elevation; see `docs/STUDY.md`). Until then `--sandbox` on Windows is process containment only; use a VM for untrusted repositories. |
| macOS | CI ran the full suite green earlier; Seatbelt profile never exercised | run `fh run ... --sandbox` on a Mac |
| `csharp-ls`, `gopls` diagnostics | wired, not run (pyright, TypeScript 7 and rust-analyzer were tested against the real servers) | a machine with .NET and Go projects |
| Release workflow | packaging script tested locally on the musl build; the workflow never ran | push a `v0.1.0` tag |
| Terminal-Bench adapter | not built: its tasks need containers | Docker on the VPS |
| SWE-bench without Docker | adapter needs each repo's dependencies installed | plan: mount the static musl `fh` into the official per-instance images |

## 3. Small follow-ups I would do next

1. Skill lifecycle from Hermes: stale (14 days) and archived (30 days) by inactivity, and a `pin` flag; deterministic, never deletes. (not started; you asked to stay with what we have)
2. (done) Cross-session search: `fh search`, `/search`, web "Past tasks" search.
3. Run the miner/IMPROVE/research calls with the stable prompt prefix so they reuse vLLM's prefix cache (measure on the VPS first).
4. Windows filesystem sandbox (accounts + ACLs, or documented VM requirement). (The Windows installer is done: scripts/install.ps1, tested with real PowerShell 7 on Linux; not yet run on Windows.)
5. MCP: resources and prompts, OAuth for servers without dynamic client registration (pre-registered client id).
6. (done) Web UI file search with jump-to-line. Still open: keyboard navigation in the tree.

## 4. Decisions so far (your answers, 2026-09-30)

| Question | Answer | Consequence |
|---|---|---|
| .NET Framework 4.8 on Windows a launch requirement? | **Yes** | Windows moves from "later" to launch-blocking: classic projects are now built with MSBuild and tested with vstest.console (located through vswhere; **written, not yet run on Windows**). Still needed before launch: run it on a real Windows host with a .NET Framework solution, job objects, a Windows sandbox (or a documented VM requirement), portable integration tests so the Windows CI job can block. |
| MCP servers | **All** | Any stdio or streamable-HTTP server works, and remote servers that need OAuth now work too: `fh mcp login <server>` runs OAuth 2.1 (discovery, dynamic client registration, PKCE S256, resource indicators) and tokens refresh automatically (tested against a mock authorization server; **not** yet against GitHub, Sentry or Linear themselves). Servers without dynamic client registration are not supported yet. First-tier list to try: filesystem, git, fetch, GitHub, Playwright, a SQL server, docs (Context7), memory. |
| Language servers | **C#, TypeScript, JavaScript, Python, Rust** | Python (pyright), TypeScript/JavaScript (TypeScript 7 native server, or typescript-language-server for older projects) and Rust (rust-analyzer) are tested against the real servers, and servers are now kept warm between rounds. C# (`csharp-ls`) is wired but unrun, and classic .NET Framework projects need MSBuild on the machine. |
| Token cost of fan-out | **the rule in section 5, numbers as proposed** (80% time or +5 points, 2x cost); written up in docs/FANOUT.md | the eval report now prints uncached tokens per solved task, wall time per solved task, and a PASS/FAIL line for each part of the rule |
| Auth beyond bearer | **LiteLLM in front of vLLM** (per-user keys) | documented in docs/LITELLM.md; nothing to build. mTLS and the token command stay available | LiteLLM in front of vLLM is verified for the request shapes `fh` sends (docs/LITELLM.md); it fits the "gateway with per-user keys" row |
| First eval baseline | your one repository later, plus famous benchmarks | Adapters exist for the Aider polyglot benchmark (all six languages verified) and SWE-bench Verified; see 7. |
| Trust step per repository | **Yes** | already how it works (`fh trust`) |

## 5. Thinking about fan-out cost

Your model runs on your own GPU, so a token has no invoice: what fan-out spends is **wall time, GPU queue and KV-cache room**, and it earns **speed and isolation**. Count what actually costs GPU work: *uncached prompt tokens + completion tokens* (a cached prompt token is nearly free because prefix caching reuses it). Workers share the stable system prompt, so most of their prompt tokens are cached; a worker's real cost is its own context plus its output.

A rule you can adopt and the eval will check:

1. **Quality first**: fan-out must not lower the pass rate versus `fh-single` on the multi-file tasks.
2. **Then it must pay for itself**: wall time at most 0.8x of single-agent, *or* pass rate at least 5 points higher.
3. **Cost ceiling**: uncached-plus-completion tokens per solved task at most 2x single-agent (1.5x if you share the GPU with other work).
4. **Stay narrow**: fan out only when the planner declares two or more subtasks with disjoint files (already so); everything else runs as one agent.

The harness already caps a task (`maxTaskTokens`, workers get 60% of it). The measurement is built: `fh eval` and the VPS summary compute *uncached tokens per solved task* and wall time per runner and grade the rule above as PASS/FAIL lines (when the server does not report cached tokens, the measured prefix-cache hit rate is used to estimate them).

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

1. (settled) fan-out rule: docs/FANOUT.md. (settled) auth: LiteLLM.
3. Try `fh mcp login` against one real remote server you use (GitHub, Sentry, ...) and tell me which one fails.
4. Send the test repository whenever it is ready.
