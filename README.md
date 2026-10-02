# Frankenstein Harness

A native coding-agent harness written in **Rust** with three front ends: a full-screen **terminal UI**, a plain **CLI**, and a **local browser UI**,, built for one model: **Qwen on your own vLLM** with MTP speculative decoding in mind. It combines ideas from Qwen Code and DeepSeek Harness (dsh) into a single self-contained binary with no Node, Python or other runtime on the machine that runs it.

What makes it different from a plain agent loop:

- **Verify before output.** Work is provisional. Deterministic checks (build/type-check, tests, lint, diff scope, secret scan, "no deleted or skipped tests", conflict markers) decide; an LLM reviewer must cite `file:line` plus a verbatim quote or its finding is dropped. A failed verification rolls the tree back and keeps the patch under `.fh/rejected/`. With no runnable checks the result says `UNVERIFIED`.
- **Intake funnel.** Inspects the repo, asks at most five targeted questions, enriches the task with acceptance criteria, always shows a plan (a compact one for trivial tasks). `--auto` skips questions and plan and runs up to five verification rounds with different review checklists.
- **Hidden skill system.** A deterministic gate (repo fingerprint + BM25, no LLM call) picks guidance from immutable built-in packs (senior personas, .NET Framework 4.8, .NET 8-11, React 18/19, Angular 17+, AngularJS 1.x, Vue 2/3, Spring Boot, Django, an Apple-HIG-inspired UI baseline) and from a per-user SQLite store of learned skills. Learned skills are project, stack or global scoped, validated as data-only, versioned, rolled back or quarantined automatically when they hurt verification results, and never shown to the user beyond activity notices. Your own `AGENTS.md` and `SKILL.md` files always win.
- **Master/worker orchestration.** Independent subtasks with disjoint file ownership run in parallel under a governor that reads vLLM `/metrics` (KV-cache usage, queue depth), so parallel long contexts do not evict each other's prefix cache.
- **Extensibility.** Hooks (`.fh/hooks.json`: SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop), an MCP client (`.fh/mcp.json`, stdio and streamable HTTP), LSP diagnostics as a verifier input (only errors *new* relative to the base count), formatter checks on changed files, and a native Linux sandbox (Landlock + seccomp, no external binary). Project-level hooks, MCP servers and LSP commands only load after `fh trust`.
- **Speed.** Language servers stay warm between rounds (about 0.3 s per check instead of 2.4 s), checks and the reviewer run in parallel with build and tests, several edits per file go in one call, and syntax errors come back in the same turn.
- **Qwen/vLLM specifics.** Stable prompt prefix for prefix caching, edit-by-search/replace instead of file rewrites, thinking budget per step (`enable_thinking` on for planning, recovery and review, off for routine tool turns), streamed tool-call repair, `<tool_call>` recovery from content, think-leak detection, loop detection, structured-output fallback when guided decoding conflicts with the reasoning parser, retry/backoff, end-to-end cancellation (abort the request, kill the process tree).

Status: implemented and covered by 120 automated tests against a mock vLLM server (plus real pyright, TypeScript 7 language server, `openssl` mTLS and Landlock kernel tests), a real-browser check of the web UI and a pseudo-terminal check of the TUI. **Not yet measured against your real vLLM**: launch vLLM with [`deploy/vllm/serve.sh`](deploy/vllm/serve.sh) (tuned profiles for an RTX 6000 Blackwell + Qwen 27B with MTP=3, see [docs/BLACKWELL.md](docs/BLACKWELL.md)), then run `scripts/vps-validate.sh` ([docs/VPS_VALIDATION_PROMPT.md](docs/VPS_VALIDATION_PROMPT.md)). What is left to build is planned in [docs/ROADMAP.md](docs/ROADMAP.md).

## Build and run

```bash
# Rust 1.86+ and a C compiler (build-essential) are needed to build; nothing is needed to run the binary.
git clone https://github.com/cardimvitor/frankstein-harness && cd frankstein-harness
cargo build --release              # ~2 minutes; produces target/release/fh

export FH_ENDPOINT=http://127.0.0.1:8000/v1
export FH_MODEL=<served model name>
export FH_API_KEY=<token>          # omit for an open endpoint (or FH_AUTH_SCHEME=none)

./target/release/fh doctor         # endpoint, model, auth, /metrics, sandbox
./target/release/fh run "fix the failing test in lib/math.js"
./target/release/fh run "add pagination to the orders endpoint" --auto --sandbox
./target/release/fh                # full-screen terminal UI (Tab cycles approvals, Ctrl+A autonomous, F1 keys)
./target/release/fh serve          # local web UI on 127.0.0.1:7878 (one-time code printed in the terminal)
```

**Installers** (attached to each release): Windows, no administrator needed, installs to `%LOCALAPPDATA%\Programs\fh`, verifies the SHA-256 and adds it to your user PATH:

```powershell
iwr -useb https://raw.githubusercontent.com/cardimvitor/frankenstein-harness/main/scripts/install.ps1 | iex
.\install.ps1 -Uninstall      # to remove it
```

Linux and macOS: `curl -fsSL https://raw.githubusercontent.com/cardimvitor/frankenstein-harness/main/scripts/install.sh | bash` (installs to `~/.local/bin`). Both accept a version (`-Version v0.1.0` / `VERSION=v0.1.0`) and a local folder (`-Source` / `SOURCE=`) for offline installs. They cannot work until the first release is published (push a `v*` tag).

Or install it somewhere on your `PATH` (`cargo install --path .` or copy the binary). Run `fh --help` for every option.

vLLM flags this harness assumes (yours; the harness never changes them):

```
--speculative-config '{"method":"mtp","num_speculative_tokens":3}'
--reasoning-parser qwen3 --enable-auto-tool-choice --tool-call-parser qwen3_coder --enable-prefix-caching
```

## Configuration

Precedence: defaults, then `~/.config/frankenstein-harness/config.json` (or the OS equivalent), then `<repo>/.fh/config.json`, then environment.

```json
{
  "endpoint": "http://127.0.0.1:8000/v1",
  "model": "qwen",
  "authScheme": "bearer",
  "apiKeyEnv": "FH_API_KEY",
  "extraHeaders": { "x-team": "core" },
  "contextWindow": 131072,
  "maxConcurrency": 3,
  "verifyRoundsNormal": 2,
  "verifyRoundsAuto": 5
}
```

Override what gets verified with `.fh/verify.json`: `[{"name":"build","cmd":"dotnet build","kind":"build"},{"name":"test","cmd":"dotnet test","kind":"test"}]` (`kind`: build, types, lint, test). When the repository has `scripts/test.sh` (`.ps1` on Windows), its own `scripts/{build,typecheck,lint,test}` are used instead of autodetection. Autodetected otherwise: npm/pnpm/yarn scripts, .NET solutions/projects (+ tests), pytest or unittest, `go`, `cargo`.

## Commands

| Command | What it does |
|---|---|
| `fh` / `fh tui` | full-screen terminal UI: streaming chat, plan/question/approval cards, verification and files panels, skill notices, mode switches; `fh --plain` or `fh chat` for a line-based chat |
| `fh run "<task>"` | one task; `--auto`, `--mode plan\|ask\|auto-edit\|yolo`, `--yes`, `--plan-only`, `--commit`, `--keep`, `--sandbox`, `--json` |
| `fh direct "<task>"` | the model alone, no harness: one request with a repository snapshot, SEARCH/REPLACE edits applied as written; no tools, loop, verification or skills. The baseline other harnesses are measured against |
| `fh serve` | local web UI: chat, streamed thinking, plan approval and questions, diffs, command output, skill notices, verification status, mode switch |
| `fh doctor` | connectivity and configuration check |
| `fh validate-vllm` | measure MTP acceptance, prefix cache, tool-call reliability, long context, concurrency, cancellation |
| `fh eval` | run the eval corpus, optionally against plain Qwen Code ([docs/EVAL.md](docs/EVAL.md)) |
| `fh undo` / `fh keep` | restore the working tree to the last checkpoint / apply the last rejected patch anyway (both recorded as verifier signals in `fh stats`) |
| `fh search <words> [--all]` | search past task sessions (what was asked, the plan, what was delivered, files touched); also `/search` in the TUI and a "Past tasks" search in the web UI, which also searches workspace files and jumps to the line |
| `fh sessions`, `fh resume [id]` | list task sessions; continue an interrupted one with its stored plan and checkpoint (also `/resume` in the TUI, a button in the web UI) |
| `fh mcp list\|login <server>\|logout <server>` | MCP servers; `login` signs in to remote servers that need OAuth |
| `fh trust [--revoke\|--list]` | allow project hooks, MCP servers and LSP commands in this repository |
| `fh record <id> ...` | snapshot your own work as an eval task; the oracle must fail on the base and pass with the solution |
| `fh activity`, `fh history [skill]` | skill activity notices; version history with hashes and diffs of learned skills |
| `fh stats` | runtime statistics: verdicts, rounds, tokens per task, reviewer quality |
| `fh auth set\|clear\|status` | keep the API key in the OS keychain (Linux `secret-tool`, macOS Keychain) instead of an env var |

## Modes for ablations

- **`--no-memory`** (or `FH_MEMORY=off`, config `"memory": "off"`): fh still creates, improves and enriches skills after every verified task, but learned skills are never put in the prompt (builtin framework packs stay). Each result lists the learned skills it withheld (`skillsWithheld`). Comparing it with the default isolates what using the memory buys.
- **`--consolidate`** (or `FH_CONSOLIDATE=1`, config `"consolidate": true`, needs `maxConcurrency` > 1): maximum parallelism inside a task with one agent that consolidates everything. When the work splits into file-disjoint parts, workers run in parallel and the consolidating agent reviews and reconciles the combined result. When it does not (a single issue), up to four read-only scouts (plan mode: they cannot edit) investigate different angles in parallel (locate, tests, impact, solution) and return findings and a proposed solution; one agent then consolidates the reports, implements one coherent change and verifies it. See [docs/FANOUT.md](docs/FANOUT.md).
- **`FH_SHARED_STORE=1`**: several users share one skill store (group-writable setgid directory; local disk only). Prefer one private store per task merged back by one owner (`python3 -m fh_harbor.sync`, [integrations/harbor](integrations/harbor/README.md)). A store that cannot be opened never stops a task: fh uses a private copy and says why.

## Platforms

Linux is exercised locally and in CI; macOS in CI only. Windows compiles and has a Credential Manager path for the API key, but is untested beyond CI, has no sandbox, and the test suite still shells out to POSIX `sh`. See [docs/SECURITY.md](docs/SECURITY.md) and [docs/ROADMAP.md](docs/ROADMAP.md).

Prebuilt binaries for six targets are produced by `.github/workflows/release.yml` when a `v*` tag is pushed; or `cargo install --git https://github.com/cardimvitor/frankstein-harness`.

## Development

```bash
cargo test                          # mock vLLM server, no model needed
python3 scripts/tui-smoke.py        # drives the real TUI in a pseudo-terminal (after `cargo build --release`)
# real-browser check of the web UI (needs Playwright + Chromium, and `cargo build --release`):
PLAYWRIGHT_MODULE=$(npm root -g)/playwright node scripts/ui-smoke.mjs /tmp/fh-ui
```

`fh mock-server --port N` starts the scripted mock vLLM used by the tests, handy for dry-running `scripts/vps-validate.sh` without a GPU. The original TypeScript prototype that the Rust port was checked against lives in git history (commit `d7cc73e`).

## Docs

- [docs/DESIGN.md](docs/DESIGN.md): design, discovery findings, decisions and implementation status
- [docs/VPS_VALIDATION_PROMPT.md](docs/VPS_VALIDATION_PROMPT.md): the one prompt to validate everything on the VPS
- [docs/ROADMAP.md](docs/ROADMAP.md): what is left (VPS-blocked tuning, platform gaps, questions for you)
- [docs/STUDY.md](docs/STUDY.md): the Codex/Hermes study (not done, and why)
- [docs/FANOUT.md](docs/FANOUT.md): how master/worker fan-out works and the rule that keeps it on
- [docs/LITELLM.md](docs/LITELLM.md): LiteLLM in front of vLLM (verified request shapes, what changes)
- [docs/BLACKWELL.md](docs/BLACKWELL.md), [docs/VERSIONS.md](docs/VERSIONS.md), [docs/EVAL.md](docs/EVAL.md), [docs/SECURITY.md](docs/SECURITY.md)

## License

MIT. This is a clean-room implementation: Qwen Code (Apache 2.0) and dsh (MIT) were read as design references only, and no code was copied from them.
