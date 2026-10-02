"""Run with:  python3 -m unittest discover -s integrations/harbor/tests -v   (no Harbor or Docker needed)."""
import json
import os
import sqlite3
import stat
import subprocess
import sys
import tempfile
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from fh_harbor import common  # noqa: E402

SKILLS = "id TEXT PRIMARY KEY, name TEXT, scope TEXT, stack TEXT, versions TEXT, project_id TEXT, source TEXT, origin TEXT, summary TEXT, keywords TEXT, body TEXT, version INTEGER, hash TEXT, state TEXT, created INTEGER, updated INTEGER"
DDL = f"""
CREATE TABLE skills({SKILLS});
CREATE TABLE skill_versions(skill_id TEXT, version INTEGER, hash TEXT, body TEXT, diff TEXT, reason TEXT, ts INTEGER);
CREATE TABLE projects(id TEXT PRIMARY KEY, label TEXT, stack_key TEXT, tokens TEXT, first_seen INTEGER, last_seen INTEGER);
CREATE TABLE tasks(id TEXT PRIMARY KEY, project_id TEXT, verdict TEXT, rounds INTEGER, ts INTEGER);
CREATE TABLE task_skills(task_id TEXT, skill_id TEXT, skill_version INTEGER);
CREATE TABLE activity(ts INTEGER, kind TEXT, skill TEXT, reason TEXT, project_id TEXT);
CREATE TABLE verify_stats(task_id TEXT PRIMARY KEY, raised INTEGER, valid INTEGER, dropped INTEGER, blockers INTEGER, rounds INTEGER, verdict TEXT, tokens INTEGER, ms INTEGER, ts INTEGER);
CREATE TABLE suppressed(skill_id TEXT PRIMARY KEY, reason TEXT, ts INTEGER);
"""


def make_db(path, skills=(), tasks=()):
    c = sqlite3.connect(path)
    c.executescript(DDL)
    for sid, body, updated in skills:
        c.execute("INSERT INTO skills VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", (sid, sid, "global", None, None, None, "learned", "t", "", "", body, 1, "h", "active", 1, updated))
        c.execute("INSERT INTO skill_versions VALUES (?,?,?,?,?,?,?)", (sid, 1, "h", body, "", "r", updated))
    for tid in tasks:
        c.execute("INSERT INTO tasks VALUES (?,?,?,?,?)", (tid, "p", "pass", 1, 1))
        c.execute("INSERT INTO activity VALUES (?,?,?,?,?)", (1, "used", "s", tid, "p"))
    c.commit()
    c.close()


def count(path, table):
    c = sqlite3.connect(path)
    try:
        return c.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
    finally:
        c.close()


class RunCommand(unittest.TestCase):
    def run_fake(self, body: str, bin_name="fake-fh"):
        with tempfile.TemporaryDirectory() as d:
            fake = Path(d) / bin_name
            fake.write_text("#!/bin/bash\n" + body)
            fake.chmod(fake.stat().st_mode | stat.S_IXUSR)
            old = common.FH_BIN
            common.FH_BIN = str(fake)
            try:
                logs = Path(d) / "logs"
                cmd = common.run_command("fix the bug 'quoted' $HOME", logs_dir=str(logs), max_concurrency=2, commit=True)
            finally:
                common.FH_BIN = old
            r = subprocess.run(["bash", "-c", cmd], capture_output=True, text=True)
            return r.returncode, (logs / common.RESULT_FILE).read_text() if (logs / common.RESULT_FILE).exists() else ""

    def test_task_outcomes_do_not_raise(self):
        for code in (0, 1, 3):
            rc, out = self.run_fake(f'echo \'{{"verdict": "fail", "llm": {{"requests": 5}}}}\'; exit {code}')
            self.assertEqual(rc, 0, code)

    def test_usage_error_and_interrupts_surface(self):
        for code in (2, 130, 101):
            rc, _ = self.run_fake(f"exit {code}")
            self.assertEqual(rc, code)

    def test_model_unreachable_is_an_infrastructure_error(self):
        rc, _ = self.run_fake('printf \'{\\n  "verdict": "error",\\n  "llm": {\\n    "requests": 0\\n  }\\n}\\n\'; exit 1')
        self.assertEqual(rc, 4)

    def test_error_after_requests_is_a_task_failure(self):
        rc, _ = self.run_fake('printf \'{\\n  "verdict": "error",\\n  "llm": {\\n    "requests": 7\\n  }\\n}\\n\'; exit 1')
        self.assertEqual(rc, 0)

    def test_instruction_is_passed_verbatim_and_env_flags(self):
        rc, out = self.run_fake('printf "%s|%s|%s" "$FH_MAX_CONCURRENCY" "$*" ""; exit 0')
        self.assertEqual(rc, 0)
        self.assertIn("2|run --auto --yes --mode yolo --json --commit fix the bug 'quoted' $HOME", out)


class ParseResult(unittest.TestCase):
    def test_skips_progress_lines(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "r.json"
            p.write_text('✦ using skill "x"\n▸ planning\n{\n  "verdict": "pass",\n  "llm": {"requests": 3}\n}\n')
            self.assertEqual(common.parse_result(p)["verdict"], "pass")
            p.write_text("garbage only")
            self.assertIsNone(common.parse_result(p))
            self.assertIsNone(common.parse_result(Path(d) / "missing"))


class SkillSync(unittest.TestCase):
    def test_first_merge_copies_then_newest_skill_wins_and_history_appends_once(self):
        with tempfile.TemporaryDirectory() as d:
            shared, a, b = Path(d) / "shared", Path(d) / "a.db", Path(d) / "b.db"
            make_db(a, skills=[("s1", "old", 10)], tasks=["t1"])
            make_db(b, skills=[("s1", "new", 20), ("s2", "other", 5)], tasks=["t1", "t2"])
            common.merge(shared, a)
            common.merge(shared, b)
            common.merge(shared, b)  # idempotent
            db = shared / "skills.db"
            c = sqlite3.connect(db)
            self.assertEqual(c.execute("SELECT body FROM skills WHERE id='s1'").fetchone()[0], "new")
            c.close()
            self.assertEqual(count(db, "skills"), 2)
            self.assertEqual(count(db, "tasks"), 2)
            self.assertEqual(count(db, "activity"), 2)  # t1 appears in both inputs but is stored once; t2 once

    def test_older_incoming_skill_does_not_overwrite(self):
        with tempfile.TemporaryDirectory() as d:
            shared, a, b = Path(d) / "shared", Path(d) / "a.db", Path(d) / "b.db"
            make_db(a, skills=[("s1", "new", 20)])
            make_db(b, skills=[("s1", "stale", 10)])
            common.merge(shared, a)
            common.merge(shared, b)
            c = sqlite3.connect(shared / "skills.db")
            self.assertEqual(c.execute("SELECT body FROM skills").fetchone()[0], "new")
            c.close()

    def test_concurrent_merges_lose_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            shared = Path(d) / "shared"
            dbs = []
            for i in range(8):
                p = Path(d) / f"in{i}.db"
                make_db(p, skills=[(f"s{i}", "x", i + 1)], tasks=[f"t{i}"])
                dbs.append(p)
            with ThreadPoolExecutor(8) as ex:
                list(ex.map(lambda p: common.merge(shared, p), dbs))
            self.assertEqual(count(shared / "skills.db", "skills"), 8)
            self.assertEqual(count(shared / "skills.db", "tasks"), 8)

    def test_snapshot_is_consistent_copy(self):
        with tempfile.TemporaryDirectory() as d:
            shared = Path(d) / "shared"
            src = Path(d) / "a.db"
            make_db(src, skills=[("s1", "x", 1)])
            common.merge(shared, src)
            out = Path(d) / "snap.db"
            self.assertTrue(common.snapshot(shared, out))
            self.assertEqual(count(out, "skills"), 1)
            self.assertFalse(common.snapshot(Path(d) / "none", Path(d) / "x.db"))


if __name__ == "__main__":
    unittest.main()
