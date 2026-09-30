#!/usr/bin/env python3
"""Turn the Aider polyglot benchmark (225 Exercism exercises in C++, Go, Java, JavaScript, Python, Rust)
into `fh eval` task directories.

    python3 scripts/polyglot_to_tasks.py --langs python,rust,javascript,go --out eval/polyglot --limit-per-lang 10 --verify

The benchmark repository is cloned once (default https://github.com/Aider-AI/polyglot-benchmark) or read from --repo.
Per exercise: repo/ holds the stub and the tests (the reference solution under .meta/ and the instructions under .docs/
are NOT copied), the prompt is the exercise instructions in the style of the Aider benchmark, and the oracle runs the
language's test command. Skipped tests are enabled (xtest -> test, @Disabled removed, #[ignore] runs via --include-ignored).

--verify proves every task is solvable and non-trivial: the oracle must FAIL on the stub and PASS once the reference
solution is copied over the solution file. Tasks that fail either check (or whose toolchain is missing) are dropped.
"""
import argparse, json, os, re, shutil, subprocess, sys, tempfile

REPO_URL = "https://github.com/Aider-AI/polyglot-benchmark"
MANIFESTS = {"Cargo.toml", "package.json", "build.gradle", "CMakeLists.txt", "go.mod", "pom.xml"}

LANGS = {
    # oracle, optional setup (part of the base state), toolchain binary that must exist
    "python": {"oracle": "python3 -m unittest discover -q -p '*_test.py'", "setup": None, "bin": "python3"},
    "rust": {"oracle": "cargo test --quiet -- --include-ignored", "setup": None, "bin": "cargo"},
    "go": {"oracle": "go test ./...", "setup": None, "bin": "go"},
    "javascript": {"oracle": "npm test --silent", "setup": "npm install --no-audit --no-fund --silent", "bin": "npm"},
    "java": {"oracle": "gradle test --no-daemon -q", "setup": None, "bin": "gradle"},
    "cpp": {"oracle": "mkdir -p build && cd build && cmake .. -DEXERCISM_RUN_ALL_TESTS=1 >/dev/null && make -s && ctest --output-on-failure", "setup": None, "bin": "cmake"},
}


def run(cmd, cwd, timeout=900):
    try:
        return subprocess.run(cmd, cwd=cwd, shell=True, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(cmd, 124, "", "timeout")


def prompt_for(instr, solution_files):
    files = ", ".join(solution_files)
    return (f"{instr.strip()}\n\nUse the instructions above to modify the supplied files: {files}\n"
            "Don't change the names of existing functions or classes, as they may be referenced from other code like unit tests. "
            "Only use standard libraries; don't add dependencies. Do not modify the test files.")


def enable_tests(repo_dir, lang):
    """Aider's benchmark runs every test: undo the exercises' skip markers."""
    for root, _, files in os.walk(repo_dir):
        if "node_modules" in root or "/build" in root:
            continue
        for f in files:
            p = os.path.join(root, f)
            try:
                s = open(p, encoding="utf-8").read()
            except (UnicodeDecodeError, OSError):
                continue
            n = s
            if lang == "javascript" and f.endswith((".spec.js", ".test.js")):
                n = re.sub(r"\bxtest\(", "test(", n)
                n = re.sub(r"\bxit\(", "it(", n)
                n = re.sub(r"\bxdescribe\(", "describe(", n)
            if lang == "java" and f.endswith(".java"):
                n = re.sub(r'^\s*@Disabled\(.*\)\s*$', "", n, flags=re.M)
            if n != s:
                open(p, "w", encoding="utf-8").write(n)


def build_task(src, lang, name, out):
    cfg = json.load(open(os.path.join(src, ".meta", "config.json")))
    files = cfg.get("files", {})
    solution = files.get("solution", [])
    instr_path = os.path.join(src, ".docs", "instructions.md")
    if not solution or not os.path.exists(instr_path):
        return None, "no solution files or instructions"
    instr = open(instr_path, encoding="utf-8").read()
    append = os.path.join(src, ".docs", "instructions.append.md")
    if os.path.exists(append):
        instr += "\n\n" + open(append, encoding="utf-8").read()
    tid = f"polyglot-{lang}-{name}"
    tdir = os.path.join(out, tid)
    if os.path.exists(tdir):
        shutil.rmtree(tdir)
    repo = os.path.join(tdir, "repo")
    shutil.copytree(src, repo, ignore=shutil.ignore_patterns(".meta", ".docs", ".git", "build", "target", "node_modules", ".gradle"))
    enable_tests(repo, lang)
    if lang == "javascript":
        open(os.path.join(repo, ".gitignore"), "a").write("node_modules\n")
    if lang == "cpp":
        # CMake derives the exercise name from the directory, which is "repo" here
        cm = os.path.join(repo, "CMakeLists.txt")
        t = open(cm).read().replace("get_filename_component(exercise ${CMAKE_CURRENT_SOURCE_DIR} NAME)", f"set(exercise {name})")
        open(cm, "w").write(t)
        open(os.path.join(repo, ".gitignore"), "a").write("build\n")
    spec = LANGS[lang]
    task = {"id": tid, "prompt": prompt_for(instr, [s for s in solution]), "oracle": spec["oracle"], "timeoutS": 900, "oracleTimeoutS": 600, "tags": ["polyglot", lang]}
    if spec["setup"]:
        task["setup"] = spec["setup"]
    json.dump(task, open(os.path.join(tdir, "task.json"), "w"), indent=2)
    return (tdir, cfg), None


def verify(tdir, src, cfg, lang):
    """Oracle must fail on the stub and pass with the reference solution."""
    spec = LANGS[lang]
    solutions = [s for s in cfg["files"].get("solution", []) if os.path.basename(s) not in MANIFESTS]
    examples = cfg["files"].get("example", [])
    if not solutions or len(solutions) != len(examples):
        return "cannot map the reference solution"
    work = tempfile.mkdtemp(prefix="fh-poly-")
    try:
        w = os.path.join(work, "repo")
        shutil.copytree(os.path.join(tdir, "repo"), w)
        if spec["setup"]:
            run(spec["setup"], w, 900)
        if run(spec["oracle"], w).returncode == 0:
            return "oracle passes on the stub"
        for s, e in zip(solutions, examples):
            os.makedirs(os.path.dirname(os.path.join(w, s)) or w, exist_ok=True)
            shutil.copy(os.path.join(src, e), os.path.join(w, s))
        r = run(spec["oracle"], w)
        if r.returncode != 0:
            return "oracle fails with the reference solution: " + (r.stdout + r.stderr)[-200:].replace("\n", " ")
        return None
    finally:
        shutil.rmtree(work, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--repo", default="", help="local checkout of the polyglot benchmark (default: clone from GitHub)")
    ap.add_argument("--langs", default="python,rust,javascript,go")
    ap.add_argument("--out", default="eval/polyglot")
    ap.add_argument("--limit-per-lang", type=int, default=0)
    ap.add_argument("--exercises", default="", help="comma-separated exercise names")
    ap.add_argument("--verify", action="store_true")
    a = ap.parse_args()
    repo = a.repo
    if not repo:
        repo = os.path.join(tempfile.gettempdir(), "fh-polyglot-benchmark")
        if not os.path.isdir(repo):
            subprocess.run(["git", "clone", "-q", "--depth", "1", REPO_URL, repo], check=True)
    langs = [l for l in a.langs.split(",") if l]
    names_filter = {n for n in a.exercises.split(",") if n}
    os.makedirs(a.out, exist_ok=True)
    done, skipped = [], []
    for lang in langs:
        if lang not in LANGS:
            skipped.append((lang, "unknown language"))
            continue
        if not shutil.which(LANGS[lang]["bin"]):
            skipped.append((lang, f"toolchain '{LANGS[lang]['bin']}' not installed"))
            continue
        base = os.path.join(repo, lang, "exercises", "practice")
        names = sorted(os.listdir(base)) if os.path.isdir(base) else []
        if names_filter:
            names = [n for n in names if n in names_filter]
        count = 0
        for name in names:
            if a.limit_per_lang and count >= a.limit_per_lang:
                break
            src = os.path.join(base, name)
            built, why = build_task(src, lang, name, a.out)
            if why:
                skipped.append((f"{lang}/{name}", why))
                continue
            tdir, cfg = built
            if a.verify:
                why = verify(tdir, src, cfg, lang)
                if why:
                    shutil.rmtree(tdir, ignore_errors=True)
                    skipped.append((f"{lang}/{name}", why))
                    continue
            done.append(os.path.basename(tdir))
            count += 1
    json.dump({"converted": done, "skipped": skipped}, open(os.path.join(a.out, "INDEX.json"), "w"), indent=2)
    print(f"converted {len(done)} task(s) into {a.out}; skipped {len(skipped)}")
    for i, why in skipped:
        print(f"  skipped {i}: {why}", file=sys.stderr)
    return 0 if done else 1


if __name__ == "__main__":
    sys.exit(main())
