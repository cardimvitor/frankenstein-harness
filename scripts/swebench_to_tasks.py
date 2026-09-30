#!/usr/bin/env python3
"""Turn SWE-bench (Verified) instances into `fh eval` task directories.

    python3 scripts/swebench_to_tasks.py instances.jsonl --out eval/tasks --limit 25 --repos django/django,pallets/flask

Input: a JSONL file (one instance per line) or a JSON array with the SWE-bench fields
instance_id, repo, base_commit, problem_statement, test_patch, FAIL_TO_PASS, PASS_TO_PASS, version.
`--download N` fetches the first N rows of princeton-nlp/SWE-bench_Verified from the Hugging Face
datasets server instead (needs network access to huggingface.co).

Per instance the script clones the repository (cached as a mirror), checks out `base_commit`, applies the
hidden `test_patch`, strips `.git`, and writes
    <out>/<instance_id>/repo/          the starting state the agent works on
    <out>/<instance_id>/task.json      prompt, tags, timeouts, optional setup command
    <out>/<instance_id>/oracle.sh      FAIL_TO_PASS + PASS_TO_PASS runner (kept OUTSIDE repo/, never shown to the agent)

The oracle needs the repository's dependencies installed in the environment that runs `fh eval`
(SWE-bench normally uses one Docker image per instance; there is no Docker here). Pass `--setup-cmd`
(for example "pip install -e .") to run a preparation step on each copy before the agent starts.
Repositories whose test runner is not understood (sympy, ...) are skipped unless --allow-unknown.
"""
import argparse, json, os, re, shutil, subprocess, sys, tempfile, urllib.request


def sh(cmd, cwd=None, inp=None, check=True):
    r = subprocess.run(cmd, cwd=cwd, input=inp, text=True, capture_output=True)
    if check and r.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)} failed: {r.stderr.strip()[:400]}")
    return r


def load_instances(path, download):
    if download:
        rows, offset = [], 0
        while len(rows) < download:
            n = min(100, download - len(rows))
            url = ("https://datasets-server.huggingface.co/rows?dataset=princeton-nlp%2FSWE-bench_Verified"
                   f"&config=default&split=test&offset={offset}&length={n}")
            with urllib.request.urlopen(url, timeout=60) as f:
                data = json.load(f)
            batch = [r["row"] for r in data.get("rows", [])]
            if not batch:
                break
            rows += batch
            offset += len(batch)
        return rows
    text = open(path).read().strip()
    if text.startswith("["):
        return json.loads(text)
    return [json.loads(l) for l in text.splitlines() if l.strip()]


def as_list(v):
    if isinstance(v, str):
        try:
            v = json.loads(v)
        except ValueError:
            v = [v]
    return list(v or [])


def django_label(t):
    # "test_name (module.Class)" -> "module.Class.test_name"
    m = re.match(r"^(\S+) \(([\w.]+)\)$", t.strip())
    return f"{m.group(2)}.{m.group(1)}" if m else t.strip()


def oracle_script(repo, f2p, p2p, pytest_cmd, allow_unknown):
    """Returns the oracle shell script, or None when the repo's runner is unknown."""
    if repo == "django/django":
        labels = sorted({django_label(t) for t in f2p + p2p})
        body = "\n".join(labels)
        return ("set -e\nxargs -d '\\n' python tests/runtests.py --settings=test_sqlite --parallel 1 <<'FH_TESTS'\n" + body + "\nFH_TESTS\n")
    if repo == "sympy/sympy" and not allow_unknown:
        return None
    ids = f2p + p2p
    # tests in the same file are passed once per id; xargs batches them and fails if any batch fails
    return ("set -e\nxargs -d '\\n' " + pytest_cmd + " <<'FH_TESTS'\n" + "\n".join(ids) + "\nFH_TESTS\n")


def ensure_mirror(cache, repo, template):
    path = os.path.join(cache, repo.replace("/", "__") + ".git")
    if not os.path.isdir(path):
        os.makedirs(cache, exist_ok=True)
        sh(["git", "clone", "-q", "--mirror", template.format(repo=repo), path])
    return path


def convert(inst, out, cache, template, setup_cmd, pytest_cmd, allow_unknown):
    iid, repo = inst["instance_id"], inst["repo"]
    f2p, p2p = as_list(inst.get("FAIL_TO_PASS")), as_list(inst.get("PASS_TO_PASS"))
    if not f2p:
        return "no FAIL_TO_PASS tests"
    script = oracle_script(repo, f2p, p2p, pytest_cmd, allow_unknown)
    if script is None:
        return f"no known test runner for {repo} (use --allow-unknown to try pytest)"
    task_dir = os.path.join(out, iid)
    if os.path.exists(task_dir):
        shutil.rmtree(task_dir)
    mirror = ensure_mirror(cache, repo, template)
    repo_dir = os.path.join(task_dir, "repo")
    os.makedirs(task_dir)
    sh(["git", "clone", "-q", mirror, repo_dir])
    sh(["git", "checkout", "-q", inst["base_commit"]], cwd=repo_dir)
    if inst.get("test_patch"):
        sh(["git", "apply", "--whitespace=nowarn", "-"], cwd=repo_dir, inp=inst["test_patch"])
    shutil.rmtree(os.path.join(repo_dir, ".git"))
    open(os.path.join(task_dir, "oracle.sh"), "w").write(script)
    task = {
        "id": iid,
        "prompt": inst["problem_statement"].strip(),
        "oracleFile": "oracle.sh",
        "timeoutS": 1800,
        "oracleTimeoutS": 1800,
        "tags": ["swebench", repo, str(inst.get("version", ""))],
    }
    if setup_cmd:
        task["setup"] = setup_cmd
    json.dump(task, open(os.path.join(task_dir, "task.json"), "w"), indent=2)
    return None


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("instances", nargs="?", help="JSONL or JSON array of SWE-bench instances")
    ap.add_argument("--download", type=int, default=0, help="fetch the first N rows of SWE-bench_Verified instead")
    ap.add_argument("--out", default="eval/tasks")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--repos", default="", help="comma-separated owner/name filter")
    ap.add_argument("--ids", default="", help="comma-separated instance ids")
    ap.add_argument("--cache", default=os.path.join(tempfile.gettempdir(), "fh-swebench-cache"))
    ap.add_argument("--clone-url-template", default="https://github.com/{repo}.git")
    ap.add_argument("--setup-cmd", default="", help="run in each copy before the agent starts (e.g. 'pip install -e .')")
    ap.add_argument("--pytest-cmd", default="python -m pytest -q -p no:cacheprovider", help="command that receives the test ids")
    ap.add_argument("--allow-unknown", action="store_true")
    a = ap.parse_args()
    if not a.instances and not a.download:
        ap.error("give an instances file or --download N")
    insts = load_instances(a.instances, a.download)
    repos = {r for r in a.repos.split(",") if r}
    ids = {i for i in a.ids.split(",") if i}
    if repos:
        insts = [i for i in insts if i["repo"] in repos]
    if ids:
        insts = [i for i in insts if i["instance_id"] in ids]
    if a.limit:
        insts = insts[: a.limit]
    os.makedirs(a.out, exist_ok=True)
    done, skipped = [], []
    for inst in insts:
        try:
            why = convert(inst, a.out, a.cache, a.clone_url_template, a.setup_cmd, a.pytest_cmd, a.allow_unknown)
        except Exception as e:  # keep going: one bad instance must not stop the batch
            why = str(e)
        (skipped.append((inst["instance_id"], why)) if why else done.append(inst["instance_id"]))
    json.dump({"converted": done, "skipped": skipped}, open(os.path.join(a.out, "INDEX.json"), "w"), indent=2)
    print(f"converted {len(done)} task(s) into {a.out}; skipped {len(skipped)}")
    for i, why in skipped:
        print(f"  skipped {i}: {why}", file=sys.stderr)
    return 0 if done else 1


if __name__ == "__main__":
    sys.exit(main())
