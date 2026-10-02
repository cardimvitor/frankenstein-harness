"""python3 -m unittest discover -s integrations/ai-jail/tests -v   (the end-to-end test needs cargo and git)"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import mine_commit_tasks as m  # noqa: E402

SRC = '''pub fn add(a: i32, b: i32) -> i32 {
    let s = "}}} not a brace";
    let c = '}';
    let _ = (s, c);
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adds() {
        assert_eq!(add(1, 2), 3);
        let raw = r#"{ unbalanced"#;
        assert!(raw.len() > 0);
    }
}
'''


class Scanning(unittest.TestCase):
    def test_finds_the_test_module_despite_braces_in_strings_chars_and_raw_strings(self):
        mods = m.test_modules(SRC)
        self.assertEqual(len(mods), 1)
        self.assertTrue(mods[0][2].startswith("#[cfg(test)]\nmod tests {"))
        self.assertTrue(mods[0][2].rstrip().endswith("}"))
        self.assertNotIn("pub fn add", mods[0][2])

    def test_lifetimes_and_comments_do_not_confuse_the_matcher(self):
        t = "fn f<'a>(x: &'a str) -> &'a str { /* } */ x } // }\n#[cfg(test)]\nmod t { fn g<'a>() {} }\n"
        mods = m.test_modules(t)
        self.assertEqual(len(mods), 1)
        self.assertEqual(mods[0][2], "#[cfg(test)]\nmod t { fn g<'a>() {} }")

    def test_replacing_wipes_the_candidates_tests_and_appends_missing_modules(self):
        cand = SRC.replace("assert_eq!(add(1, 2), 3);", "assert!(true); // agent weakened it")
        hidden = [m.test_modules(SRC)[0][2]]
        out = m.replace_test_modules(cand, hidden)
        self.assertIn("assert_eq!(add(1, 2), 3)", out)
        self.assertNotIn("weakened", out)
        no_mod = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n"
        self.assertIn("mod tests", m.replace_test_modules(no_mod, hidden))

    def test_test_fns_picks_test_attributes_only(self):
        t = "#[test]\nfn a() { assert!(true); }\nfn helper() {}\n#[tokio::test]\nasync fn b() {}\n#[test]\n#[ignore]\nfn c() {}\n"
        self.assertEqual(sorted(m.test_fns(t)), ["a", "b", "c"])

    def test_interface_hints_from_compiler_errors(self):
        err = "error[E0425]: cannot find function `parse_port` in this scope\nerror[E0599]: no method named `jail` found for struct `Cfg`\nwarning: x\n"
        h = m.interface_hints(err)
        self.assertEqual(len(h), 2)
        self.assertIn("parse_port", h[0])


def run(cmd, cwd):
    return subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, check=True)


@unittest.skipUnless(shutil.which("cargo") and shutil.which("git"), "needs cargo and git")
class EndToEnd(unittest.TestCase):
    def test_list_build_validate_and_grade_on_a_tiny_crate(self):
        with tempfile.TemporaryDirectory() as d:
            repo = Path(d) / "repo"
            (repo / "src").mkdir(parents=True)
            (repo / "Cargo.toml").write_text('[package]\nname = "tiny"\nversion = "0.1.0"\nedition = "2021"\n')
            (repo / "src" / "lib.rs").write_text("pub fn clamp(x: i32, lo: i32, hi: i32) -> i32 {\n    if x < lo { lo } else { x }\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn below() { assert_eq!(clamp(-5, 0, 10), 0); }\n}\n")
            env = {"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"}
            git = lambda *a: subprocess.run(["git", *a], cwd=repo, capture_output=True, text=True, check=True, env={**os.environ, **env})
            git("init", "-q")
            subprocess.run(["cargo", "generate-lockfile"], cwd=repo, capture_output=True, check=True)
            git("add", "-A")
            git("commit", "-qm", "chore: base")
            (repo / "src" / "lib.rs").write_text("pub fn clamp(x: i32, lo: i32, hi: i32) -> i32 {\n    if x < lo { lo } else if x > hi { hi } else { x }\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn below() { assert_eq!(clamp(-5, 0, 10), 0); }\n    #[test]\n    fn above_is_clamped() { assert_eq!(clamp(50, 0, 10), 10); }\n}\n")
            git("commit", "-qam", "fix(clamp): honour the upper bound")
            sha = git("rev-parse", "HEAD").stdout.strip()
            target = os.environ.setdefault("CARGO_TARGET_DIR", str(Path(d) / "target"))
            self.assertTrue(target)

            out = subprocess.run([sys.executable, str(Path(m.__file__)), "list", "--repo", str(repo)], capture_output=True, text=True)
            cands = json.loads(out.stdout)
            self.assertEqual(cands["count"], 1)
            self.assertEqual(cands["candidates"][0]["required_tests"], 1)

            tasks = Path(d) / "tasks"
            b = subprocess.run([sys.executable, str(Path(m.__file__)), "build", "--repo", str(repo), "--sha", sha, "--out", str(tasks)], capture_output=True, text=True)
            self.assertEqual(b.returncode, 0, b.stderr + b.stdout)
            task = Path(json.loads(b.stdout)["task"])
            self.assertEqual(json.loads((task / "task.json").read_text())["required_tests"], ["above_is_clamped"])
            self.assertNotIn("above_is_clamped", (task / "workspace" / "src" / "lib.rs").read_text())

            v = subprocess.run([sys.executable, str(Path(m.__file__)), "validate", str(task), "--repo", str(repo)], capture_output=True, text=True)
            self.assertEqual(v.returncode, 0, v.stdout + v.stderr)
            vj = json.loads(v.stdout)
            self.assertTrue(vj["valid"])
            self.assertEqual(vj["fail_to_pass"], ["above_is_clamped"])
            self.assertEqual(vj["dropped"], [])

            # a candidate that does nothing fails; one that fixes the bug passes; one that weakens the module still fails
            cand = Path(d) / "cand"
            shutil.copytree(task / "workspace", cand)
            self.assertEqual(subprocess.run([sys.executable, str(Path(m.__file__)), "grade", str(task), str(cand)], capture_output=True, text=True).returncode, 1)
            (cand / "src" / "lib.rs").write_text("pub fn clamp(x: i32, lo: i32, hi: i32) -> i32 {\n    x.max(lo).min(hi)\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn above_is_clamped() { assert!(true); }\n}\n")
            g = subprocess.run([sys.executable, str(Path(m.__file__)), "grade", str(task), str(cand)], capture_output=True, text=True)
            self.assertEqual(g.returncode, 0, g.stdout + g.stderr)
            self.assertEqual(json.loads(g.stdout)["ok"], ["above_is_clamped"])
            (cand / "src" / "lib.rs").write_text("pub fn clamp(x: i32, lo: i32, hi: i32) -> i32 {\n    let _ = (lo, hi);\n    x\n}\n")
            self.assertEqual(subprocess.run([sys.executable, str(Path(m.__file__)), "grade", str(task), str(cand)], capture_output=True, text=True).returncode, 1)


class Drops(unittest.TestCase):
    @unittest.skipUnless(shutil.which("cargo") and shutil.which("git"), "needs cargo and git")
    def test_tests_that_do_not_run_on_the_reference_drop_out_and_a_task_that_already_passes_is_invalid(self):
        with tempfile.TemporaryDirectory() as d:
            repo = Path(d) / "repo"
            (repo / "src").mkdir(parents=True)
            (repo / "Cargo.toml").write_text('[package]\nname = "tiny"\nversion = "0.1.0"\nedition = "2021"\n')
            lib0 = "pub fn one() -> i32 { 1 }\n"
            (repo / "src" / "lib.rs").write_text(lib0)
            env = {"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"}
            git = lambda *a: subprocess.run(["git", *a], cwd=repo, capture_output=True, text=True, check=True, env={**os.environ, **env})
            git("init", "-q")
            subprocess.run(["cargo", "generate-lockfile"], cwd=repo, capture_output=True, check=True)
            git("add", "-A")
            git("commit", "-qm", "chore: base")
            # the commit adds a test that already passes on the base, and one gated to an OS that is not this one
            gated = "windows" if sys.platform != "win32" else "linux"
            (repo / "src" / "lib.rs").write_text(lib0 + f'\n#[cfg(test)]\nmod tests {{\n    use super::*;\n    #[test]\n    fn always_ok() {{ assert_eq!(one(), 1); }}\n    #[cfg(target_os = "{gated}")]\n    #[test]\n    fn other_os_only() {{ assert_eq!(one(), 1); }}\n}}\n')
            git("commit", "-qam", "fix(one): add tests")
            sha = git("rev-parse", "HEAD").stdout.strip()
            os.environ.setdefault("CARGO_TARGET_DIR", str(Path(d) / "target"))
            tasks = Path(d) / "tasks"
            b = subprocess.run([sys.executable, str(Path(m.__file__)), "build", "--repo", str(repo), "--sha", sha, "--out", str(tasks)], capture_output=True, text=True)
            task = Path(json.loads(b.stdout)["task"])
            v = subprocess.run([sys.executable, str(Path(m.__file__)), "validate", str(task), "--repo", str(repo)], capture_output=True, text=True)
            vj = json.loads(v.stdout)
            self.assertFalse(vj["valid"])
            self.assertEqual(vj["dropped"], ["other_os_only"])
            self.assertIn("do not test the change", vj["why_invalid"][0])


if __name__ == "__main__":
    unittest.main()
