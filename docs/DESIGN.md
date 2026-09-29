# Frankenstein Harness: planning and discovery

Status: draft for review. Phase produces research and design only; no harness code yet.

## 1. Goal and scope

A harness combining DeepSeek Harness (dsh: TypeScript, Cordis plugins, dev preview) and Qwen Code (QwenLM/qwen-code, Apache 2.0, TypeScript, Gemini CLI fork), running on one model: Qwen3.8 27B on the user's vLLM with MTP=3.

- Targets: CLI/TUI and a local browser chat/coding UI.
- Priorities: as fast as possible, code-only, Qwen-specific tuning.
- Separate repo; the Organizer (Flutter) app is untouched.
- Platforms: Windows, Linux, macOS from P1 (Windows is required for .NET Framework / MSBuild).

## 2. License

- Own code: MIT.
- Code copied from Qwen Code stays Apache 2.0 (Gemini CLI heritage): keep LICENSE, NOTICE and file headers. A TypeScript fork is therefore mixed MIT + Apache 2.0, not pure MIT. A Rust port of designs (no copied code) can be pure MIT.
- To verify first: dsh license (reported MIT), repo URL and stability.
- Skill packs built from official docs: check content licenses (many are CC-BY-4.0, which requires attribution).
- Apple HIG: check terms of use; summarize principles, do not copy text.

## 3. Verified vs unverified inputs

Verified (Qwen Code GitHub README): Plan Mode; Skills (built-in /review, /batch, /loop, /bugfix; `.qwen/skills/<name>/SKILL.md`); Subagents; Agent Teams; Dynamic Workflows; Hooks; MCP; LSP; Auto-Memory; Auto-Skills; Auto Mode; Sandbox; Git Worktrees; providers including vLLM (auth via /auth).

Not confirmed: approval-mode names; detailed behavior of skills, subagents, hooks and auto-skills. Action: read the repo's `docs/` after cloning (docs site is blocked in this environment).

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

- None blocking. Confirm dsh license/repo and the vLLM auth scheme with the service owner.
