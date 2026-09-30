import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execSync } from 'node:child_process';
import { startMock, type MockServer, type Scripted } from './mock-vllm.ts';
import { loadConfig } from '../src/config.ts';
import { Engine, headlessIO, type IO } from '../src/engine.ts';
import { SkillStore } from '../src/skills/store.ts';
import { BUILTIN_TASKS } from '../src/eval/corpus.ts';
import { runOne, summarize } from '../src/eval/runner.ts';

const cfgFor = (m: MockServer) => ({ ...loadConfig('/x', {} as any), endpoint: m.url, metricsUrl: m.url.replace('/v1', '/metrics'), retries: 0, verifyRoundsNormal: 2, verifyRoundsAuto: 3 });

function fixture(id = 'js-off-by-one') {
  const t = BUILTIN_TASKS.find((x) => x.id === id)!;
  const d = mkdtempSync(join(tmpdir(), 'fh-e-'));
  for (const [f, b] of Object.entries(t.files)) { mkdirSync(join(d, f, '..'), { recursive: true }); writeFileSync(join(d, f), b); }
  execSync('git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -qm base', { cwd: d });
  return { d, t };
}

/** Routes a request the way a real model would: structured stages by schema, agent turns by history. */
function brain(script: { fixWith?: string; intake?: object; review?: object; mine?: object; fixSteps?: Scripted[] }) {
  return (req: any): Scripted => {
    const props = req.response_format?.json_schema?.schema?.properties;
    if (props?.trivial) return { content: JSON.stringify(script.intake ?? { trivial: true, questions: [], enriched: 'Fix sumRange to be inclusive', acceptance: ['tests pass'], plan: [{ step: 'fix loop bound', files: ['lib/math.js'] }], assumptions: [], subtasks: [] }) };
    if (props?.verdict) return { content: JSON.stringify(script.review ?? { verdict: 'pass', findings: [] }) };
    if (props?.create !== undefined) return { content: JSON.stringify(script.mine ?? { create: false }) };
    const msgs = req.messages;
    const last = msgs.at(-1);
    if (last.role === 'tool') return { content: 'Fixed the loop bound.' };
    if (last.role === 'user' && /Verification round/.test(last.content)) return { tool_calls: [{ name: 'edit', args: { path: 'lib/math.js', old_text: 'i < b; i++', new_text: 'i <= b; i++' } }] };
    if (script.fixWith === 'nothing') return { content: 'I made no changes.' };
    return { tool_calls: [{ name: 'edit', args: { path: 'lib/math.js', old_text: script.fixWith ?? 'i < b; i++', new_text: 'i <= b; i++' } }] };
  };
}

const quiet = (log: string[] = []): IO => headlessIO((m) => log.push(m));

test('engine e2e (auto): plan -> work -> verify passes; skill gate has no LLM call; final gated by verdict', async () => {
  const m = await startMock(); m.fallback = brain({});
  const { d } = fixture();
  const log: string[] = [];
  const engine = new Engine(cfgFor(m), quiet(log), { cwd: d, store: new SkillStore(':memory:') });
  const r = await engine.runTask('fix sumRange', { mode: 'auto', approval: 'yolo', noMine: true });
  assert.equal(r.verdict, 'pass'); assert.match(r.final, /^Verified/);
  assert.match(readFileSync(join(d, 'lib/math.js'), 'utf8'), /i <= b/);
  assert.equal(r.gate!.llmCalls, 0); assert.ok(r.gate!.ms < 50);
  assert.equal(r.rounds, 1); assert.deepEqual(r.changed, ['lib/math.js']);
  assert.ok(r.llm.requests >= 3); assert.ok(log.some((l) => /round 1: pass/.test(l)));
  // planner call precedes the agent and is the only pre-work LLM call
  assert.equal(m.requests[0].response_format.json_schema.schema.properties.trivial !== undefined, true);
  // stable prefix: identical system prompt on every agent request
  const sys = m.requests.filter((q: any) => !q.response_format).map((q: any) => q.messages[0].content);
  assert.equal(new Set(sys).size, 1);
  await m.close();
});

test('engine: failing verification -> fix round -> pass', async () => {
  const m = await startMock(); m.fallback = brain({ fixWith: 'i < b; i++' });
  // first attempt edits something harmless so tests still fail; fix round applies the real fix
  m.fallback = ((orig) => (req: any) => {
    const last = req.messages.at(-1);
    if (!req.response_format && last.role === 'user' && !/Verification round/.test(last.content)) return { tool_calls: [{ name: 'edit', args: { path: 'lib/math.js', old_text: 'let total = 0;', new_text: 'let total = 0; // start' } }] };
    return orig(req);
  })(brain({}));
  const { d } = fixture();
  const engine = new Engine(cfgFor(m), quiet(), { cwd: d, store: new SkillStore(':memory:') });
  const r = await engine.runTask('fix sumRange', { mode: 'auto', approval: 'yolo', noMine: true });
  assert.equal(r.verdict, 'pass'); assert.equal(r.rounds, 2);
  await m.close();
});

test('engine: unfixable -> verdict fail, changes rolled back, rejected patch saved, final says not delivered', async () => {
  const m = await startMock();
  m.fallback = ((orig) => (req: any) => {
    const last = req.messages.at(-1);
    if (!req.response_format && last.role !== 'tool') return { tool_calls: [{ name: 'edit', args: { path: 'lib/math.js', old_text: 'let total = 0;', new_text: 'let total = 1;' } }] };
    return orig(req);
  })(brain({}));
  const { d } = fixture();
  const original = readFileSync(join(d, 'lib/math.js'), 'utf8');
  const engine = new Engine(cfgFor(m), quiet(), { cwd: d, store: new SkillStore(':memory:') });
  const r = await engine.runTask('fix sumRange', { mode: 'auto', approval: 'yolo', noMine: true });
  assert.equal(r.verdict, 'fail'); assert.equal(r.rolledBack, true); assert.match(r.final, /NOT delivered/);
  assert.equal(readFileSync(join(d, 'lib/math.js'), 'utf8'), original);
  assert.ok(existsSync(join(d, r.rejectedPatch!)));
  await m.close();
});

test('engine: repo without tests is reported UNVERIFIED, never silently passed', async () => {
  const m = await startMock();
  m.fallback = (req: any) => {
    const p = req.response_format?.json_schema?.schema?.properties;
    if (p?.trivial) return { content: JSON.stringify({ trivial: true, questions: [], enriched: 'edit note', acceptance: [], plan: [{ step: 's', files: ['a.txt'] }], assumptions: [], subtasks: [] }) };
    if (p?.verdict) return { content: JSON.stringify({ verdict: 'pass', findings: [] }) };
    if (req.messages.at(-1).role === 'tool') return { content: 'done' };
    return { tool_calls: [{ name: 'edit', args: { path: 'a.txt', old_text: 'x', new_text: 'y' } }] };
  };
  const d = mkdtempSync(join(tmpdir(), 'fh-nt-')); writeFileSync(join(d, 'a.txt'), 'x');
  execSync('git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -qm b', { cwd: d });
  const r = await new Engine(cfgFor(m), quiet(), { cwd: d, store: new SkillStore(':memory:') }).runTask('edit', { mode: 'auto', approval: 'yolo', noMine: true });
  assert.equal(r.verdict, 'unverified'); assert.match(r.final, /UNVERIFIED/); assert.equal(readFileSync(join(d, 'a.txt'), 'utf8'), 'y');
  await m.close();
});

test('engine (guided): questions are asked, plan approved with one key; plan rejection aborts before any edit', async () => {
  const m = await startMock();
  let intakeCalls = 0;
  const b = brain({});
  m.fallback = (req: any) => {
    if (req.response_format?.json_schema?.schema?.properties?.trivial) {
      intakeCalls++;
      return { content: JSON.stringify(intakeCalls === 1 ? { trivial: false, questions: ['Inclusive or exclusive?'], enriched: 'x', acceptance: [], plan: [], assumptions: [], subtasks: [] } : { trivial: true, questions: ['ignored'], enriched: 'Make sumRange inclusive', acceptance: ['tests pass'], plan: [{ step: 'fix', files: ['lib/math.js'] }], assumptions: [], subtasks: [] }) };
    }
    return b(req);
  };
  const { d } = fixture();
  const asked: string[][] = [], plans: string[] = [];
  const io: IO = { ...headlessIO(), askQuestions: async (q) => { asked.push(q); return ['inclusive']; }, approvePlan: async (p) => { plans.push(p); return { ok: true }; } };
  const r = await new Engine(cfgFor(m), io, { cwd: d, store: new SkillStore(':memory:') }).runTask('fix', { mode: 'normal', approval: 'auto-edit', noMine: true });
  assert.deepEqual(asked, [['Inclusive or exclusive?']]); assert.equal(plans.length, 1); assert.match(plans[0], /fix/);
  assert.equal(r.verdict, 'pass');
  assert.match(JSON.stringify(m.requests[1].messages), /Q: Inclusive or exclusive\?[\s\S]*A: inclusive/);
  await m.close();
  const m2 = await startMock(); m2.fallback = brain({});
  const { d: d2 } = fixture(); const orig = readFileSync(join(d2, 'lib/math.js'), 'utf8');
  const r2 = await new Engine(cfgFor(m2), { ...headlessIO(), approvePlan: async () => ({ ok: false }) }, { cwd: d2, store: new SkillStore(':memory:') }).runTask('fix', { mode: 'normal', approval: 'auto-edit', noMine: true });
  assert.equal(r2.verdict, 'aborted'); assert.equal(readFileSync(join(d2, 'lib/math.js'), 'utf8'), orig);
  await m2.close();
});

test('engine: parallel workers on disjoint files (multi-file task) then merged verification', async () => {
  const { d } = fixture('js-two-modules');
  const m = await startMock();
  m.fallback = (req: any) => {
    const p = req.response_format?.json_schema?.schema?.properties;
    if (p?.trivial) return { content: JSON.stringify({ trivial: false, questions: [], enriched: 'implement both', acceptance: ['tests pass'], plan: [{ step: 'a', files: ['server/validate.js'] }, { step: 'b', files: ['client/format.js'] }], assumptions: [], subtasks: [{ id: 'server', goal: 'implement isEmail in server/validate.js', files: ['server/**'], deps: [] }, { id: 'client', goal: 'implement formatCents in client/format.js', files: ['client/**'], deps: [] }] }) };
    if (p?.verdict) return { content: JSON.stringify({ verdict: 'pass', findings: [] }) };
    const user = req.messages.find((x: any) => x.role === 'user')?.content as string;
    if (req.messages.at(-1).role === 'tool') return { content: 'done' };
    if (user.includes('(server)')) return { tool_calls: [{ name: 'edit', args: { path: 'server/validate.js', old_text: "throw new Error('todo');", new_text: "return typeof s === 'string' && /^[^@\\s]+@[^@\\s.]+\\.[^@\\s]+$/.test(s);" } }] };
    if (user.includes('(client)')) return { tool_calls: [{ name: 'edit', args: { path: 'client/format.js', old_text: "throw new Error('todo');", new_text: "return '$' + (n / 100).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });" } }] };
    return { content: 'noop' };
  };
  const log: string[] = [];
  const r = await new Engine({ ...cfgFor(m), maxConcurrency: 2 }, quiet(log), { cwd: d, store: new SkillStore(':memory:') }).runTask('two changes', { mode: 'auto', approval: 'yolo', noMine: true });
  assert.equal(r.verdict, 'pass', JSON.stringify(r.verify?.rounds.at(-1)?.checks));
  assert.equal(r.workers!.length, 2); assert.ok(log.some((l) => /wave 1\/1: 2 worker/.test(l)));
  assert.deepEqual(r.changed.sort(), ['client/format.js', 'server/validate.js']);
  await m.close();
});

test('engine: post-delivery mining creates a project skill after a verified pass, activity is logged, and the next task uses it', async () => {
  const m = await startMock();
  const body = '- Keep loop bounds inclusive when the API contract says inclusive.\n- Add a boundary test for equal endpoints.\n- Prefer <= over adjusting the end value.';
  m.fallback = brain({ mine: { create: true, name: 'Inclusive ranges', summary: 'range helpers are inclusive', keywords: ['range', 'sum', 'inclusive', 'loop'], body } });
  const { d } = fixture(); const store = new SkillStore(':memory:');
  const engine = new Engine(cfgFor(m), quiet(), { cwd: d, store });
  const r = await engine.runTask('fix sumRange', { mode: 'auto', approval: 'yolo' });
  await engine.drain();
  assert.equal(r.verdict, 'pass');
  assert.ok(store.activity().some((a) => a.kind === 'created' && a.skill === 'Inclusive ranges'));
  assert.equal(store.userSkills().length, 1);
  // hidden: the skill body never appears in the task result the user sees
  assert.doesNotMatch(JSON.stringify({ final: r.final, plan: r.plan }), /Keep loop bounds/);
  execSync('git checkout -q -- . && git clean -fdq', { cwd: d });
  m.requests.length = 0;
  const r2 = await engine.runTask('fix the sumRange inclusive loop', { mode: 'auto', approval: 'yolo', noMine: true });
  assert.ok(r2.skillsUsed.includes('Inclusive ranges'));
  assert.match(JSON.stringify(m.requests.find((q: any) => !q.response_format).messages), /Keep loop bounds inclusive/);
  await m.close();
});

test('eval runner: builtin corpus is well-formed (fails before, oracle command runs) and runOne solves a task via the engine', async () => {
  for (const t of BUILTIN_TASKS) {
    const d = mkdtempSync(join(tmpdir(), 'fh-c-'));
    for (const [f, b] of Object.entries(t.files)) { mkdirSync(join(d, f, '..'), { recursive: true }); writeFileSync(join(d, f), b); }
    let code = 0; try { execSync(t.oracle, { cwd: d, stdio: 'ignore', env: { ...process.env, NODE_TEST_CONTEXT: undefined } }); } catch { code = 1; }
    assert.equal(code, 1, `${t.id} must fail before the fix`);
  }
  const m = await startMock(); m.fallback = brain({});
  const row = await runOne(cfgFor(m), BUILTIN_TASKS[0], 'fh', 1, { qwenCmd: 'qwen', auto: true });
  assert.equal(row.solved, true); assert.equal(row.verdict, 'pass'); assert.equal(row.malformed, 0);
  assert.match(summarize([row]), /\| fh \| 1 \| 100\.0% \|/);
  await m.close();
});
