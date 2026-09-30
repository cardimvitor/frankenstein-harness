#!/usr/bin/env python3
"""Drives the real `fh` TUI inside a pseudo-terminal against the scripted mock vLLM (no model needed).
   cargo build --release && python3 scripts/tui-smoke.py
Checks: header, plan card, approval, verified result, clean exit on Ctrl+D, file actually changed."""
import os, pty, select, time, re, sys, subprocess, tempfile, signal
BIN = os.environ.get('FH_BIN', os.path.abspath('target/release/fh'))
REPO = tempfile.mkdtemp(prefix='fh-tui-repo-')
open(REPO+'/mathx.py','w').write('def sum_range(a, b):\n    total = 0\n    for i in range(a, b):\n        total += i\n    return total\n')
open(REPO+'/test_mathx.py','w').write('import unittest\nfrom mathx import sum_range\n\nclass T(unittest.TestCase):\n    def test_inclusive(self):\n        self.assertEqual(sum_range(1, 4), 10)\n')
subprocess.run('git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -qm b', shell=True, cwd=REPO, check=True)
PORT = 20000 + os.getpid() % 10000
mock = subprocess.Popen([BIN, 'mock-server', '--port', str(PORT)], stdout=subprocess.DEVNULL)
time.sleep(1)

env = dict(os.environ, FH_ENDPOINT='http://127.0.0.1:'+str(PORT)+'/v1', FH_MODEL='mock-qwen', FH_AUTH_SCHEME='none', FH_HOME=os.path.join(os.environ.get('TMPDIR','/tmp'),'fh-tui-home'), TERM='xterm-256color')
pid, fd = pty.fork()
if pid == 0:
    os.chdir(REPO)
    os.execve(BIN, ['fh'], env)
import fcntl, termios, struct
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 130, 0, 0))
buf = b''
def pump(t=1.0):
    global buf
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try: buf += os.read(fd, 65536)
            except OSError: return
def text(): return re.sub(rb'\x1b\[[0-9;?]*[a-zA-Z]', b'', buf).decode('utf8', 'ignore')
pump(1.5)
print('HEADER' , 'Frankenstein' in text(), 'F1 for keys' in text())
os.write(fd, b'fix sum_range so it is inclusive'); pump(0.5)
os.write(fd, b'\r'); pump(4)
print('PLAN', 'Enter = go' in text() or 'Enter = approve' in text())
os.write(fd, b'\r'); pump(8)
t = text()
print('VERIFIED', 'Verified' in t, 'round 1: pass' in t)
os.write(fd, b'\x04'); pump(1)
try:
    _, status = os.waitpid(pid, os.WNOHANG)
    print('EXIT', os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0 if _ else 'still running')
except ChildProcessError: print('EXIT gone')
ok = 'b + 1' in open(REPO+'/mathx.py').read()
print('FILE', ok)
mock.terminate()
sys.exit(0 if ok else 1)
