import { test } from 'node:test';
import assert from 'node:assert/strict';
import { request } from 'node:http';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execSync } from 'node:child_process';
import { startMock } from './mock-vllm.ts';
import { loadConfig } from '../src/config.ts';
import { startWebServer, CSP } from '../src/ui/server.ts';
import { Engine, headlessIO } from '../src/engine.ts';
import { SkillStore } from '../src/skills/store.ts';
import { BUILTIN_TASKS } from '../src/eval/corpus.ts';

interface Res { status: number; headers: Record<string, any>; body: string }
function http(port: number, method: string, path: string, headers: Record<string, string> = {}, body?: unknown): Promise<Res> {
  return new Promise((resolve, reject) => {
    const req = request({ host: '127.0.0.1', port, method, path, headers: { host: `127.0.0.1:${port}`, ...headers } }, (res) => {
      let b = ''; res.on('data', (c) => (b += c)); res.on('end', () => resolve({ status: res.statusCode!, headers: res.headers, body: b }));
    });
    req.on('error', reject);
    if (body !== undefined) req.write(JSON.stringify(body));
    req.end();
  });
}
const J = { 'content-type': 'application/json', 'x-fh': '1' };

async function setup() {
  const m = await startMock();
  const d = mkdtempSync(join(tmpdir(), 'fh-w-'));
  const t = BUILTIN_TASKS[0];
  for (const [f, b] of Object.entries(t.files)) { mkdirSync(join(d, f, '..'), { recursive: true }); writeFileSync(join(d, f), b); }
  execSync('git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -qm b', { cwd: d });
  const cfg = { ...loadConfig('/x', {} as any), endpoint: m.url, metricsUrl: m.url.replace('/v1', '/metrics'), retries: 0 };
  const srv = await startWebServer(cfg, d, { engine: new Engine(cfg, headlessIO(), { cwd: d, store: new SkillStore(':memory:') }) });
  return { m, d, srv, cfg };
}
async function login(port: number, code: string) {
  const r = await http(port, 'POST', '/api/auth', J, { code });
  const cookie = String(r.headers['set-cookie']?.[0] ?? '').split(';')[0];
  return { r, cookie };
}

test('web: static pages carry a strict CSP and hardening headers; no inline script/style in the page', async () => {
  const { m, srv } = await setup();
  const r = await http(srv.port, 'GET', '/');
  assert.equal(r.status, 200); assert.equal(r.headers['content-security-policy'], CSP);
  assert.match(CSP, /default-src 'none'/); assert.doesNotMatch(CSP, /unsafe-inline|unsafe-eval|\*/);
  assert.equal(r.headers['x-content-type-options'], 'nosniff'); assert.equal(r.headers['referrer-policy'], 'no-referrer');
  assert.doesNotMatch(r.body, /<script(?![^>]*\bsrc=)[^>]*>/); assert.doesNotMatch(r.body, /\son\w+=|style=/);
  assert.equal(srv.url.startsWith('http://127.0.0.1:'), true); assert.doesNotMatch(srv.url, /code|token|#/);
  await srv.close(); await m.close();
});

test('web: Host and Origin validation (DNS rebinding, cross-origin), CSRF header, no CORS headers', async () => {
  const { m, srv } = await setup();
  assert.equal((await http(srv.port, 'GET', '/', { host: 'evil.example.com' })).status, 403);
  assert.equal((await http(srv.port, 'GET', '/', { host: `evil.example.com:${srv.port}` })).status, 403);
  assert.equal((await http(srv.port, 'GET', '/', { host: `localhost:${srv.port}` })).status, 200);
  assert.equal((await http(srv.port, 'POST', '/api/auth', { ...J, origin: 'http://evil.example.com' }, { code: 'x' })).status, 403);
  assert.equal((await http(srv.port, 'POST', '/api/auth', { 'content-type': 'text/plain' }, '{}')).status, 403); // needs x-fh + json
  const ok = await http(srv.port, 'POST', '/api/auth', { ...J, origin: `http://127.0.0.1:${srv.port}` }, { code: 'wrong' });
  assert.equal(ok.status, 401);
  assert.equal(ok.headers['access-control-allow-origin'], undefined);
  const pre = await http(srv.port, 'OPTIONS', '/api/task', { origin: 'http://evil.example.com', 'access-control-request-method': 'POST' });
  assert.equal(pre.status, 403);
  await srv.close(); await m.close();
});

test('web: one-time code -> HttpOnly SameSite=Strict cookie; code is single-use; API needs the cookie', async () => {
  const { m, srv } = await setup();
  assert.equal((await http(srv.port, 'GET', '/api/state')).status, 401);
  const code = srv.code();
  const { r, cookie } = await login(srv.port, code);
  assert.equal(r.status, 200);
  const sc = String(r.headers['set-cookie'][0]);
  assert.match(sc, /HttpOnly/); assert.match(sc, /SameSite=Strict/); assert.doesNotMatch(sc, /Domain=/);
  assert.equal((await http(srv.port, 'GET', '/api/state', { cookie })).status, 200);
  assert.equal((await login(srv.port, code)).r.status, 401); // consumed
  assert.notEqual(srv.code(), code);
  assert.equal((await http(srv.port, 'GET', '/api/state', { cookie: 'fh_session=forged' })).status, 401);
  await srv.close(); await m.close();
});

test('web: repeated bad codes rotate the code', async () => {
  const { m, srv } = await setup();
  const c0 = srv.code();
  for (let i = 0; i < 5; i++) await http(srv.port, 'POST', '/api/auth', J, { code: `bad${i}` });
  assert.notEqual(srv.code(), c0);
  assert.equal((await login(srv.port, c0)).r.status, 401);
  await srv.close(); await m.close();
});

test('web e2e: submit a task over the API, stream events over SSE, get a verified result; secrets are redacted', async () => {
  const { m, d, srv } = await setup();
  process.env.FH_API_KEY = 'super-secret-token-value-123';
  m.fallback = (req: any) => {
    const p = req.response_format?.json_schema?.schema?.properties;
    if (p?.trivial) return { content: JSON.stringify({ trivial: true, questions: [], enriched: 'fix sumRange', acceptance: ['tests pass'], plan: [{ step: 'fix', files: ['lib/math.js'] }], assumptions: [], subtasks: [] }) };
    if (p?.verdict) return { content: JSON.stringify({ verdict: 'pass', findings: [] }) };
    if (req.messages.at(-1).role === 'tool') return { content: 'Done. The key is super-secret-token-value-123 (should be redacted).' };
    return { tool_calls: [{ name: 'edit', args: { path: 'lib/math.js', old_text: 'i < b; i++', new_text: 'i <= b; i++' } }] };
  };
  const { cookie } = await login(srv.port, srv.code());
  const events: { type: string; data: any }[] = [];
  const done = new Promise<void>((resolve) => {
    const req = request({ host: '127.0.0.1', port: srv.port, path: '/api/events', headers: { host: `127.0.0.1:${srv.port}`, cookie } }, (res) => {
      let buf = '';
      res.on('data', (c) => {
        buf += c;
        for (let i; (i = buf.indexOf('\n\n')) >= 0;) {
          const blk = buf.slice(0, i); buf = buf.slice(i + 2);
          const type = blk.match(/^event: (.+)$/m)?.[1]; const data = blk.match(/^data: (.+)$/m)?.[1];
          if (type && data) { events.push({ type, data: JSON.parse(data) }); if (type === 'result') { req.destroy(); resolve(); } }
        }
      });
    });
    req.on('error', () => resolve()); req.end();
  });
  const sub = await http(srv.port, 'POST', '/api/task', { ...J, cookie }, { task: 'fix sumRange', mode: 'auto', approval: 'yolo' });
  assert.equal(sub.status, 202);
  assert.equal((await http(srv.port, 'POST', '/api/task', { ...J, cookie }, { task: 'second' })).status, 409); // one at a time
  await done;
  const result = events.find((e) => e.type === 'result')!.data;
  assert.equal(result.verdict, 'pass'); assert.match(result.diff, /i <= b/);
  assert.ok(events.some((e) => e.type === 'tool_end' && e.data.name === 'edit'));
  assert.ok(events.some((e) => e.type === 'notice' && e.data.kind === 'verify'));
  assert.doesNotMatch(JSON.stringify(events), /super-secret-token-value-123/); assert.match(result.final, /«redacted»|Verified/);
  assert.match(readFileSync(join(d, 'lib/math.js'), 'utf8'), /i <= b/);
  delete process.env.FH_API_KEY;
  await srv.close(); await m.close();
});

test('web: interactive plan approval round trip and cancel', async () => {
  const { m, srv } = await setup();
  m.fallback = (req: any) => {
    const p = req.response_format?.json_schema?.schema?.properties;
    if (p?.trivial) return { content: JSON.stringify({ trivial: false, questions: [], enriched: 'x', acceptance: [], plan: [{ step: 'do it', files: ['lib/math.js'] }], assumptions: [], subtasks: [] }) };
    return { content: 'noop' };
  };
  srv.engine.io = srv.engine.io; // keep web IO
  const { cookie } = await login(srv.port, srv.code());
  let ask: any;
  const got = new Promise<void>((resolve) => {
    const req = request({ host: '127.0.0.1', port: srv.port, path: '/api/events', headers: { host: `127.0.0.1:${srv.port}`, cookie } }, (res) => {
      let buf = '';
      res.on('data', (c) => { buf += c; const mm = buf.match(/event: ask\ndata: (.+)\n\n/); if (mm) { ask = JSON.parse(mm[1]); req.destroy(); resolve(); } });
    });
    req.on('error', () => resolve()); req.end();
  });
  await http(srv.port, 'POST', '/api/task', { ...J, cookie }, { task: 'fix', mode: 'normal', approval: 'ask' });
  await got;
  assert.equal(ask.kind, 'plan'); assert.match(ask.payload.plan, /do it/);
  assert.equal((await http(srv.port, 'POST', '/api/answer', { ...J, cookie }, { id: 'nope', value: true })).status, 404);
  await http(srv.port, 'POST', '/api/answer', { ...J, cookie }, { id: ask.id, value: { ok: false } }); // reject plan
  for (let i = 0; i < 40; i++) { const s = JSON.parse((await http(srv.port, 'GET', '/api/state', { cookie })).body); if (!s.busy) { assert.equal(s.last.verdict, 'aborted'); break; } await new Promise((r) => setTimeout(r, 100)); }
  await srv.close(); await m.close();
});
