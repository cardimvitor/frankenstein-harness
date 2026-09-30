# Frankenstein Harness

A native coding-agent harness (CLI and local browser UI) written in **Rust**, built for one model: **Qwen on your own vLLM** with MTP speculative decoding in mind. It combines ideas from Qwen Code and DeepSeek Harness (dsh) into a single self-contained binary with no Node, Python or other runtime on the machine that runs it.

What makes it different from a plain agent loop:

- **Verify before output.** Work is provisional. Deterministic checks (build/type-check, tests, lint, diff scope, secret scan, "no deleted or skipped tests", conflict markers) decide; an LLM reviewer must cite `file:line` plus a verbatim quote or its finding is dropped. A failed verification rolls the tree back and keeps the patch under `.fh/rejected/`. With no runnable checks the result says `UNVERIFIED`.
- **Intake funnel.** Inspects the repo, asks at most five targeted questions, enriches the task with acceptance criteria, always shows a plan (a compact one for trivial tasks). `--auto` skips questions and plan and runs up to five verification rounds with different review checklists.
- **Hidden skill system.** A deterministic gate (repo fingerprint + BM25, no LLM call) picks guidance from immutable built-in packs (senior personas, .NET Framework 4.8, .NET 8-10, React 18/19, Angular 17+, AngularJS 1.x, an Apple-HIG-inspired UI baseline) and from a per-user SQLite store of learned skills. Learned skills are project, stack or global scoped, validated as data-only, versioned, rolled back or quarantined automatically when they hurt verification results, and never shown to the user beyond activity notices. Your own `AGENTS.md` and `SKILL.md` files always win.
- **Master/worker orchestration.** Independent subtasks with disjoint file ownership run in parallel under a governor that reads vLLM `/metrics` (KV-cache usage, queue depth), so parallel long contexts do not evict each other's prefix cache.
- **Qwen/vLLM specifics.** Stable prompt prefix for prefix caching, edit-by-search/replace instead of file rewrites, thinking budget per step (`enable_thinking` on for planning, recovery and review, off for routine tool turns), streamed tool-call repair, `<tool_call>` recovery from content, think-leak detection, loop detection, structured-output fallback when guided decoding conflicts with the reasoning parser, retry/backoff, end-to-end cancellation (abort the request, kill the process tree).

Status: implemented and covered by 62 automated tests against a mock vLLM server, plus a real-browser check of the web UI. **Not yet measured against your real vLLM**: run `scripts/vps-validate.sh` (see [docs/VPS_VALIDATION_PROMPT.md](docs/VPS_VALIDATION_PROMPT.md)). GPU notes for your RTX 6000 Blackwell setup are in [docs/BLACKWELL.md](docs/BLACKWELL.md).

## Build and run

```bash
# Rust 1.80+ and a C compiler (build-essential) are needed to build; nothing is needed to run the binary.
git clone https://github.com/cardimvitor/frankstein-harness && cd frankstein-harness
cargo build --release              # ~2 minutes; produces target/release/fh

export FH_ENDPOINT=http://127.0.0.1:8000/v1
export FH_MODEL=<served model name>
export FH_API_KEY=<token>          # omit for an open endpoint (or FH_AUTH_SCHEME=none)

./target/release/fh doctor         # endpoint, model, auth, /metrics, sandbox
./target/release/fh run "fix the failing test in lib/math.js"
./target/release/fh run "add pagination to the orders endpoint" --auto --sandbox
./target/release/fh serve          # local web UI on 127.0.0.1:7878 (one-time code printed in the terminal)
```

Install it somewhere on your `PATH` (`cargo install --path .` or copy the binary). Run `fh --help` for every option.

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

Override what gets verified with `.fh/verify.json`: `[{"name":"build","cmd":"dotnet build","kind":"build"},{"name":"test","cmd":"dotnet test","kind":"test"}]` (`kind`: build, types, lint, test). Autodetected: npm/pnpm/yarn scripts, .NET solutions/projects (+ tests), pytest or unittest, `go`, `cargo`.

## Commands

| Command | What it does |
|---|---|
| `fh` / `fh chat` | interactive session in the current repo |
| `fh run "<task>"` | one task; `--auto`, `--mode plan\|ask\|auto-edit\|yolo`, `--yes`, `--plan-only`, `--commit`, `--keep`, `--sandbox`, `--json` |
| `fh serve` | local web UI: chat, streamed thinking, plan approval and questions, diffs, command output, skill notices, verification status, mode switch |
| `fh doctor` | connectivity and configuration check |
| `fh validate-vllm` | measure MTP acceptance, prefix cache, tool-call reliability, long context, concurrency, cancellation |
| `fh eval` | run the eval corpus, optionally against plain Qwen Code ([docs/EVAL.md](docs/EVAL.md)) |
| `fh undo` | restore the working tree to the last per-task checkpoint |
| `fh activity` | recent skill activity (created, improved, used, quarantined) |

## Platforms

Linux and macOS are exercised. Windows is a target (PowerShell shell tool, `taskkill` process-tree kill, `%LOCALAPPDATA%` data dir) but untested here, and there is no Windows sandbox yet; the test suite shells out to POSIX `sh`. See [docs/SECURITY.md](docs/SECURITY.md).

## Development

```bash
cargo test                          # mock vLLM server, no model needed
# real-browser check of the web UI (needs Playwright + Chromium, and `cargo build --release`):
PLAYWRIGHT_MODULE=$(npm root -g)/playwright node scripts/ui-smoke.mjs /tmp/fh-ui
```

`fh mock-server --port N` starts the scripted mock vLLM used by the tests, handy for dry-running `scripts/vps-validate.sh` without a GPU. The original TypeScript prototype that the Rust port was checked against lives in git history (commit `d7cc73e`).

## Docs

- [docs/DESIGN.md](docs/DESIGN.md): design, discovery findings, decisions and implementation status
- [docs/VPS_VALIDATION_PROMPT.md](docs/VPS_VALIDATION_PROMPT.md): the one prompt to validate everything on the VPS
- [docs/BLACKWELL.md](docs/BLACKWELL.md), [docs/EVAL.md](docs/EVAL.md), [docs/SECURITY.md](docs/SECURITY.md)

## License

MIT. This is a clean-room implementation: Qwen Code (Apache 2.0) and dsh (MIT) were read as design references only, and no code was copied from them.
