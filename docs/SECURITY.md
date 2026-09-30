# Security model and checklist

Frankenstein Harness runs model-generated commands with your privileges. Nothing here is a substitute for a disposable VM or container for untrusted repositories.

## Local web UI checklist (all enforced in `src/ui/server.ts`, all tested in `test/web.test.ts` and by the live smoke test)

| Control | Implementation |
|---|---|
| Loopback only | binds `127.0.0.1`; never `0.0.0.0` |
| DNS rebinding | every request must carry `Host: 127.0.0.1:<port>` or `localhost:<port>` |
| Cross-origin / WebSocket-style CSRF | `Origin`, when present, must equal the page origin; every POST requires `X-FH: 1` and `Content-Type: application/json` (forces a CORS preflight for other origins); no CORS headers are ever sent |
| No token in URL | a one-time code is printed in the terminal, typed into the page and exchanged for a session cookie; the code is consumed on use, rotated after 5 bad attempts |
| Cookie | `HttpOnly; SameSite=Strict; Path=/`, random 256-bit token, constant-time comparison |
| Strict CSP | `default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'self'; frame-ancestors 'none'`; the page has no inline script or style |
| XSS from repo or model text | the client never uses `innerHTML`; markdown is rendered with a small DOM builder (no raw HTML, no links) |
| Secret leakage | values of environment variables whose names contain key/token/secret/password/auth are redacted from every event, including secrets split across streamed chunks and the final result |
| Other headers | `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `Cache-Control: no-store`, `Cross-Origin-Resource-Policy: same-origin` |
| Request size | JSON bodies over 1 MB are rejected |

Not covered: TLS (loopback only), multi-user access, and exposing the port through a tunnel. If you tunnel it (for example SSH port-forward to a VPS), the browser still talks to `127.0.0.1` on your machine; do not publish the port.

## Agent safety

- **Modes**: `plan` (read-only), `ask`, `auto-edit` (default), `yolo`. A hard deny list (root/home deletion, disk formatting, fork bombs, pipe-to-shell, force push, power commands) applies in every mode including `yolo`.
- **Workspace confinement**: file tools resolve paths through symlinks and refuse anything outside the workspace or inside `.git`. Workers can only write files they own.
- **Environment scrubbing**: shell commands run without variables that look like secrets.
- **OS sandbox (opt-in, `--sandbox`)**: Linux bubblewrap (read-only root, writable workspace, private `/tmp`, PID namespace) and macOS Seatbelt (writes limited to the workspace and temp). **Windows has no sandbox yet**; use a VM/container there.
- **Verification gate**: file changes are provisional. Failed verification rolls back to the pre-task checkpoint and saves the rejected patch under `.fh/rejected/`. A run with no build/test command is reported `UNVERIFIED`.
- **Prompt injection**: repository text and tool output are data. They cannot change permissions: approvals are decided by code, not by model output, and hidden skills cannot contain commands, URLs, secrets or override phrases.

## Skill store

- Lives in the OS user data directory (`$XDG_DATA_HOME`, `~/Library/Application Support`, `%LOCALAPPDATA%`), never in the repository.
- Skills are data only: writes are rejected if they contain URLs, shell commands, secrets, or instruction-override language. Repo-derived skills stay project-scoped; promotion needs verified successes in at least two projects.
- Version history with diffs, rollback, and quarantine. A skill whose tasks fail verification more often than comparable tasks without it is rolled back or quarantined automatically.
- User-visible files (`AGENTS.md`, `.fh/skills/*/SKILL.md`, and read-only compatibility with `.qwen/skills`, `.claude/skills`, `.agents/skills`) override internal skills.

## Credentials

Bearer token from the environment variable named by `apiKeyEnv` (default `FH_API_KEY`). Custom header and no-auth schemes are supported (`authScheme`). Secrets are never written to config, logs, reports or skills. mTLS and OIDC are not implemented.
