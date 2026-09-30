# Frankenstein Harness: planning and discovery

Status: design plus a working implementation (section 17). The vLLM-dependent parts are implemented but not yet measured on the real server.

## 1. Goal and scope

A harness combining DeepSeek Harness (dsh: TypeScript, Cordis plugins, dev preview) and Qwen Code (QwenLM/qwen-code, Apache 2.0, TypeScript, Gemini CLI fork), running on one model: Qwen3.8 27B on the user's vLLM with MTP=3.

- Targets: CLI/TUI and a local browser chat/coding UI.
- Priorities: as fast as possible, code-only, Qwen-specific tuning.
- Separate repo; the Organizer (Flutter) app is untouched.
- Platforms: Windows, Linux, macOS from P1 (Windows is required for .NET Framework / MSBuild).

## 2. License

- Own code: MIT.
- Code copied from Qwen Code stays Apache 2.0 (Gemini CLI heritage): keep LICENSE, NOTICE and file headers. A TypeScript fork is therefore mixed MIT + Apache 2.0, not pure MIT. A Rust port of designs (no copied code) can be pure MIT.
- dsh verified (section 16): MIT, `deepseek-ai/deepseek-harness`, developer preview 0.2.0-rc.2 with announced compatibility-breaking changes. Its vendored Cordis packages are MIT too.
- Qwen Code verified (section 16): Apache 2.0 `LICENSE`, files carry `SPDX-License-Identifier: Apache-2.0` headers, no separate NOTICE file at the repo root.
- Skill packs built from official docs: check content licenses (many are CC-BY-4.0, which requires attribution).
- Apple HIG: check terms of use; summarize principles, do not copy text.

## 3. Verified vs unverified inputs

Verified (Qwen Code GitHub README): Plan Mode; Skills (built-in /review, /batch, /loop, /bugfix; `.qwen/skills/<name>/SKILL.md`); Subagents; Agent Teams; Dynamic Workflows; Hooks; MCP; LSP; Auto-Memory; Auto-Skills; Auto Mode; Sandbox; Git Worktrees; providers including vLLM (auth via /auth).

Previously unconfirmed items (approval-mode names, skills, subagents, hooks, auto-skills) were read from the cloned repos and are recorded in section 16. Sources read: Qwen Code 0.24.7 and dsh 0.2.0-rc.2, shallow clones as of 2026-09-29.

## 4. Core behaviors

### 4.1 Verify before output

Stream progress and thinking live, but gate the final answer, file writes and commits on the verdict. Clarifying questions are not gated.

Verification runs in rounds; deterministic checks decide.

- Deterministic arbiters: build/type-check, tests, lint/format, LSP diagnostics (Roslyn, tsserver), diff scope (only intended files), patch applies cleanly, secret scan, no deleted/skipped tests.
- LLM reviewers: fixed output schema, must cite file:line; findings that cannot be checked are dropped.
- Each round runs all checks in parallel and exits early on pass.
- Normal mode: at least 1 round. Auto mode: up to 5 rounds with different checklists.
- No tests available: result is marked "unverified", never silently passed.
- Track the verifier false-positive rate (false positives cause churn).

### 4.2 Intake funnel

Inspect the repo first; ask targeted questions (cap 3-5 per round); enrich the prompt with constraints and acceptance criteria; always show a plan. Trivial tasks get a compact plan approved with one key. Auto mode: no plan or questions; internal clarification plus up to 5 verification rounds.

### 4.3 Personas and version packs

- Base packs: backend, frontend, SQL, security, DevOps, testing, architecture, performance, plus a router.
- Version packs: .NET Framework through .NET 10 (add .NET 11 when released, expected Nov 2026), React, Angular including AngularJS 1.x.
- Build order by usage: .NET Framework 4.8, .NET 8/10 LTS, React 18/19, Angular 17+, then the long tail.

## 5. Skill system

### 5.1 Skill gate (every task, cheap by default)

1. Deterministic match first: repo fingerprint (TargetFramework, package.json React/Angular versions, lockfiles) plus BM25/embedding lookup in the skill index.
2. The LLM is involved only when the match is ambiguous, and only inside the planner's structured output, never as an extra turn.
3. Decision per task: USE / IMPROVE / CREATE. IMPROVE and CREATE run asynchronously after delivery, rate-limited, off the critical path. Thin skills trigger research.

Target: under 50 ms p50 overhead; zero extra LLM calls when a skill matches.

### 5.2 Storage

- Immutable built-in layer shipped with the binary, plus a writable per-user, per-project store (SQLite in the OS user data dir: XDG on Linux, `~/Library/Application Support` on macOS, `%LOCALAPPDATA%` on Windows), never in the repo. Shared by all internal agents.
- One user-level index spans all projects so cross-project matching works.
- Internal skills are hidden: no browse/edit, but activity notices (created/improved/used, with reason) in CLI and web UI, plus a history log with content hashes and diffs.
- User-added skills and instructions use standard `AGENTS.md` / `SKILL.md` files, visible and editable. User rules override internal skills on conflict.

### 5.3 Scope classification

Each skill has a scope:

- `project`: tied to one repo's conventions (folder layout, naming, custom build scripts).
- `stack`: tied to a technology/version (e.g. .NET 8 EF Core, Angular 17 signals).
- `global`: general senior-specialist practice.

The miner decides scope from evidence: a pattern in one repo only is project; the same pattern across repos sharing a stack fingerprint is stack; stack-independent is global. Retrieval ranks project > stack > global. Project skills never leak into unrelated repos.

### 5.4 Cross-project reuse (ask first)

On entering a new or unfamiliar repo, the gate looks for project-scoped skills from the user's other projects with a similar fingerprint. If found, ask once (e.g. "Project X has 6 skills for .NET 8 + Angular 17 conventions; reuse them here?") with choices reuse all / pick / no. The question shows only names and one-line summaries, never content. Accepted skills are copied (not linked) into the new project's store and tagged with origin. Auto mode: no question; only stack/global skills are reused.

### 5.5 Safety and quality

- Poisoning protection: every skill is tagged with source; repo-derived skills stay project-scoped and are promoted to global only after N verified successes; skills are data only (no permissions, commands, URLs or secrets; secret scan on write); version history with rollback and quarantine.
- Skills are fully hidden with no debug commands, so the harness polices quality itself: auto-quarantine or roll back a skill when tasks using it fail verification more often than without it (internal A/B tracking).

## 6. Multi-agent orchestration

- One master agent plans, splits, dispatches narrow-scoped workers, consolidates, runs verification on the merged result, and accepts, re-verifies, sends work back, or spawns more agents; bounded by round and budget limits. Nothing from a worker reaches the user directly.
- Fan out only when files do not overlap; otherwise serial. Prefer file ownership in one tree over per-agent worktrees (worktrees need their own node_modules / NuGet restore).
- Adaptive concurrency governor reading vLLM `/metrics` (KV-cache usage, `num_requests_waiting`): too many parallel long contexts evict each other's prefix cache and slow the user down.
- Build on Qwen Code's Subagents / Agent Teams / Dynamic Workflows where possible.

## 7. Speed design (MTP=3, Qwen-specific)

- Prompt layout for vLLM prefix caching: stable prefix (system prompt, tool schemas) first; gate-selected skills and task content after it.
- Structured, predictable output feeds MTP well: tool calls, diff/search-replace edits (never whole-file rewrites), short fixed-schema verdicts.
- Small tool set with short schemas, per-session file cache, parallel tool calls, streaming.
- Thinking budget by step: off for routing and trivial edits; high for planning, hard debugging, final verification.
- Qwen layer: tuned prompts and tool descriptions, sampling defaults, malformed tool-call repair, loop detection.

### Serving (user's vLLM)

```
--speculative-config '{"method":"mtp","num_speculative_tokens":3}'
--reasoning-parser qwen3 --enable-auto-tool-choice --tool-call-parser qwen3_coder
```

Measure ourselves, don't trust secondhand figures: MTP acceptance per draft position, prefix-cache hit rate, malformed tool-call rate, thinking leaking into tool args, streaming tool calls under MTP, long-context degradation, cost of 5-round auto mode.

## 8. Authentication toward vLLM

Bearer token baseline. Config = endpoint + model + auth scheme + extra headers. Secret comes from an env var or the OS keychain, never from the repo, skills, logs or browser. Later: mTLS, OIDC/JWT, per-user tokens, client-identifier header. Confirm the scheme with the service owner.

## 9. UI design baseline: Apple HIG

The harness's own UI (web, and TUI where it applies) follows Apple's Human Interface Guidelines as a design reference (clarity, hierarchy, consistency, accessibility, light/dark, layout, typography, spacing, motion), not a native look on web/Windows/Linux.

For UI code generated in user projects, in order:

1. Project has its own design system/guideline or clear UI patterns: follow the project (detected by repo scan; stored as a project skill).
2. Otherwise, new project, or the user asks for something different: use the HIG-based default.

Implemented as a built-in global skill "HIG UI baseline" plus an intake question when the signal is unclear (auto mode follows the order without asking).

## 10. Local web UI security

- Bind to 127.0.0.1; validate Host and Origin headers (DNS rebinding, WebSocket CSRF).
- No token in URL: one-time code exchanged for an HttpOnly, SameSite=Strict cookie.
- Strict CSP, no CORS, sanitized markdown rendering (XSS from repo or model content would run with shell access).

## 11. Language decision

Model inference dominates task time, so Rust's startup and memory gains barely affect task speed. OS sandboxing (Landlock/seccomp, Seatbelt; Windows job objects/AppContainer) is available to both languages.

Decide by measurement: Phase 0 builds the eval harness and baselines plain qwen-code on the vLLM. Then a time-boxed Rust spike (~2 weeks) with kill criteria written in advance: tool-call handling parity, per-turn harness overhead, estimated effort to feature parity, and Windows/Linux/macOS support. Choose Rust only if it wins; otherwise fork Qwen Code (TS) and add dsh's web UI and plugin ideas.

## 12. Discovery work (read-only)

- dsh source: plugins, services, agent loop, approval policy, prompt assembly, skills, sessions, sandbox, web UI.
- Qwen Code source: ReAct loop, tool schemas, Qwen tool-call handling, Plan Mode, Skills/Auto-Skills, Subagents/Teams/Workflows, Hooks, Auto Mode, MCP, LSP.
- Codex CLI (Rust): loop, sandbox, TUI.
- Hermes: only what not to copy.
- Model/serving measurements (section 7).
- Skill formats: SKILL.md / AGENTS.md compatibility; internal store and index design; repo fingerprint for scope and cross-project matching.
- Version sources and current latest versions for .NET, React, Angular, plus content licenses.
- Web UI spec: chat, streaming/thinking, plan approval and funnel questions, diffs/file tree, command output, skill activity notices, mode switch, verification status.
- Also study: context management and compaction; approval/permission model and sandbox; checkpoints and undo (git snapshot per turn); end-to-end cancellation (abort vLLM requests, kill child processes, cancel subagents); vLLM down/offline behavior (retry/backoff, skill research without internet); session resume; opt-in local-only telemetry; prompt injection via tool output and repo content; MCP compatibility.
- Eval corpus: SWE-bench Verified subset, Terminal-Bench, recorded .NET/React/Angular tasks from the user's own work.
- Deliverable: feature matrix (keep / port / rewrite / drop).

## 13. Roadmap

| Phase | Scope |
|---|---|
| P0 | Eval harness, baselines (malformed calls, MTP acceptance, prefix-cache hits), Rust spike and language decision |
| P1 | Core loop, tools, deterministic verifier, CLI |
| P2 | Intake funnel, plan mode, auto mode, LLM reviewers |
| P3 | Skill gate, scope classification, cross-project reuse prompt, user SKILL.md/AGENTS.md, hidden store, activity log, HIG UI baseline skill |
| P4 | Master/worker orchestration with concurrency governor |
| P5 | Web UI plus security review |
| P6 | Version packs in usage order, then long tail |

## 14. Success criteria (vs plain qwen-code on the same model)

- Higher pass rate by a margin set in P0; median task time no worse.
- Malformed tool calls under 1%; verifier false positives under 10%.
- Skill gate adds no LLM call when a skill matches.
- Zero open findings on the web UI security checklist.

## 15. Open questions

- Confirm the vLLM auth scheme with the service owner.
- Base decided: native hard fork of Qwen Code, Qwen-only (section 16.4). Confirm the fork name, license header policy and which Qwen Code packages to keep in the first pass.

## 16. Discovery findings (read from source, 2026-09-29)

### 16.1 Qwen Code 0.24.7 (Apache 2.0, pnpm monorepo, ~10k files, very active)

- Packages: `core`, `cli`, `web-shell` (React browser terminal UI over a daemon, `qwen serve`), `sdk-typescript/python/java`, `acp-bridge`, `channels`, `vscode-ide-companion`, `desktop`, and more.
- Approval modes (five): Plan (read-only), Ask Permissions (config value `default`), Auto-Edit, Auto (default out of the box; two-stage LLM classifier: fast `shouldBlock`, then a thinking stage only on block; uses the configured fast model), YOLO. Shift+Tab cycles plan, default, auto-edit, auto, yolo. Hard `permissions.deny/ask` rules win over the classifier; writes to its own config/skills/hooks paths and `.git/` always go through the classifier.
- Hooks: 22 events, including `PreToolUse`, `PostToolUse`, `PostToolBatch`, `UserPromptSubmit`, `Stop`, `StopFailure`, `SubagentStart/Stop`, `PreCompact/PostCompact`, `PermissionRequest/Denied`. Types: command, HTTP, function, prompt. A `Stop` hook is a natural place for a verify-before-output gate.
- Skills: `SKILL.md` folders, model-invoked or `/name`; `/learn` creates `.qwen/skills/learned-skill-<name>/`. Auto-skills carry `source: auto-skill` frontmatter and live in the project's `.qwen/skills/`. A deterministic Auto-Skill Curator (design doc 2026-07-27) marks skills stale at 30 days and archives at 90 days, without an LLM. This differs from our design: their skills are visible and in-repo, ours are hidden and outside the repo, and their lifecycle uses age, not verification outcomes.
- Subagents, Agent Teams, Dynamic Workflows, background agents, worktrees, `/batch`, `/loop`, goals, scheduled tasks, LSP, MCP, memory with recall and microcompaction, tool-output offload, file-history snapshots (undo).
- External subagent executors `claude-code` and `codex`; native Windows launches of these are rejected (macOS/Linux/WSL only).
- Sandbox: Linux bwrap (or Landlock fallback with open network only), macOS Seatbelt, Docker/Podman. No native Windows process sandbox is documented, and ACP, `qwen serve` and web terminals reject the execution sandbox. This is a gap against our all-three-platforms goal.
- Model layer: OpenAI-compatible provider is used for vLLM (`baseUrl`, `envKey`, `samplingParams`, `extra_body`, `streamIdleTimeoutMs`). `streamingToolCallParser.ts` repairs malformed streamed tool-call JSON. Also present: `prefix-caching.ts`, `taggedThinkingParser.ts`, per-provider quirks, loop detection (`StopFailure` type `loop_detected`), adaptive tool-call cap, reasoning-effort overrides. Whether these give Qwen3 `qwen3_coder` output good results under MTP is a P0 measurement.

### 16.2 dsh 0.2.0-rc.2 (MIT, developer preview)

- Everything is a Cordis plugin: model adapter, tool registry, session log and agent loop are replaceable from configuration. Profiles (`web`, `headless`, `sdk`, `sdk-minimal`, `acp`) are composed from bundles plus YAML patches (`--dump-config` prints the tree).
- Packages include `core` (session, agent-loop, tools, system-prompt), `llm` (`llm-pi-ai` routes OpenAI-compatible or self-hosted endpoints with `compat` switches; `llm-retry`), `compaction`, `guard` (repeat-tool-reminder), `sandbox` (local, policy, **windows-acl** restricted-token backend), `skill`, `subagent` (acp, claude-code, codex, sdk), `workflow`, `goal`, `lsp`, `mcp`, `hooks`, `web`, `apps/{cli,web,desktop}`, Python and TS SDKs.
- Web UI starts on `127.0.0.1:3080` via `npx @deepseek-ai/dsh web`. `SAFETY.md` states it is unaudited and must not be the only security control.
- Vendors Cordis source under MIT; runtime is Node/pnpm like Qwen Code. Compatibility-breaking changes are expected during preview.

### 16.3 Implications for the plan

1. Both bases are TypeScript monorepos with a browser UI already. The larger cost is not features but tracking two fast-moving upstreams. A fork of Qwen Code inherits roughly 10k files of surface we do not need (channels, mobile, desktop, omni/media, IM integrations).
2. Several planned features already exist upstream and should be reused, not rebuilt: Plan Mode, five approval modes with a classifier, hooks, subagents/workflows, worktrees, undo snapshots, MCP/LSP, tool-call repair, prefix-caching helper, web-shell.
3. Genuinely new work is smaller than the plan implied: verification gate with deterministic arbiters and rounds, intake funnel, the hidden outcome-scored skill store with scope classification and cross-project reuse, master/worker governor tied to vLLM `/metrics`, HIG UI baseline, version packs, and the eval harness.
4. Windows: dsh's `sandbox-windows-acl` is the only Windows confinement found; it is a candidate to port or reference.
5. Auto Mode exists in Qwen Code as a permission classifier. Our "auto mode" is a different thing (autonomous run plus up to 5 verification rounds). Name ours differently in UI (for example "Autonomous") to avoid confusing users of both.

### 16.4 Base options and recommendation

| Option | What | For | Against |
|---|---|---|---|
| A | Extension layer on Qwen Code: hooks (`Stop`, `PostToolBatch`), skills, SDK, MCP server, no fork | Cheapest; tracks upstream; ships in days; tests the verification gate on real tasks | Hidden store and funnel may not fit hook surface; upstream owns UI |
| B | Fork Qwen Code | Full control of loop, UI, skill store | Merge burden on a daily-moving codebase; large surface |
| C | Fork dsh | Plugin architecture makes swapping loop/skills/tools clean; MIT only | Preview, breaking changes, no Qwen-specific tool-call handling, DeepSeek-oriented |
| D | Rust rewrite | Startup, memory, single binary | Model time dominates; highest effort |

Decision (user, 2026-09-29): Frankenstein Harness ships as a native, Qwen-only product, not as an extension. Option A is therefore not a delivery form. It survives only as a throwaway P0 measurement rig if it saves time, and nothing from it is a product dependency.

Recommended base: option B, a hard fork of Qwen Code (Apache 2.0 headers and NOTICE kept, our own name and MIT for new code), with dsh contributing ideas only (Windows ACL sandbox, profile composition, web app patterns), not code, unless a specific piece is worth vendoring under its MIT license.

Fork rules that keep the "best of both worlds" real and measurable:

- Remove what a Qwen-only coding harness does not need: other-provider content generators (Anthropic, Gemini, per-vendor quirks other than Qwen/vLLM), IM channels, mobile, desktop, omni/media, browser/computer use, external Claude Code/Codex executors.
- Specialize the model layer for Qwen3.8 on vLLM: one provider path, one tool-call dialect (`qwen3_coder`), tuned prompts and sampling, MTP-aware output shaping (section 7).
- Treat upstream as a source of ideas, not a merge target: record the fork commit, review upstream releases periodically, and cherry-pick fixes by hand. Do not promise to track upstream.
- Skills, verification and orchestration are first-class core code, not hooks. The hidden skill store and gate run inside the loop, so the gate adds no extra turn and no user-visible surface.
- Acceptance is the P0 eval harness: each core change must beat or match plain Qwen Code on pass rate, median task time, malformed-call rate and false positives (section 14). A change that does not is reverted.
- Rust stays a time-boxed spike after the fork baseline exists; it wins only if measured harness overhead or Windows/sandbox needs justify it.

### 16.5 Feature matrix (first pass)

| Feature | Source | Decision |
|---|---|---|
| Streaming tool-call parser with repair | Qwen Code | keep, measure on Qwen3.8 |
| Plan Mode, approval modes | Qwen Code | keep; add compact one-key plan for trivial tasks |
| Auto Mode classifier | Qwen Code | keep as permission layer |
| Hooks (22 events) | Qwen Code | keep; primary extension seam for option A |
| Auto-skills + curator | Qwen Code | rewrite: hidden store, scope classification, outcome-scored quarantine |
| SKILL.md / AGENTS.md user files | Qwen Code, dsh | keep, visible and editable |
| Subagents, teams, workflows | Qwen Code | keep; add file-ownership scheduler and vLLM-aware governor |
| Worktrees | Qwen Code | keep optional; default to file ownership in one tree |
| Sandbox Linux/macOS | Qwen Code | keep |
| Sandbox Windows | dsh `sandbox-windows-acl` | port or reference |
| Web UI | Qwen Code `web-shell`, dsh `apps/web` | evaluate both in P0; restyle to HIG; apply section 10 checklist |
| Plugin/profile composition (Cordis) | dsh | port idea only if option C is chosen |
| Undo snapshots | Qwen Code file-history | keep; add per-turn git snapshot |
| IM channels, mobile, desktop, omni media, browser/computer use | Qwen Code | drop |
| Verification gate, intake funnel, HIG skill, version packs, eval harness | new | build |

### 16.6 Still to read (next discovery pass)

Qwen Code: `prompts.ts` and tool descriptions, compaction service, `permissions/`, `daemon`/web-shell auth model. dsh: `system-prompt`, `approval`, `skill` internals, web app security posture (Host/Origin checks). Codex CLI (Rust) sandbox and TUI; Hermes anti-patterns; version sources and licenses for .NET/React/Angular docs; measurements on the user's vLLM (need endpoint access, which this cloud session does not have).

## 17. Implementation status (2026-09-30)

### 17.1 Decision update: clean-room native build, not a fork

Section 16.4 recommended a hard fork of Qwen Code. During implementation this was replaced by a **clean-room native implementation** in a small TypeScript codebase (Node >= 22.18, no runtime dependencies, no build step):

- A fork means ~10k files (channels, mobile, desktop, media, IM integrations) that could not be stripped and verified in one pass, and a daily-moving upstream.
- The design here (verification gates, hidden outcome-scored skills, file-ownership orchestration) lives in the core loop either way, so nothing was lost by not inheriting Qwen Code's loop.
- Qwen Code and dsh were read as design references only; **no code was copied**, so the project is pure MIT and there is no Apache 2.0 NOTICE obligation.
- Trade-off accepted: Qwen Code features that were not rebuilt (LSP, MCP, hooks, Windows sandbox, undo UI beyond `fh undo`, IDE integrations) are absent. See 17.3.

### 17.2 What is implemented and where

| Design item | Code | Verified by |
|---|---|---|
| vLLM client: streaming, reasoning_content, streamed tool-call assembly, repair of truncated/fenced/think-leaked JSON, `<tool_call>` recovery, retry/backoff, abort, per-step thinking toggle, Qwen sampling defaults | `src/llm/` | `test/llm.test.ts` |
| `/metrics` parsing: KV usage, queue depth, prefix-cache hits, MTP acceptance overall and per position | `src/llm/metrics.ts` | `test/llm.test.ts` |
| Tools: read, list, grep, search/replace edit (exact then whitespace-tolerant), create-only write, bash (timeout, cancel kills the process tree, scrubbed env) | `src/tools/`, `src/util/proc.ts` | `test/agent.test.ts` |
| Permission modes plan/ask/auto-edit/yolo with a hard deny list; workspace and symlink confinement; file ownership | `src/agent/permissions.ts`, `src/util/paths.ts` | `test/agent.test.ts` |
| Agent loop: parallel read-only calls, loop detection, deterministic context compaction, stable prompt prefix, per-step thinking budget | `src/agent/` | `test/agent.test.ts`, `test/engine.test.ts` |
| Verification rounds: build/test/lint/type-check autodetect (.NET, npm/pnpm/yarn, pytest, go, cargo, `.fh/verify.json`), diff scope, secret scan, deleted/skipped tests, conflict markers; LLM reviewer with mandatory verifiable `file:line` + quote; "unverified" when nothing runnable; rollback on fail with rejected patch saved | `src/verify/`, `src/session/checkpoint.ts` | `test/verify.test.ts`, `test/engine.test.ts` |
| Repo fingerprint: .NET Framework/.NET versions, React/Angular/AngularJS versions, Python/Go/Rust, project id, similarity | `src/fingerprint.ts` | `test/verify.test.ts` |
| Intake funnel: repo inspection, <= 5 questions, enriched prompt, acceptance criteria, plan (one-key for trivial), auto mode without questions, plan feedback | `src/funnel/intake.ts`, `src/engine.ts`, `src/ui/term.ts` | `test/engine.test.ts` |
| Skill gate: fingerprint + BM25, zero LLM calls, p50 well under 50 ms, ambiguity resolved inside the planner output | `src/skills/gate.ts` | `test/skills.test.ts` |
| Skill store: SQLite in the OS user data dir, immutable built-ins, versions with diffs, rollback, quarantine, activity log, scope classification and promotion, data-only validation, cross-project reuse offer (names and summaries only, copy with origin, none in auto mode), A/B quality police | `src/skills/` | `test/skills.test.ts` |
| Built-in packs: 8 senior personas, .NET Framework 4.8, .NET 8-10, React 18/19, Angular 17+, AngularJS 1.x, HIG UI baseline | `src/skills/builtin/` | `test/skills.test.ts` |
| User `AGENTS.md`, `FRANKENSTEIN.md`, `.fh/skills/*/SKILL.md` (plus read-only `.qwen`, `.claude`, `.agents` skills) override internal skills | `src/skills/usercfg.ts` | `test/skills.test.ts` |
| Master/worker: planner-declared subtasks, disjoint-ownership waves, dependency ordering, serial fallback, per-worker ownership enforced by the write tool, failures routed to the owning worker, governor from vLLM metrics | `src/orchestrator/`, `src/engine.ts` | `test/orchestrator.test.ts`, `test/engine.test.ts` |
| Post-delivery skill mining (rate-limited, verified-pass only), outcome tracking, quarantine and promotion passes | `src/skills/miner.ts`, `src/engine.ts` | `test/engine.test.ts` |
| Local web UI: 127.0.0.1, Host/Origin checks, one-time code to HttpOnly SameSite=Strict cookie, strict CSP, no innerHTML, redaction of secrets including across stream chunks; chat, streamed thinking, plan and question cards, tool output, diffs, skill notices, verification status, mode switch | `src/ui/server.ts`, `src/ui/web/` | `test/web.test.ts`, live smoke in `scripts/vps-validate.sh` |
| Eval harness: built-in smoke corpus, task directories, fh and plain Qwen Code runners, pass/time/malformed/verifier metrics | `src/eval/` | `test/engine.test.ts` |
| vLLM validator: connectivity/auth, metrics availability, speed, tool-call reliability, stream vs non-stream equality at temperature 0, think leakage, structured output with thinking, MTP acceptance structured vs prose and per position, prefix cache stable vs variable-first, long-context needle, concurrency and KV calibration, cancellation, error handling | `src/validate/vllm.ts` | `test/validate.test.ts` |
| One-command VPS validation and summary against the success criteria | `scripts/vps-validate.sh`, `scripts/summarize.ts`, `docs/VPS_VALIDATION_PROMPT.md` | dry-run against a mock server |

Test status: 57 automated tests (mock vLLM, no model needed), typecheck clean.

### 17.3 Known gaps (not implemented or not verified)

- **Nothing has been run against the real Qwen3.8 27B / vLLM MTP=3 setup.** Sampling defaults, tool-call behaviour under MTP, acceptance rates, prefix-cache effectiveness and the concurrency limit are placeholders until `scripts/vps-validate.sh` runs on the VPS.
- Windows: PowerShell shell tool and `taskkill` tree-kill are coded but untested; no Windows sandbox. macOS Seatbelt and Linux bubblewrap are implemented but `--sandbox` was not exercised (no bwrap in the build environment).
- Not built: MCP client, LSP diagnostics as a verifier input (compiler/test output is used instead), hooks, session resume, LLM-based context summarization (deterministic pruning only), opt-in telemetry, mTLS/OIDC auth, .NET 11 pack (not released), and version packs beyond the seven listed.
- Reviewer false-positive rate can only be measured with an oracle (the eval does this); the built-in corpus is five smoke tasks, not SWE-bench or Terminal-Bench.
- The web UI has not been driven in a real browser in this environment; its security controls are tested at the HTTP level and the client code avoids `innerHTML` by construction.
- Rust spike: not started; the measurements from the VPS run decide whether it is worth doing.
