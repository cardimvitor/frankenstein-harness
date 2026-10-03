#!/usr/bin/env python3
"""Go/no-go check of the evaluation machine. Run it BEFORE downloading or starting anything.

    python3 scripts/preflight.py --workdir /workspace/harness-eval [--min-disk-gb 300] [--json out.json]

Prints PASS / WARN / FAIL per check and exits 1 if any check FAILs (abort), 0 otherwise. It only reads and probes:
it never installs, kills or changes anything. The model repositories probed default to Qwen3.8-27B-NVFP4 @dbb8f445, the MiMo-9B NVFP4+MTP build;
override with FH_REPO_BIG, FH_REV_BIG, FH_REPO_SMALL_MTP.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import urllib.request

RESULTS: list[dict] = []
DEFAULTS = {"FH_REPO_BIG": "nvidia/Qwen3.8-27B-NVFP4", "FH_REPO_SMALL_MTP": "ycui7/MiMo-V2.6-Distill-Qwen-9B-NVFP4-MTP"}
DEFAULT_REV_BIG = "dbb8f445"


def sh(cmd: str, timeout: int = 20) -> tuple[int, str]:
    try:
        p = subprocess.run(cmd, shell=True, capture_output=True, text=True, timeout=timeout)
        return p.returncode, (p.stdout + p.stderr).strip()
    except subprocess.TimeoutExpired:
        return 124, "timeout"


def check(name: str, status: str, detail: str, fix: str = "") -> None:
    RESULTS.append({"check": name, "status": status, "detail": detail, "fix": fix})
    mark = {"PASS": "[ OK ]", "WARN": "[WARN]", "FAIL": "[FAIL]"}[status]
    print(f"{mark} {name}: {detail}" + (f"\n         -> {fix}" if fix and status != "PASS" else ""))


def version_tuple(v: str) -> tuple[int, ...]:
    return tuple(int(x) for x in re.findall(r"\d+", v)[:3])


def gpu() -> None:
    rc, out = sh("nvidia-smi --query-gpu=name,memory.total,compute_cap,driver_version --format=csv,noheader,nounits")
    if rc != 0:
        check("gpu.present", "FAIL", "nvidia-smi failed: " + out[:120], "install the NVIDIA driver; this evaluation needs the GPU")
        return
    lines = [l for l in out.splitlines() if l.strip()]
    check("gpu.count", "PASS" if len(lines) == 1 else "WARN", f"{len(lines)} GPU(s): " + "; ".join(lines), "the plan assumes exactly one GPU; with more, state which one is used (CUDA_VISIBLE_DEVICES)")
    name, mem, cc, drv = [x.strip() for x in lines[0].split(",")][:4]
    check("gpu.memory", "PASS" if float(mem) >= 90000 else "FAIL", f"{name}: {float(mem)/1024:.0f} GiB", "needs ~96 GB: two models run side by side (0.78 + 0.14 of the memory)")
    check("gpu.arch", "PASS" if cc.startswith("12.") else "WARN", f"compute capability {cc} (Blackwell SM120 expected)", "the validated NVFP4 + MTP path is for Blackwell; on another architecture the reference speed will not match")
    check("gpu.driver", "PASS" if version_tuple(drv) >= (580,) else "WARN", f"driver {drv}", "CUDA 13 needs a recent driver (580+)")
    rc, out = sh("nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader")
    procs = [l for l in out.splitlines() if l.strip()] if rc == 0 else []
    check("gpu.free", "PASS" if not procs else "WARN", "no compute process on the GPU" if not procs else f"{len(procs)} process(es) using the GPU: " + "; ".join(procs[:5]), "phase 0 stops them (the machine is dedicated), but confirm none is someone else's production job")
    rc, out = sh("nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits")
    if rc == 0 and out.strip().isdigit():
        used = int(out.strip())
        check("gpu.vram_used", "PASS" if used < 500 else "WARN", f"{used} MiB in use", "must be < 500 MiB before the servers start")


def machine(workdir: str, min_disk: int) -> None:
    cpus = os.cpu_count() or 0
    check("cpu.cores", "PASS" if cpus >= 16 else ("WARN" if cpus >= 8 else "FAIL"), f"{cpus} cores", "tasks run in containers with 2 CPUs each; fewer than 16 cores caps the useful concurrency")
    try:
        mem_kb = int(re.search(r"MemTotal:\s+(\d+)", open("/proc/meminfo").read()).group(1))
        gb = mem_kb / 1024 / 1024
        check("ram", "PASS" if gb >= 64 else ("WARN" if gb >= 32 else "FAIL"), f"{gb:.0f} GiB", "16 GB stay reserved for the servers and the proxy; each task container gets 4 GB")
    except Exception as e:
        check("ram", "WARN", f"cannot read /proc/meminfo: {e}")
    os.makedirs(workdir, exist_ok=True)
    free = shutil.disk_usage(workdir).free / 1e9
    check("disk.free", "PASS" if free >= min_disk else "FAIL", f"{free:.0f} GB free in {workdir} (need {min_disk})", "copies of every exercise per configuration, checkpoints and container images")
    try:
        t = os.path.join(workdir, ".preflight-write-test")
        open(t, "w").write("x")
        os.remove(t)
        check("disk.writable", "PASS", "workdir is writable")
    except Exception as e:
        check("disk.writable", "FAIL", str(e))
    rc, out = sh("df -k /dev/shm | tail -1")
    if rc == 0:
        parts = out.split()
        gb = int(parts[1]) / 1024 / 1024 if len(parts) > 1 and parts[1].isdigit() else 0
        check("shm", "PASS" if gb >= 16 else "WARN", f"/dev/shm {gb:.0f} GiB", "vLLM wants a large /dev/shm (mount -o remount,size=32G /dev/shm)")
    rc, out = sh("ulimit -n")
    check("ulimit.nofile", "PASS" if out.isdigit() and int(out) >= 65536 else "WARN", f"open files limit {out}", "ulimit -n 1048576; many parallel containers and sockets")
    rc, out = sh("uname -srm; cat /etc/os-release | head -2 | tr '\\n' ' '")
    check("os", "PASS" if "Linux" in out else "FAIL", out.replace("\n", " "), "Linux is required")


def hogs() -> None:
    """Who is using the machine right now. The machine is rented and dedicated: anything not part of the evaluation should go in phase 0."""
    rc, out = sh("ps -eo pid,user,pcpu,pmem,comm --sort=-pcpu | head -8")
    check("resources.top_cpu", "PASS", "top CPU users:\n         " + out.replace("\n", "\n         ") if rc == 0 else "ps failed")
    rc, out = sh("ps -eo pid,user,pcpu,pmem,comm --sort=-pmem | head -6")
    check("resources.top_mem", "PASS", "top memory users:\n         " + out.replace("\n", "\n         ") if rc == 0 else "ps failed")
    rc, out = sh("cat /proc/loadavg")
    load = float(out.split()[0]) if rc == 0 and out.split() else 0.0
    cpus = os.cpu_count() or 1
    check("resources.load", "PASS" if load < cpus * 0.25 else "WARN", f"load average {load:.1f} on {cpus} cores", "something is busy: find it in the list above and, if it is not part of the evaluation, stop it in phase 0")
    rc, out = sh("free -m | awk '/Swap/ {print $3\" of \"$2\" MiB swap used\"}'")
    check("resources.swap", "PASS", out or "no swap info")
    rc, out = sh("systemctl list-units --type=service --state=running --no-legend 2>/dev/null | awk '{print $1}' | head -60 | tr '\\n' ' '")
    check("resources.services", "PASS", ("running services: " + out[:600]) if out else "systemd not available or no services listed")
    rc, out = sh("cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null")
    check("resources.cpu_governor", "PASS" if out in ("", "performance") else "WARN", out or "not exposed (virtualized host)", "cpupower frequency-set -g performance")
    rc, out = sh("nvidia-smi --query-gpu=persistence_mode,power.limit,power.max_limit,clocks.max.sm --format=csv,noheader")
    check("resources.gpu_settings", "PASS", out or "n/a", "enable persistence mode; leave the power limit at its maximum so the card is not throttled")


def takeover() -> None:
    """Can this session actually take over the whole (rented) machine? Reports; never works around a provider restriction."""
    uid = os.geteuid()
    rc, out = sh("sudo -n true 2>&1 && echo sudo-ok")
    has = uid == 0 or "sudo-ok" in out
    check("takeover.privileges", "PASS" if has else "FAIL", "root" if uid == 0 else ("passwordless sudo" if has else "not root and no passwordless sudo"), "the takeover needs root: run the session as root or grant passwordless sudo")
    rc, out = sh("cat /proc/1/cgroup 2>/dev/null | head -1; cat /sys/fs/cgroup/cpu.max 2>/dev/null; cat /sys/fs/cgroup/memory.max 2>/dev/null")
    lines = out.splitlines()
    cpu_cap = next((l for l in lines if l.split()[:1] and l.split()[0] != "max" and re.match(r"^\d+ \d+$", l)), "")
    mem_cap = next((l for l in lines if l.isdigit()), "")
    check("takeover.cgroup_limits", "PASS" if not (cpu_cap or mem_cap) else "WARN", "no cgroup CPU/memory cap on this session" if not (cpu_cap or mem_cap) else f"cgroup caps: cpu.max={cpu_cap or '-'} memory.max={mem_cap or '-'}", "a cap set by the provider is a limit to REPORT to the owner, not to bypass; if it is ours (systemd slice/user limits), lift it")
    rc, out = sh("nvidia-smi -q 2>/dev/null | grep -i -E 'MIG Mode|Current  *: *(Enabled|Disabled)|Virtualization Mode' | head -4 | tr '\\n' ' '")
    check("takeover.gpu_partition", "WARN" if re.search(r"Enabled|vGPU|VGPU", out or "") else "PASS", out or "no MIG/vGPU info", "MIG or vGPU means the card is partitioned: ask the owner whether the whole GPU is meant to be available")
    rc, out = sh("who 2>/dev/null | awk '{print $1}' | sort -u | tr '\\n' ' '")
    check("takeover.other_sessions", "PASS", ("logged-in users: " + out) if out else "no other interactive sessions", "other users' sessions are listed in the inventory; the owner decides about them")
    rc, out = sh("ps -eo user= | sort | uniq -c | sort -rn | head -6 | tr '\\n' ';'")
    check("takeover.process_owners", "PASS", out or "n/a")


def containers() -> None:
    rt = None
    for cand in ("docker", "podman"):
        if shutil.which(cand):
            rt = cand
            break
    if not rt:
        check("containers.runtime", "FAIL", "neither docker nor podman found", "install Docker (or Podman); the public benchmarks do not run without containers")
        return
    rc, out = sh(f"{rt} info --format '{{{{.ServerVersion}}}}' 2>&1 | head -1" if rt == "docker" else "podman info --format '{{.Version.Version}}' 2>&1 | head -1", 30)
    check("containers.runtime", "PASS" if rc == 0 else "FAIL", f"{rt} {out[:60]}", "the daemon must be running and usable by this user")
    if rc != 0:
        return
    rc, out = sh(f"{rt} run --rm --network none alpine:3.20 echo ok 2>&1 | tail -1", 120)
    check("containers.run", "PASS" if out.strip().endswith("ok") else "FAIL", out[-80:] if out else "no output", "must be able to pull and run a container (registry access)")
    rc, out = sh(f"{rt} network create --internal fh-preflight-net >/dev/null 2>&1 && echo created; {rt} network rm fh-preflight-net >/dev/null 2>&1", 30)
    check("containers.internal_network", "PASS" if "created" in out else "FAIL", "internal network can be created" if "created" in out else "cannot create an --internal network", "the isolation of the agents depends on it")
    rc, out = sh(f"{rt} run --rm --memory 256m --cpus 1 alpine:3.20 echo quota-ok 2>&1 | tail -1", 60)
    check("containers.quotas", "PASS" if "quota-ok" in out else "WARN", out[-80:], "CPU and memory quotas per task container are required")
    rc, out = sh("ip -4 addr show docker0 2>/dev/null | grep -o 'inet [0-9.]*' | head -1")
    check("containers.host_gateway", "PASS" if rc == 0 else "WARN", out or "no docker0 address (ok for podman/rootless)", "task containers must reach the proxy on the host")


def toolchain() -> None:
    need = {"git": "", "python3": "", "curl": "", "tar": ""}
    for t in need:
        check(f"tool.{t}", "PASS" if shutil.which(t) else "FAIL", shutil.which(t) or "missing")
    py = sys.version_info
    check("tool.python_version", "PASS" if py >= (3, 11) else "WARN", f"python {py.major}.{py.minor}", "Harbor/Pier need Python 3.12 (install with uv)")
    for t, fix in (("uv", "pip install uv"), ("cargo", "install rustup (or use a prebuilt fh binary)"), ("node", "Node 24.15.0"), ("npm", "with Node"), ("hf", "pip install -U huggingface_hub[cli] hf_transfer"), ("pwsh", "PowerShell for some exercises")):
        check(f"tool.{t}", "PASS" if shutil.which(t) else "WARN", shutil.which(t) or "missing (installed in phase 0)", fix)
    rc, out = sh("python3 -c 'import vllm,sys;print(vllm.__version__)' 2>&1 | tail -1")
    ok = rc == 0 and re.match(r"\d+\.\d+", out or "") and version_tuple(out) >= (0, 30)
    check("vllm.version", "PASS" if ok else "WARN", out[:60] if out else "not installed", "vLLM >= 0.30 (the MiMo-9B NVFP4 card asks for it); installed in phase 0")


def network() -> None:
    def probe(label: str, url: str, fix: str, method: str = "GET", fail_status: str = "FAIL") -> None:
        try:
            req = urllib.request.Request(url, method=method, headers={"User-Agent": "fh-preflight"})
            with urllib.request.urlopen(req, timeout=15) as r:
                check(f"net.{label}", "PASS", f"{url} -> HTTP {r.status}")
        except Exception as e:
            code = getattr(e, "code", None)
            ok = code in (200, 301, 302, 307, 401, 403)
            check(f"net.{label}", "PASS" if ok else fail_status, f"{url} -> {code or e}", fix)

    probe("huggingface", "https://huggingface.co", "the checkpoints come from the Hugging Face Hub")
    probe("github", "https://github.com", "the plan clones repositories")
    probe("pypi", "https://pypi.org/simple/pip/", "Harbor, Pier, vLLM")
    probe("crates", "https://index.crates.io/config.json", "Rust crates for the fh build and ai-jail", fail_status="WARN")
    probe("npm", "https://registry.npmjs.org/", "OpenCode, Qwen Code, Claude Code installers", fail_status="WARN")
    probe("registry", "https://registry-1.docker.io/v2/", "container images", fail_status="FAIL")
    for var, label in (("FH_REPO_BIG", "big"), ("FH_REPO_SMALL_MTP", "small_mtp")):
        repo = os.environ.get(var) or DEFAULTS[var]
        if not repo:
            check(f"model.{label}", "WARN", f"{var} not set: model repo not probed", f"export {var}=<org/name>")
            continue
        rev = (os.environ.get("FH_REV_BIG") or DEFAULT_REV_BIG) if var == "FH_REPO_BIG" else None
        url = f"https://huggingface.co/api/models/{repo}" + (f"/revision/{rev}" if rev else "")
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "fh-preflight"}), timeout=20) as r:
                data = json.load(r)
                files = [s["rfilename"] for s in data.get("siblings", [])]
                has_w = any(f.endswith(".safetensors") for f in files)
                try:
                    with urllib.request.urlopen(urllib.request.Request(url + "?blobs=true", headers={"User-Agent": "fh-preflight"}), timeout=20) as r2:
                        tot = sum((x.get("size") or 0) for x in json.load(r2).get("siblings", []))
                    check(f"model.{label}.size", "PASS", f"{tot/1e9:.1f} GB to download")
                except Exception:
                    pass
                check(f"model.{label}", "PASS" if has_w else "FAIL", f"{repo}{('@' + rev) if rev else ''}: {len(files)} files, safetensors {'present' if has_w else 'MISSING'}", "the exact revision must exist; if not, stop and ask the owner")
        except Exception as e:
            check(f"model.{label}", "FAIL", f"{repo}: {getattr(e, 'code', e)}", "repository or revision not reachable (private? renamed? gated?)")
    for port in (8000, 8001, 8002, 8003, 8090):
        s = socket.socket()
        s.settimeout(1)
        busy = s.connect_ex(("127.0.0.1", port)) == 0
        s.close()
        check(f"port.{port}", "PASS" if not busy else "WARN", "free" if not busy else "in use", "ports 8000-8003 are the two vLLM servers and the proxies, 8090 serves the fh binary")


def aijail() -> None:
    check("aijail.bwrap", "PASS" if shutil.which("bwrap") else "WARN", shutil.which("bwrap") or "bubblewrap missing", "the ai-jail session needs bwrap; without it the grader is wrong (PoisonError cascade)")
    rc, out = sh("unshare -Ur true 2>&1 && echo userns-ok")
    check("aijail.userns", "PASS" if "userns-ok" in out else "WARN", "user namespaces work" if "userns-ok" in out else out[:100], "enable unprivileged user namespaces (sysctl kernel.unprivileged_userns_clone=1 / user.max_user_namespaces)")
    rc, out = sh("bwrap --ro-bind / / --dev /dev true 2>&1 && echo bwrap-ok") if shutil.which("bwrap") else (1, "no bwrap")
    check("aijail.bwrap_runs", "PASS" if "bwrap-ok" in out else "WARN", "bwrap can create a sandbox" if "bwrap-ok" in out else out[:100], "required only for the ai-jail session")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--workdir", default=os.environ.get("WORKDIR", "./harness-eval"))
    ap.add_argument("--min-disk-gb", type=int, default=300, help="250 is the floor with cleanup, 400-500 recommended (plan section 1.1)")
    ap.add_argument("--json")
    a = ap.parse_args()
    for section in (gpu, lambda: machine(a.workdir, a.min_disk_gb), hogs, takeover, containers, toolchain, network, aijail):
        section()
    fails = [r for r in RESULTS if r["status"] == "FAIL"]
    warns = [r for r in RESULTS if r["status"] == "WARN"]
    print(f"\n{len(RESULTS) - len(fails) - len(warns)} PASS, {len(warns)} WARN, {len(fails)} FAIL -> {'NO-GO: abort and report' if fails else 'GO (review the WARNs)'}")
    if a.json:
        json.dump({"go": not fails, "results": RESULTS}, open(a.json, "w"), indent=2)
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
