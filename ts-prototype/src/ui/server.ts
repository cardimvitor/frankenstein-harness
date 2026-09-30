import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { AddressInfo } from 'node:net';
import { Engine, type IO, type TaskResult } from '../engine.ts';
import type { Config } from '../config.ts';
import { redact, StreamRedactor } from '../config.ts';
import type { Mode } from '../types.ts';
import { Checkpoints } from '../session/checkpoint.ts';
import type { ReuseOffer } from '../skills/reuse.ts';

const WEB = join(dirname(fileURLToPath(import.meta.url)), 'web');
const STATIC: Record<string, [string, string]> = {
  '/': ['index.html', 'text/html; charset=utf-8'],
  '/app.js': ['app.js', 'text/javascript; charset=utf-8'],
  '/style.css': ['style.css', 'text/css; charset=utf-8'],
};

/** Strict CSP: no inline script/style, no third-party origins, no framing, no form posts elsewhere. */
export const CSP = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; font-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

const safeEq = (a: string, b: string) => { const A = Buffer.from(a), B = Buffer.from(b); return A.length === B.length && timingSafeEqual(A, B); };
const newSecret = (n = 24) => randomBytes(n).toString('base64url');

interface Ev { id: number; type: string; data: unknown }

export interface WebServer { url: string; port: number; code: () => string; close(): Promise<void>; engine: Engine; sessions: Set<string> }

export async function startWebServer(cfg: Config, cwd: string, opts: { port?: number; engine?: Engine; log?: (m: string) => void } = {}): Promise<WebServer> {
  const events: Ev[] = [];
  const clients = new Set<ServerResponse>();
  const pendingAsk = new Map<string, (v: any) => void>();
  const sessions = new Set<string>();
  let code = newSecret(9);
  let failures = 0;
  let nextId = 1;
  let busy: AbortController | undefined;
  let lastResult: TaskResult | undefined;

  let progressRed = new StreamRedactor(), reasoningRed = new StreamRedactor();
  const emit = (type: string, data: unknown) => {
    if (type !== 'progress' && type !== 'reasoning') {
      // flush text held back for cross-chunk redaction before any other event
      const p = progressRed.flush(), r = reasoningRed.flush();
      if (p) emit('progress', { d: p, raw: true });
      if (r) emit('reasoning', { d: r, raw: true });
      progressRed = new StreamRedactor(); reasoningRed = new StreamRedactor();
    }
    const ev = { id: nextId++, type, data };
    events.push(ev); if (events.length > 2000) events.shift();
    for (const res of clients) res.write(`id: ${ev.id}\nevent: ${ev.type}\ndata: ${JSON.stringify(ev.data)}\n\n`);
  };
  const ask = <T>(kind: string, payload: unknown): Promise<T> => new Promise((res) => {
    const id = newSecret(6); pendingAsk.set(id, res as (v: any) => void); emit('ask', { id, kind, payload });
  });

  const io: IO = {
    notice: (kind, message) => emit('notice', { kind, message: redact(message) }),
    progress: (d) => { const t = progressRed.push(d); if (t) emit('progress', { d: t, raw: true }); },
    reasoning: (d) => { const t = reasoningRed.push(d); if (t) emit('reasoning', { d: t, raw: true }); },
    toolStart: (name, args) => emit('tool_start', { name, args: JSON.parse(redact(JSON.stringify(args))) }),
    toolEnd: (name, ok, output, ms) => emit('tool_end', { name, ok, output: redact(output).slice(0, 6000), ms }),
    askQuestions: (questions) => ask<string[]>('questions', { questions }),
    approvePlan: (plan, trivial) => ask('plan', { plan, trivial }),
    confirm: (tool, args) => ask<boolean>('confirm', { tool: tool.spec.name, args: JSON.parse(redact(JSON.stringify(args))) }),
    offerReuse: (offers: ReuseOffer[]) => ask('reuse', { offers }),
  };
  const engine = opts.engine ?? new Engine(cfg, io, { cwd });
  engine.io = io;

  let allowedHosts = new Set<string>();

  const sec = (res: ServerResponse) => {
    res.setHeader('Content-Security-Policy', CSP);
    res.setHeader('X-Content-Type-Options', 'nosniff');
    res.setHeader('Referrer-Policy', 'no-referrer');
    res.setHeader('Cache-Control', 'no-store');
    res.setHeader('Cross-Origin-Resource-Policy', 'same-origin');
    res.setHeader('Cross-Origin-Opener-Policy', 'same-origin');
  };
  const send = (res: ServerResponse, status: number, body: unknown, type = 'application/json') => {
    sec(res);
    res.writeHead(status, { 'content-type': type });
    res.end(typeof body === 'string' ? body : JSON.stringify(body));
  };
  const cookieOf = (req: IncomingMessage) => (req.headers.cookie ?? '').split(/;\s*/).map((p) => p.split('=')).find(([k]) => k === 'fh_session')?.[1];
  const authed = (req: IncomingMessage) => { const c = cookieOf(req); return !!c && [...sessions].some((s) => safeEq(s, c)); };
  const readJson = (req: IncomingMessage) => new Promise<any>((resolve, reject) => {
    let n = 0; const chunks: Buffer[] = [];
    req.on('data', (b: Buffer) => { n += b.length; if (n > 1_000_000) { reject(new Error('too large')); req.destroy(); } else chunks.push(b); });
    req.on('end', () => { try { resolve(chunks.length ? JSON.parse(Buffer.concat(chunks).toString('utf8')) : {}); } catch { reject(new Error('bad json')); } });
  });

  const server: Server = createServer(async (req, res) => {
    try {
      // DNS-rebinding defence: only our own loopback host names are accepted.
      if (!allowedHosts.has(String(req.headers.host ?? '').toLowerCase())) return send(res, 403, { error: 'bad host' });
      const url = new URL(req.url ?? '/', 'http://x');
      const origin = req.headers.origin;
      if (origin && origin !== `http://${req.headers.host}`) return send(res, 403, { error: 'bad origin' });
      if (req.method === 'GET' && STATIC[url.pathname]) {
        const [file, type] = STATIC[url.pathname];
        return send(res, 200, readFileSync(join(WEB, file), 'utf8'), type);
      }
      if (!url.pathname.startsWith('/api/')) return send(res, 404, { error: 'not found' });
      if (req.method === 'POST' && (req.headers['x-fh'] !== '1' || !String(req.headers['content-type'] ?? '').startsWith('application/json'))) return send(res, 403, { error: 'bad request headers' });

      if (url.pathname === '/api/auth' && req.method === 'POST') {
        const body = await readJson(req);
        if (typeof body.code === 'string' && safeEq(body.code, code)) {
          const token = newSecret(32); sessions.add(token); code = newSecret(9); failures = 0; // one-time: consumed
          sec(res);
          res.writeHead(200, { 'content-type': 'application/json', 'set-cookie': `fh_session=${token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=86400` });
          return void res.end('{"ok":true}');
        }
        if (++failures >= 5) { code = newSecret(9); failures = 0; opts.log?.(`too many failed logins; new code: ${code}`); }
        return send(res, 401, { error: 'invalid code' });
      }
      if (!authed(req)) return send(res, 401, { error: 'not authenticated' });

      if (url.pathname === '/api/events' && req.method === 'GET') {
        sec(res);
        res.writeHead(200, { 'content-type': 'text/event-stream', connection: 'keep-alive' });
        const last = Number(req.headers['last-event-id'] ?? url.searchParams.get('after') ?? 0);
        for (const ev of events) if (ev.id > last) res.write(`id: ${ev.id}\nevent: ${ev.type}\ndata: ${JSON.stringify(ev.data)}\n\n`);
        clients.add(res); req.on('close', () => clients.delete(res));
        return;
      }
      if (url.pathname === '/api/state') return send(res, 200, { busy: !!busy, cwd: engine.cwd, model: cfg.model, last: lastResult ? { verdict: lastResult.verdict } : null });
      if (url.pathname === '/api/activity') return send(res, 200, engine.store.activity(50));
      if (url.pathname === '/api/task' && req.method === 'POST') {
        if (busy) return send(res, 409, { error: 'a task is already running' });
        const b = await readJson(req);
        if (typeof b.task !== 'string' || !b.task.trim() || b.task.length > 20000) return send(res, 400, { error: 'task required' });
        const mode = b.mode === 'auto' ? 'auto' : 'normal';
        const approval: Mode = (['plan', 'ask', 'auto-edit', 'yolo'] as const).includes(b.approval) ? b.approval : 'auto-edit';
        busy = new AbortController(); emit('busy', { busy: true, task: b.task.slice(0, 200) });
        const ac = busy;
        void engine.runTask(b.task, { mode, approval, signal: ac.signal, sandbox: !!b.sandbox }).then((r) => { lastResult = r; emit('result', JSON.parse(redact(JSON.stringify(r)))); }, (e) => emit('result', { verdict: 'error', reason: redact(String(e?.message ?? e)) }))
          .finally(() => { busy = undefined; emit('busy', { busy: false }); void engine.drain(); });
        return send(res, 202, { ok: true });
      }
      if (url.pathname === '/api/answer' && req.method === 'POST') {
        const b = await readJson(req);
        const fn = pendingAsk.get(String(b.id));
        if (!fn) return send(res, 404, { error: 'no such question' });
        pendingAsk.delete(String(b.id)); fn(b.value);
        return send(res, 200, { ok: true });
      }
      if (url.pathname === '/api/cancel' && req.method === 'POST') {
        busy?.abort();
        for (const [id, fn] of pendingAsk) { pendingAsk.delete(id); fn(false); }
        return send(res, 200, { ok: true });
      }
      if (url.pathname === '/api/undo' && req.method === 'POST') {
        const cp = new Checkpoints(engine.cwd);
        return send(res, 200, { ok: true, note: (await cp.isRepo()) ? 'use the rolled-back patch in .fh/rejected or git; per-turn undo runs through the CLI (fh undo)' : 'not a git repository' });
      }
      return send(res, 404, { error: 'not found' });
    } catch (e) {
      send(res, 500, { error: redact(String((e as Error).message)) });
    }
  });

  await new Promise<void>((r) => server.listen(opts.port ?? 0, '127.0.0.1', r));
  const port = (server.address() as AddressInfo).port;
  allowedHosts = new Set([`127.0.0.1:${port}`, `localhost:${port}`]);
  return {
    url: `http://127.0.0.1:${port}/`, port, code: () => code, sessions, engine,
    close: () => new Promise<void>((r) => { for (const c of clients) c.end(); server.closeAllConnections(); server.close(() => r()); }),
  };
}
