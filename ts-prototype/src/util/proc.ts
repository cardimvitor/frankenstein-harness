import { spawn } from 'node:child_process';
import { platform } from 'node:os';

export interface RunResult { code: number | null; stdout: string; stderr: string; timedOut: boolean; aborted: boolean; ms: number }

export function killTree(pid: number | undefined) {
  if (!pid) return;
  try {
    if (platform() === 'win32') spawn('taskkill', ['/PID', String(pid), '/T', '/F'], { stdio: 'ignore' });
    else process.kill(-pid, 'SIGKILL');
  } catch { /* already gone */ }
}

const SECRET_ENV = /key|token|secret|password|passwd|credential|auth/i;
export function scrubbedEnv(env = process.env): NodeJS.ProcessEnv {
  const out: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(env)) {
    // NODE_TEST_CONTEXT makes a nested `node --test` report to its parent runner and always exit 0.
    if (k === 'NODE_TEST_CONTEXT') continue;
    if (k.startsWith('GIT_CONFIG_') || !SECRET_ENV.test(k)) out[k] = v;
  }
  return out;
}

export function shellFor(cmd: string): { file: string; args: string[] } {
  if (platform() === 'win32') return { file: 'powershell.exe', args: ['-NoProfile', '-NonInteractive', '-Command', cmd] };
  return { file: 'bash', args: ['-c', cmd] };
}

/** Truncate keeping head and tail so errors at the end survive. */
export function clip(s: string, max = 12000): string {
  if (s.length <= max) return s;
  const h = Math.floor(max * 0.4), t = max - h;
  return `${s.slice(0, h)}\n… [${s.length - max} chars omitted] …\n${s.slice(-t)}`;
}

export function run(cmd: string, opts: { cwd: string; timeoutMs?: number; signal?: AbortSignal; env?: NodeJS.ProcessEnv; wrap?: (c: { file: string; args: string[] }) => { file: string; args: string[] } }): Promise<RunResult> {
  return new Promise((resolve) => {
    const t0 = Date.now();
    let sh = shellFor(cmd);
    if (opts.wrap) sh = opts.wrap(sh);
    const child = spawn(sh.file, sh.args, {
      cwd: opts.cwd, env: opts.env ?? scrubbedEnv(), detached: platform() !== 'win32', stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stdout = '', stderr = '', timedOut = false, aborted = false, done = false;
    const cap = 4_000_000;
    child.stdout.on('data', (d) => { if (stdout.length < cap) stdout += d; });
    child.stderr.on('data', (d) => { if (stderr.length < cap) stderr += d; });
    const finish = (code: number | null) => {
      if (done) return; done = true;
      clearTimeout(timer); opts.signal?.removeEventListener('abort', onAbort);
      resolve({ code, stdout, stderr, timedOut, aborted, ms: Date.now() - t0 });
    };
    const timer = opts.timeoutMs ? setTimeout(() => { timedOut = true; killTree(child.pid); }, opts.timeoutMs) : undefined;
    const onAbort = () => { aborted = true; killTree(child.pid); };
    if (opts.signal?.aborted) onAbort(); else opts.signal?.addEventListener('abort', onAbort, { once: true });
    child.on('error', (e) => { stderr += String(e); finish(-1); });
    child.on('close', (c) => finish(c));
  });
}
