"""Per-task private skill stores, merged back by one owner.

Why: when many fh processes (different users, or containers with different uids) write ONE SQLite file, the first
writer's file mode locks the others out ("attempt to write a readonly database"), and WAL files shared across
containers are fragile. Instead:

    python3 -m fh_harbor.sync snapshot --from SHARED --to TASKDIR     # before the task; TASKDIR becomes its FH_HOME
    ... run the task with FH_HOME=TASKDIR (nothing else touches SHARED) ...
    python3 -m fh_harbor.sync merge --into SHARED TASKDIR [TASKDIR ...] # after the task, under a file lock

SHARED is only ever written by the one user that runs these commands, so permissions never conflict, and the merge
(newest skill wins, history rows appended once) takes milliseconds.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from fh_harbor import common


def _db_of(p: Path) -> Path:
    return p / "skills.db" if p.is_dir() else p


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="python3 -m fh_harbor.sync", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("snapshot", help="copy the shared store into a private directory for one task")
    s.add_argument("--from", dest="src", required=True, help="shared store directory (may not exist yet)")
    s.add_argument("--to", required=True, help="the task's private state directory (its FH_HOME)")
    m = sub.add_parser("merge", help="fold private stores back into the shared one")
    m.add_argument("--into", required=True, help="shared store directory")
    m.add_argument("--shared", action="store_true", help="also make the shared store group-writable (setgid dir)")
    m.add_argument("dbs", nargs="+", help="task state directories or skills.db files")
    a = ap.parse_args(argv)
    if a.cmd == "snapshot":
        to = Path(a.to)
        to.mkdir(parents=True, exist_ok=True)
        ok = common.snapshot(Path(a.src), to / "skills.db")
        print(f"snapshot: {'copied' if ok else 'shared store is empty, starting fresh'} -> {to}")
        return 0
    into = Path(a.into)
    total = 0
    for d in a.dbs:
        n = common.merge(into, _db_of(Path(d)))
        total += n
        print(f"merged {d}: {n} skill(s) new or updated")
    if a.shared:
        common.share(into)
    print(f"shared store {into / 'skills.db'}: merge done")
    return 0


if __name__ == "__main__":
    sys.exit(main())
