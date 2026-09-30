# Study notes: Codex CLI and Hermes

**Not done.** The roadmap item (read the Codex CLI's sandbox and TUI code, write keep/avoid notes, and a "what not to copy" note from Hermes) needs the upstream source. From the build environment `api.github.com` answered 403 through the egress proxy, so nothing was read and nothing is claimed here.

What this project built without that study: a native Landlock + seccomp sandbox (`src/util/landlock.rs`) and a ratatui TUI (`src/tui/`), both verified on their own terms (real kernel and pseudo-terminal tests). If you want the comparison, run the study from a machine that can reach GitHub, or grant the session access to the repositories, and I will write it.
