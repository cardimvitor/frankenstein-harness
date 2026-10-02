"""Pieces shared by the Harbor and Pier adapters: the fh command line, result parsing and learned-skill sync."""

from __future__ import annotations

import contextlib
import fcntl
import json
import os
import shlex
import sqlite3
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

FH_BIN = "/installed-agent/fh"
FH_STATE = "/installed-agent/fh-state"
RESULT_FILE = "fh-result.json"
LOG_FILE = "fh.log"


def setting(kwargs_value: Any, env_name: str, default: str | None = None) -> str | None:
    """An adapter kwarg wins, then the host environment, then the default."""
    if kwargs_value not in (None, ""):
        return str(kwargs_value)
    return os.environ.get(env_name, default)


def endpoint_host(endpoint: str) -> str | None:
    return urlparse(endpoint).hostname


def run_command(instruction: str, *, logs_dir: str, max_concurrency: int | None, commit: bool, extra_flags: str = "") -> str:
    """The shell command that runs one fh task inside the task container.

    The task's own working directory (the image WORKDIR) is the workspace. fh needs git for checkpoints:
    a directory that is not a repository is left alone (fh still works, without rollback).
    """
    flags = "--auto --yes --mode yolo --json"
    if commit:
        flags += " --commit"
    if extra_flags:
        flags += " " + extra_flags
    conc = f"FH_MAX_CONCURRENCY={int(max_concurrency)} " if max_concurrency else ""
    result = logs_dir + "/" + RESULT_FILE
    return (
        f"mkdir -p {shlex.quote(logs_dir)} && "
        f"{conc}{FH_BIN} run {flags} {shlex.quote(instruction)} "
        f"> {shlex.quote(logs_dir + '/' + RESULT_FILE)} 2> {shlex.quote(logs_dir + '/' + LOG_FILE)}; "
        "code=$?; "
        # 0 (pass), 1 (verification failed) and 3 (unverified) are task outcomes: the benchmark's verifier decides.
        # Anything else (2 = bad usage, 130 = interrupted, a crash) is an adapter error and must surface.
        'if [ "$code" -ne 0 ] && [ "$code" -ne 1 ] && [ "$code" -ne 3 ]; then exit "$code"; fi; '
        # fh never reached the model (endpoint down, wrong key, proxy refusal): that is infrastructure, not a failed
        # attempt, so exit 4 and let the runner record an error (and retry) instead of a zero score
        f"if grep -q '\"verdict\": \"error\"' {shlex.quote(result)} 2>/dev/null && grep -q '\"requests\": 0' {shlex.quote(result)} 2>/dev/null; then exit 4; fi; "
        "exit 0"
    )


def parse_result(path: Path) -> dict[str, Any] | None:
    """fh --json prints one pretty JSON object last; earlier stdout lines (progress) are skipped."""
    try:
        text = path.read_text(errors="replace")
    except OSError:
        return None
    lines = text.splitlines()
    for i in range(len(lines) - 1, -1, -1):
        if lines[i] == "{":
            try:
                return json.loads("\n".join(lines[i:]))
            except json.JSONDecodeError:
                return None
    return None


# --- learned skills shared across trials (each trial runs in a fresh container) -------------------------

_APPEND_TABLES = ["skill_versions", "projects", "tasks", "task_skills", "activity", "verify_stats", "suppressed"]


@contextlib.contextmanager
def locked(state_dir: Path):
    state_dir.mkdir(parents=True, exist_ok=True)
    with open(state_dir / ".lock", "w") as fh:
        fcntl.flock(fh, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(fh, fcntl.LOCK_UN)


def snapshot(state_dir: Path, dest: Path) -> bool:
    """Copy the shared skill store to ``dest`` (consistent copy via the SQLite backup API)."""
    src = state_dir / "skills.db"
    if not src.exists():
        return False
    with locked(state_dir):
        a = sqlite3.connect(src)
        b = sqlite3.connect(dest)
        try:
            a.backup(b)
        finally:
            a.close()
            b.close()
    return True


def merge(state_dir: Path, incoming: Path) -> int:
    """Fold one trial's skill store into the shared one. Skills keep the newest version; history rows are appended.

    Concurrent trials each merge under a file lock, so nobody's learning overwrites another's.
    Returns the number of skills inserted or updated.
    """
    if not incoming.exists():
        return 0
    dest = state_dir / "skills.db"
    with locked(state_dir):
        if not dest.exists():
            a = sqlite3.connect(incoming)
            b = sqlite3.connect(dest)
            try:
                a.backup(b)
            finally:
                a.close()
                b.close()
            c = sqlite3.connect(dest)
            try:
                return c.execute("SELECT COUNT(*) FROM skills").fetchone()[0]
            except sqlite3.Error:
                return 0
            finally:
                c.close()
        c = sqlite3.connect(dest)
        try:
            c.execute("ATTACH DATABASE ? AS inc", (str(incoming),))
            have = {r[0] for r in c.execute("SELECT name FROM inc.sqlite_master WHERE type='table'")}
            changed = 0
            if "skills" in have:
                cur = c.execute(
                    "INSERT OR REPLACE INTO main.skills SELECT i.* FROM inc.skills i "
                    "LEFT JOIN main.skills m ON m.id = i.id WHERE m.id IS NULL OR i.updated > m.updated"
                )
                changed = cur.rowcount
            for t in _APPEND_TABLES:
                if t in have:
                    cols = [r[1] for r in c.execute(f"PRAGMA inc.table_info({t})")]
                    keys = " AND ".join(f"m.{k} IS i.{k}" for k in cols)
                    c.execute(f"INSERT OR IGNORE INTO main.{t} SELECT i.* FROM inc.{t} i WHERE NOT EXISTS (SELECT 1 FROM main.{t} m WHERE {keys})")
            c.commit()
            c.execute("DETACH DATABASE inc")
            return changed
        finally:
            c.close()


def share(state_dir: Path) -> None:
    """Group-readable/writable store (setgid directory), for the rare case where several users must write it directly."""
    try:
        state_dir.mkdir(parents=True, exist_ok=True)
        os.chmod(state_dir, (os.stat(state_dir).st_mode & 0o7777) | 0o2770)
        for name in ("skills.db", "skills.db-wal", "skills.db-shm"):
            f = state_dir / name
            if f.exists():
                os.chmod(f, (os.stat(f).st_mode & 0o7777) | 0o660)
    except OSError:
        pass
