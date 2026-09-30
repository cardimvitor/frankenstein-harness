import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, mkdirSync, rmSync, existsSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execSync } from 'node:child_process';
import { Checkpoints } from '../src/session/checkpoint.ts';
import { fingerprint, similarity } from '../src/fingerprint.ts';
import { diffChecks, parseDiffAdded } from '../src/verify/checks.ts';
import { validateFindings } from '../src/verify/reviewer.ts';
import { verifyLoop } from '../src/verify/rounds.ts';
import { findSecrets } from '../src/verify/secrets.ts';
import { startMock } from './mock-vllm.ts';
import { LlmClient } from '../src/llm/client.ts';
import { loadConfig } from '../src/config.ts';

function repo(files: Record<string, string>) {
  const d = mkdtempSync(join(tmpdir(), 'fh-v-'));
  for (const [k, v] of Object.entries(files)) { mkdirSync(join(d, k, '..'), { recursive: true }); writeFileSync(join(d, k), v); }
  execSync('git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -qm init', { cwd: d });
  return d;
}

test('checkpoint: snapshot, diff, restore (modified, added, deleted) without touching index', async () => {
  const d = repo({ 'a.txt': 'one', 'b.txt': 'two' });
  const cp = new Checkpoints(d);
  const id = (await cp.create('t'))!;
  writeFileSync(join(d, 'a.txt'), 'changed'); rmSync(join(d, 'b.txt')); writeFileSync(join(d, 'new.txt'), 'n');
  const ch = await cp.changedSince(id);
  assert.deepEqual([ch.added, ch.modified, ch.deleted], [['new.txt'], ['a.txt'], ['b.txt']]);
  assert.match(await cp.diffSince(id), /\+changed/);
  await cp.restore(id);
  assert.equal(readFileSync(join(d, 'a.txt'), 'utf8'), 'one'); assert.equal(readFileSync(join(d, 'b.txt'), 'utf8'), 'two'); assert.equal(existsSync(join(d, 'new.txt')), false);
  assert.equal(execSync('git status --porcelain', { cwd: d }).toString().trim(), ''); // user's index/worktree untouched
});

test('diffChecks: scope, secrets, deleted/skipped tests, conflict markers', async () => {
  const d = repo({ 'src/a.ts': 'x', 'test/a.test.ts': "it('a', ()=>{})" });
  const cp = new Checkpoints(d); const id = (await cp.create('t'))!;
  writeFileSync(join(d, 'src/a.ts'), 'const k = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";\n<<<<<<< HEAD\n');
  rmSync(join(d, 'test/a.test.ts')); writeFileSync(join(d, 'test/b.test.ts'), "it.skip('b', ()=>{})"); writeFileSync(join(d, 'other.txt'), 'o');
  const r = await diffChecks(d, cp, id, { allowedGlobs: ['src/**'] });
  const by = Object.fromEntries(r.results.map((x) => [x.name, x]));
  assert.equal(by['diff scope'].status, 'fail'); assert.match(by['diff scope'].detail, /other\.txt/);
  assert.equal(by['secret scan'].status, 'fail');
  assert.equal(by['test integrity'].status, 'fail'); assert.match(by['test integrity'].detail, /deleted test file/); assert.match(by['test integrity'].detail, /skipped/);
  assert.equal(by['patch sanity'].status, 'fail');
  assert.ok(parseDiffAdded('+++ b/x\n@@ -1 +5,2 @@\n+a\n b\n+c').map((a) => a.line).join() === '5,7');
});

test('findings without a verifiable citation are dropped', () => {
  const d = repo({ 'a.ts': 'line1\nconst total = a + b;\nline3\n' });
  const { valid, dropped } = validateFindings(d, [
    { file: 'a.ts', line: 2, severity: 'blocker', claim: 'bad', quote: 'const total = a + b;' },
    { file: 'a.ts', line: 2, severity: 'blocker', claim: 'hallucinated quote', quote: 'something else entirely' },
    { file: 'a.ts', line: 99, severity: 'major', claim: 'oob', quote: 'x y z' },
    { file: 'nope.ts', line: 1, severity: 'major', claim: 'unknown file', quote: 'line1' },
    { file: 'a.ts', severity: 'major', claim: 'no line' },
  ], ['a.ts']);
  assert.equal(valid.length, 1); assert.equal(dropped, 4);
});

test('secrets', () => { assert.ok(findSecrets('token = "abcdefghijklmnopqrstuvwx1234"').length); assert.equal(findSecrets('const a = 1').length, 0); });

test('fingerprint: dotnet framework, dotnet 8, react/angular/angularjs, verify commands', () => {
  const a = repo({ 'App/App.csproj': '<Project><PropertyGroup><TargetFrameworkVersion>v4.8</TargetFrameworkVersion></PropertyGroup></Project>', 'App.sln': '' });
  assert.deepEqual(fingerprint(a).stacks.map((s) => `${s.id}@${s.version}`), ['dotnet-framework@48']);
  const b = repo({ 'x/x.csproj': '<Project><PropertyGroup><TargetFramework>net8.0</TargetFramework></PropertyGroup></Project>', 'package.json': JSON.stringify({ dependencies: { react: '^18.2.0' }, scripts: { test: 'vitest', build: 'vite build' } }) });
  const fb = fingerprint(b);
  assert.ok(fb.stacks.some((s) => s.id === 'dotnet' && s.version === '8')); assert.ok(fb.stacks.some((s) => s.id === 'react' && s.version === '18'));
  assert.ok(fb.verify.some((v) => v.kind === 'build') && fb.verify.some((v) => v.name === 'dotnet build'));
  const c = repo({ 'package.json': JSON.stringify({ dependencies: { angular: '1.8.2' } }) });
  assert.ok(fingerprint(c).stacks.some((s) => s.id === 'angularjs'));
  const e = repo({ 'package.json': JSON.stringify({ dependencies: { '@angular/core': '^17.1.0' } }) });
  assert.ok(fingerprint(e).stacks.some((s) => s.id === 'angular' && s.version === '17'));
  assert.ok(similarity(fb.tokens, fingerprint(b).tokens) === 1); assert.ok(similarity(fb.tokens, fingerprint(c).tokens) < 0.5);
});

test('verifyLoop: fail -> fix -> pass; deterministic decides; unverified without tests', async () => {
  const d = repo({ 'package.json': JSON.stringify({ scripts: { test: 'node check.js' } }), 'check.js': "process.exit(require('fs').readFileSync('v.txt','utf8').trim()==='ok'?0:1)", 'v.txt': 'bad' });
  const cp = new Checkpoints(d); const base = (await cp.create('t'))!;
  writeFileSync(join(d, 'v.txt'), 'still bad');
  let fixes = 0;
  const rep = await verifyLoop({
    cwd: d, cp, base, fp: fingerprint(d), acceptance: [], maxRounds: 3,
    fix: async (fb, r) => { fixes++; assert.match(fb, /CHECK FAILED: test/); writeFileSync(join(d, 'v.txt'), 'ok'); },
  });
  assert.equal(rep.verdict, 'pass'); assert.equal(rep.rounds.length, 2); assert.equal(fixes, 1);
  const d2 = repo({ 'a.txt': 'x' }); const cp2 = new Checkpoints(d2); const b2 = (await cp2.create('t'))!;
  writeFileSync(join(d2, 'a.txt'), 'y');
  const rep2 = await verifyLoop({ cwd: d2, cp: cp2, base: b2, fp: fingerprint(d2), acceptance: [], maxRounds: 1, fix: async () => {} });
  assert.equal(rep2.verdict, 'unverified');
  const rep3 = await verifyLoop({ cwd: d, cp, base, fp: fingerprint(d), acceptance: [], maxRounds: 2, fix: async () => { /* does not fix */ } });
  // d is now passing state (v.txt=ok) so passes
  assert.equal(rep3.verdict, 'pass');
});

test('verifyLoop: reviewer blocker with valid citation forces a fix round; hallucinated one is ignored', async () => {
  const d = repo({ 'package.json': JSON.stringify({ scripts: { test: 'node -e 0' } }), 'a.js': 'const x = 1;\n' });
  const cp = new Checkpoints(d); const base = (await cp.create('t'))!;
  writeFileSync(join(d, 'a.js'), 'const x = 1;\nconst y = x / 0;\n');
  const m = await startMock();
  const good = { verdict: 'fail', findings: [{ file: 'a.js', line: 2, severity: 'blocker', claim: 'division by zero', quote: 'const y = x / 0;' }, { file: 'a.js', line: 1, severity: 'blocker', claim: 'invented', quote: 'nonexistent text' }] };
  m.queue.push({ content: JSON.stringify(good) }, { content: JSON.stringify({ verdict: 'pass', findings: [] }) });
  const llm = new LlmClient({ ...loadConfig('/x', {} as any), endpoint: m.url, retries: 0 });
  let fb = '';
  const rep = await verifyLoop({ cwd: d, cp, base, fp: fingerprint(d), llm, acceptance: ['no crashes'], maxRounds: 3, fix: async (f) => { fb = f; writeFileSync(join(d, 'a.js'), 'const x = 1;\nconst y = x / 2;\n'); } });
  assert.match(fb, /a\.js:2/); assert.doesNotMatch(fb, /invented/);
  assert.equal(rep.verdict, 'pass'); assert.equal(rep.reviewer.dropped, 1); assert.equal(rep.reviewer.valid, 1);
  assert.equal(m.requests[0].response_format.type, 'json_schema');
  await m.close();
});
