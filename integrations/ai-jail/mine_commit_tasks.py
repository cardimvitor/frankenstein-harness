#!/usr/bin/env python3
"""Turn commits of a Rust repository into eval tasks, SWE-bench style, and grade them.

A task is: the repository at the PARENT of a fix/feature commit, an issue-style instruction (written by the
operator LLM, never by the model under test), and HIDDEN TESTS that the commit added or changed. The agent works
on the parent; the grader overlays the hidden tests on whatever the agent produced and runs only those tests.

Hidden tests come from two places, because Rust keeps many tests inline:
  * files under tests/ that the commit added or changed (copied over the candidate), and
  * the `#[cfg(test)] mod ... { }` blocks of src files that already existed at the parent and whose test module
    the commit changed (the candidate's module is REPLACED by the commit's, so tests the agent wrote or deleted
    in that module cannot hide a failure).

Commands (all print JSON):
  list     --repo DIR [--kinds fix,feat,perf,refactor] [--until SHA]      candidate commits
  build    --repo DIR --sha SHA --out DIR                                 write one task directory
  validate TASK_DIR --repo DIR [--cargo-args "..."]                       base+hidden must FAIL, reference+hidden must PASS
  grade    TASK_DIR WORKSPACE [--cargo-args "..."] [--timeout S]          exit 0 when every required test ran and passed

Set CARGO_TARGET_DIR to a shared directory to reuse compiled dependencies between runs.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def sh(cmd: list[str] | str, cwd: Path | None = None, timeout: int | None = None, env: dict | None = None, shell: bool = False):
    return subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=timeout, shell=shell, env=env)


def git(repo: Path, *args: str) -> str:
    r = sh(["git", "-C", str(repo), *args])
    if r.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)}: {r.stderr.strip()}")
    return r.stdout


# ---- Rust source scanning ---------------------------------------------------------------------------------------

def _skip_string(t: str, i: int) -> int:
    """t[i] is the first quote of a normal string; returns the index after its closing quote."""
    i += 1
    while i < len(t):
        if t[i] == "\\":
            i += 2
        elif t[i] == '"':
            return i + 1
        else:
            i += 1
    return i


def _skip_raw(t: str, i: int) -> int:
    """t[i] is 'r' of r"..." or r#"..."#; returns the index after the literal (or i when it is not a raw string)."""
    j = i + 1
    hashes = 0
    while j < len(t) and t[j] == "#":
        hashes += 1
        j += 1
    if j >= len(t) or t[j] != '"':
        return i
    end = '"' + "#" * hashes
    k = t.find(end, j + 1)
    return len(t) if k < 0 else k + len(end)


def _skip_char_or_lifetime(t: str, i: int) -> int:
    """t[i] == "'": a char literal ('x', '\\n', '\\u{..}') is skipped whole; a lifetime ('a) only the tick."""
    if i + 1 < len(t) and t[i + 1] == "\\":
        k = t.find("'", i + 2)
        return len(t) if k < 0 else k + 1
    if i + 2 < len(t) and t[i + 2] == "'":
        return i + 3
    return i + 1


def match_brace(t: str, open_idx: int) -> int:
    """t[open_idx] == '{'; index just after the matching '}' (strings, chars, comments and raw strings skipped)."""
    depth = 0
    i = open_idx
    n = len(t)
    while i < n:
        c = t[i]
        if c == "/" and t.startswith("//", i):
            k = t.find("\n", i)
            i = n if k < 0 else k
        elif c == "/" and t.startswith("/*", i):
            d = 1
            i += 2
            while i < n and d:
                if t.startswith("/*", i):
                    d += 1
                    i += 2
                elif t.startswith("*/", i):
                    d -= 1
                    i += 2
                else:
                    i += 1
        elif c == '"':
            i = _skip_string(t, i)
        elif c in "rb" and (t.startswith('r"', i) or t.startswith("r#", i) or t.startswith('br"', i) or t.startswith("br#", i)) and (i == 0 or not (t[i - 1].isalnum() or t[i - 1] == "_")):
            j = i + 1 if c == "r" else i + 2
            k = _skip_raw(t, j - 1)
            i = k if k != j - 1 else i + 1
        elif c == "'":
            i = _skip_char_or_lifetime(t, i)
        elif c == "{":
            depth += 1
            i += 1
        elif c == "}":
            depth -= 1
            i += 1
            if depth == 0:
                return i
        else:
            i += 1
    return n


MOD_RE = re.compile(r"#\[cfg\(test\)\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*\{")


def test_modules(text: str) -> list[tuple[int, int, str]]:
    """(start, end, text) of every top-level `#[cfg(test)] mod name { ... }` in the file, in order."""
    out = []
    pos = 0
    while True:
        m = MOD_RE.search(text, pos)
        if not m:
            return out
        end = match_brace(text, m.end() - 1)
        out.append((m.start(), end, text[m.start():end]))
        pos = end


def replace_test_modules(text: str, hidden: list[str]) -> str:
    """Replace the file's test modules, in order, by the hidden ones; extra hidden modules are appended."""
    mods = test_modules(text)
    out, last = [], 0
    for idx, (s, e, _) in enumerate(mods):
        out.append(text[last:s])
        out.append(hidden[idx] if idx < len(hidden) else "")
        last = e
    out.append(text[last:])
    res = "".join(out)
    for extra in hidden[len(mods):]:
        res = res.rstrip("\n") + "\n\n" + extra + "\n"
    return res


FN_RE = re.compile(r"((?:#\[[^\]]*\]\s*)+)(?:pub\s+)?(?:async\s+)?fn\s+(\w+)\s*\(")


def test_fns(text: str) -> dict[str, str]:
    """name -> source of every function marked #[test] / #[tokio::test] / #[..::test] in the text."""
    out = {}
    for m in FN_RE.finditer(text):
        if not re.search(r"#\[(?:\w+::)*test\b", m.group(1)):
            continue
        brace = text.find("{", m.end())
        if brace < 0:
            continue
        end = match_brace(text, brace)
        out[m.group(2)] = text[m.start():end]
    return out


# ---- candidate selection ----------------------------------------------------------------------------------------

def commit_tree_file(repo: Path, sha: str, path: str) -> str | None:
    r = sh(["git", "-C", str(repo), "show", f"{sha}:{path}"])
    return r.stdout if r.returncode == 0 else None


def analyze(repo: Path, sha: str, parent: str) -> dict:
    """What tests did this commit add or change, and are they separable from the implementation?"""
    files = git(repo, "diff", "--name-only", parent, sha).split()
    src = [f for f in files if f.startswith("src/") and f.endswith(".rs")]
    tests = [f for f in files if f.startswith("tests/") and f.endswith(".rs")]
    hidden_files, inline, names, new_src_with_tests = [], [], [], []
    for f in tests:
        after = commit_tree_file(repo, sha, f)
        if after is None:
            continue
        before = commit_tree_file(repo, parent, f) or ""
        hidden_files.append(f)
        b, a = test_fns(before), test_fns(after)
        names += [n for n, body in a.items() if b.get(n) != body]
    for f in src:
        after = commit_tree_file(repo, sha, f)
        before = commit_tree_file(repo, parent, f)
        if after is None:
            continue
        am = [m[2] for m in test_modules(after)]
        if not am:
            continue
        if before is None:
            new_src_with_tests.append(f)  # the whole file is new: its tests cannot be separated from the implementation
            continue
        bm = [m[2] for m in test_modules(before)]
        if am != bm:
            inline.append({"file": f, "modules": am})
            b, a = test_fns("\n".join(bm)), test_fns("\n".join(am))
            names += [n for n, body in a.items() if b.get(n) != body]
    return {"src": src, "tests_files": hidden_files, "inline": inline, "required_tests": sorted(set(names)), "new_src_with_tests": new_src_with_tests}


def cmd_list(a) -> int:
    repo = Path(a.repo)
    kinds = set(a.kinds.split(","))
    rev = a.until or "HEAD"
    rows = [l for l in git(repo, "log", "--no-merges", "--format=%H|%s|%P", rev).split("\n") if l.count("|") >= 2]
    out = []
    for l in rows:
        sha, subj, par = l.split("|", 2)
        if not par.strip():
            continue
        kind = re.split(r"[(:!]", subj)[0]
        if kind not in kinds:
            continue
        info = analyze(repo, sha, par.split()[0])
        if not info["src"] or not info["required_tests"]:
            continue
        out.append({"sha": sha, "parent": par.split()[0], "kind": kind, "subject": subj, "src_files": len(info["src"]), "test_files": len(info["tests_files"]), "inline_files": len(info["inline"]), "required_tests": len(info["required_tests"]), "new_src_with_tests": info["new_src_with_tests"]})
    print(json.dumps({"count": len(out), "candidates": out}, indent=2))
    return 0


# ---- building a task --------------------------------------------------------------------------------------------

def slug(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")[:40]


def cmd_build(a) -> int:
    repo, sha = Path(a.repo), a.sha
    sha = git(repo, "rev-parse", sha).strip()
    parent = git(repo, "rev-parse", f"{sha}^").strip()
    info = analyze(repo, sha, parent)
    if not info["required_tests"]:
        print(json.dumps({"error": "the commit adds or changes no separable tests"}))
        return 1
    subject = git(repo, "log", "-1", "--format=%s", sha).strip()
    body = git(repo, "log", "-1", "--format=%b", sha).strip()
    tdir = Path(a.out) / f"{sha[:8]}-{slug(subject)}"
    if tdir.exists():
        shutil.rmtree(tdir)
    (tdir / "hidden" / "tests").mkdir(parents=True)
    (tdir / "hidden" / "inline").mkdir(parents=True)
    for f in info["tests_files"]:
        dst = tdir / "hidden" / f
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_text(commit_tree_file(repo, sha, f) or "")
    for item in info["inline"]:
        (tdir / "hidden" / "inline" / (item["file"].replace("/", "__") + ".json")).write_text(json.dumps(item))
    ws = tdir / "workspace"
    ws.mkdir()
    extract_commit(repo, parent, ws)
    meta = {"id": tdir.name, "repo": str(repo), "reference": sha, "base": parent, "subject": subject, "required_tests": info["required_tests"],
            "tests_files": info["tests_files"], "inline_files": [i["file"] for i in info["inline"]], "src_files": info["src"], "new_src_with_tests": info["new_src_with_tests"]}
    (tdir / "task.json").write_text(json.dumps(meta, indent=2))
    (tdir / "instruction.draft.md").write_text(
        "<!-- DRAFT for the operator LLM. Rewrite as an issue: the behaviour wanted, observable symptoms, constraints. Do NOT paste the diff or\n"
        "     name the files/functions to change. Add one line per symbol listed in validation.json -> interface_hints (the hidden tests call it). -->\n\n"
        f"Commit subject: {subject}\n\nCommit body:\n{body}\n\nRequired tests (hidden): {', '.join(info['required_tests'])}\n")
    print(json.dumps({"task": str(tdir), **{k: meta[k] for k in ("reference", "base", "subject", "required_tests")}}, indent=2))
    return 0


# ---- running hidden tests ---------------------------------------------------------------------------------------

def copy_tree(src: Path, dst: Path) -> None:
    # shutil.copy keeps the permission bits but gives every file a NEW mtime (copy2 would keep the old one)
    shutil.copytree(src, dst, ignore=shutil.ignore_patterns("target", ".git"), dirs_exist_ok=True, copy_function=shutil.copy)


def extract_commit(repo: Path, sha: str, dst: Path) -> None:
    tar = subprocess.run(["git", "-C", str(repo), "archive", sha], capture_output=True, check=True)
    subprocess.run(["tar", "-x", "-m", "-C", str(dst)], input=tar.stdout, check=True)  # -m: files get the extraction time


def touch_tree(root: Path) -> None:
    """Cargo decides what is up to date by file mtimes. With a shared CARGO_TARGET_DIR (compiled dependencies are
    reused between runs) a source tree whose files look OLDER than the artifacts from an earlier workspace is
    taken as already built, and the wrong binary is tested. Bump every file to 'now' before each run."""
    import time
    now = time.time()
    for f in root.rglob("*"):
        if f.is_file() and "target" not in f.relative_to(root).parts[:1]:
            os.utime(f, (now, now))


def overlay_hidden(task: Path, ws: Path) -> list[str]:
    """Put the hidden tests on top of `ws`. Returns problems (a missing file the tests belong to is a failure)."""
    problems = []
    hid = task / "hidden"
    for f in (hid / "tests").rglob("*"):
        if f.is_file():
            dst = ws / f.relative_to(hid)
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(f, dst)
    for j in (hid / "inline").glob("*.json"):
        item = json.loads(j.read_text())
        target = ws / item["file"]
        if not target.exists():
            problems.append(f"{item['file']} is missing: the hidden tests live in it")
            continue
        target.write_text(replace_test_modules(target.read_text(), item["modules"]))
    return problems


INTERFACE_RES = [
    re.compile(r"cannot find (?:function|value|type|struct|enum|macro|trait|attribute macro) `([\w:]+)`"),
    re.compile(r"no (?:function or associated item|method|variant or associated item|field) (?:named )?`(\w+)` (?:found|on)"),
    re.compile(r"unresolved import `([\w:]+)`"),
    re.compile(r"failed to resolve: .*?`(\w+)`"),
    re.compile(r"missing field `(\w+)`"),
    re.compile(r"this function takes (\d+) arguments? but (\d+) arguments? (?:was|were) supplied"),
]


def interface_hints(stderr: str) -> list[str]:
    out = []
    for line in stderr.splitlines():
        for rx in INTERFACE_RES:
            m = rx.search(line)
            if m:
                out.append(line.strip()[:200])
                break
    seen, uniq = set(), []
    for h in out:
        if h not in seen:
            seen.add(h)
            uniq.append(h)
    return uniq[:40]


def run_tests(ws: Path, required: list[str], cargo_args: str, timeout: int) -> dict:
    cmd = ["cargo", "test", "--locked", *cargo_args.split(), "--", *required]
    touch_tree(ws)
    try:
        r = sh(cmd, cwd=ws, timeout=timeout, env={**os.environ, "CARGO_INCREMENTAL": "0"})
    except subprocess.TimeoutExpired:
        return {"passed": False, "reason": "timeout", "ran": [], "failed": [], "compile_error": False, "interface_hints": []}
    out = r.stdout + "\n" + r.stderr
    compile_error = "error[" in r.stderr or "could not compile" in r.stderr
    results = dict(re.findall(r"^test (\S+) \.\.\. (ok|FAILED|ignored)", out, re.M))
    ran, failed, ok = [], [], []
    for name in required:
        hits = [(n, st) for n, st in results.items() if n == name or n.endswith("::" + name)]
        if not hits:
            failed.append(name + " (did not run)")
            continue
        ran.append(name)
        if all(st == "ok" for _, st in hits):
            ok.append(name)
        else:
            failed.append(name)
    passed = not compile_error and len(ok) == len(required)
    cargo_failed = not results and r.returncode != 0 and not compile_error
    reason = "compile error" if compile_error else ("cargo failed before running tests (see tail)" if cargo_failed else ("ok" if passed else "tests failed"))
    return {"passed": passed, "reason": reason, "ran": ran, "ok": ok, "failed": failed,
            "compile_error": compile_error, "interface_hints": interface_hints(r.stderr) if compile_error else [], "tail": out[-1500:]}


def cmd_grade(a) -> int:
    task, cand = Path(a.task), Path(a.workspace)
    meta = json.loads((task / "task.json").read_text())
    with tempfile.TemporaryDirectory(prefix="grade-") as tmp:
        ws = Path(tmp) / "ws"
        copy_tree(cand, ws)
        problems = overlay_hidden(task, ws)
        res = run_tests(ws, meta["required_tests"], a.cargo_args, a.timeout) if not problems else {"passed": False, "reason": "; ".join(problems), "ran": [], "failed": meta["required_tests"], "compile_error": False, "interface_hints": []}
    res["task"] = meta["id"]
    print(json.dumps(res, indent=2))
    return 0 if res["passed"] else 1


def cmd_validate(a) -> int:
    """Fix the task's required tests for THIS environment.

    1. reference + hidden tests: the tests that ran and passed become the required set R (tests gated to another OS,
       or needing something the grader machine lacks, drop out and are listed in `dropped`);
    2. base + hidden tests, restricted to R: at least one must fail (compile errors count), otherwise the task does not
       test the change. Those that fail are fail_to_pass, the rest pass_to_pass (they must keep passing).
    Run it on the machine that will grade (bubblewrap and user namespaces present for a sandbox project like ai-jail).
    """
    task, repo = Path(a.task), Path(a.repo)
    meta = json.loads((task / "task.json").read_text())
    candidates = meta.get("candidate_tests") or meta["required_tests"]
    out = {"task": meta["id"], "candidate_tests": candidates}
    with tempfile.TemporaryDirectory(prefix="validate-") as tmp:
        ref = Path(tmp) / "ref"
        ref.mkdir()
        extract_commit(repo, meta["reference"], ref)
        out["reference"] = run_tests(ref, candidates, a.cargo_args, a.timeout)
        required = out["reference"].get("ok", [])
        out["dropped"] = [t for t in candidates if t not in required]
        if not required:
            out["base"] = {"passed": None, "reason": "not run: nothing passes on the reference here"}
            out["valid"], out["why_invalid"], out["interface_hints"] = False, ["no hidden test passes on the reference in this environment: " + out["reference"].get("reason", "")], []
        else:
            base = Path(tmp) / "base"
            copy_tree(task / "workspace", base)
            problems = overlay_hidden(task, base)
            out["base"] = run_tests(base, required, a.cargo_args, a.timeout) if not problems else {"passed": False, "reason": "; ".join(problems), "ok": [], "failed": required}
            base_ok = set(out["base"].get("ok", []))
            f2p = [t for t in required if t not in base_ok]
            out["fail_to_pass"], out["pass_to_pass"] = f2p, sorted(base_ok)
            out["valid"] = bool(f2p)
            out["why_invalid"] = [] if f2p else ["the hidden tests already pass on the base: they do not test the change"]
            out["interface_hints"] = out["base"].get("interface_hints", [])
            if out["valid"]:
                meta["candidate_tests"] = candidates
                meta["required_tests"] = required
                meta["fail_to_pass"], meta["pass_to_pass"] = f2p, sorted(base_ok)
                (task / "task.json").write_text(json.dumps(meta, indent=2))
    (task / "validation.json").write_text(json.dumps(out, indent=2))
    print(json.dumps({k: out.get(k) for k in ("task", "valid", "why_invalid", "fail_to_pass", "pass_to_pass", "dropped", "interface_hints")}, indent=2))
    return 0 if out["valid"] else 1


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    l = sub.add_parser("list")
    l.add_argument("--repo", required=True)
    l.add_argument("--kinds", default="fix,feat,perf,refactor")
    l.add_argument("--until")
    b = sub.add_parser("build")
    b.add_argument("--repo", required=True)
    b.add_argument("--sha", required=True)
    b.add_argument("--out", required=True)
    v = sub.add_parser("validate")
    v.add_argument("task")
    v.add_argument("--repo", required=True)
    v.add_argument("--cargo-args", default="")
    v.add_argument("--timeout", type=int, default=1200)
    g = sub.add_parser("grade")
    g.add_argument("task")
    g.add_argument("workspace")
    g.add_argument("--cargo-args", default="")
    g.add_argument("--timeout", type=int, default=900)
    a = ap.parse_args(argv)
    return {"list": cmd_list, "build": cmd_build, "validate": cmd_validate, "grade": cmd_grade}[a.cmd](a)


if __name__ == "__main__":
    sys.exit(main())
