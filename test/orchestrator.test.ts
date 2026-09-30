import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, mkdirSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Governor } from '../src/orchestrator/governor.ts';
import { globsOverlap, planWaves } from '../src/orchestrator/partition.ts';
import { ownersOf, runWorkers } from '../src/orchestrator/master.ts';
import { startMock } from './mock-vllm.ts';
import { LlmClient } from '../src/llm/client.ts';
import { loadConfig } from '../src/config.ts';

const st = (id: string, files: string[], deps: string[] = []) => ({ id, goal: id, files, deps });

test('globsOverlap', () => {
  assert.ok(globsOverlap('src/**', 'src/a/b.ts')); assert.ok(globsOverlap('src/a/**', 'src/**'));
  assert.ok(!globsOverlap('src/a/**', 'src/b/**')); assert.ok(!globsOverlap('a.ts', 'b.ts')); assert.ok(globsOverlap('a.ts', 'a.ts'));
  assert.ok(!globsOverlap('srcx/**', 'src/**'));
});

test('planWaves: disjoint run in parallel; overlap and deps are serialized; bad plans fall back to serial', () => {
  assert.deepEqual(planWaves([st('a', ['api/**']), st('b', ['web/**'])]).map((w) => w.map((s) => s.id)), [['a', 'b']]);
  assert.deepEqual(planWaves([st('a', ['src/**']), st('b', ['src/x.ts'])]).map((w) => w.map((s) => s.id)), [['a'], ['b']]);
  assert.deepEqual(planWaves([st('a', ['api/**']), st('b', ['web/**'], ['a'])]).map((w) => w.map((s) => s.id)), [['a'], ['b']]);
  assert.deepEqual(planWaves([st('a', ['x/**'], ['b']), st('b', ['y/**'], ['a'])]).map((w) => w.length), [1, 1]); // cycle
  assert.deepEqual(planWaves([st('a', [])]).length, 1);
});

test('governor: limit follows KV/queue pressure and bounds concurrency', async () => {
  assert.equal(Governor.next(3, 4, { kvUsage: 0.9 }), 2);
  assert.equal(Governor.next(3, 4, { kvUsage: 0.3, waiting: 0 }), 4);
  assert.equal(Governor.next(3, 4, { kvUsage: 0.3, waiting: 2 }), 2);
  assert.equal(Governor.next(1, 4, { kvUsage: 0.95 }), 1);
  assert.equal(Governor.next(2, 4, undefined), 2);
  const g = new Governor(3, async () => ({ kvUsage: 0.2, waiting: 0, raw: {} }), 1);
  let running = 0, peak = 0;
  const work = () => g.run(async () => { running++; peak = Math.max(peak, running); await new Promise((r) => setTimeout(r, 30)); running--; });
  await g.tick(); await g.tick(); // limit -> 3
  await Promise.all([1, 2, 3, 4, 5, 6].map(work));
  assert.ok(peak <= 3 && peak >= 2, `peak ${peak}`);
});

test('workers: parallel workers respect file ownership; a worker cannot edit another worker\'s files', async () => {
  const d = mkdtempSync(join(tmpdir(), 'fh-o-')); mkdirSync(join(d, 'api')); mkdirSync(join(d, 'web'));
  writeFileSync(join(d, 'api/a.txt'), 'A'); writeFileSync(join(d, 'web/b.txt'), 'B');
  const m = await startMock();
  m.fallback = (req) => {
    const task = req.messages.find((x: any) => x.role === 'user')?.content as string;
    const last = req.messages.at(-1);
    if (last.role === 'tool') return { content: last.content.includes('ownership') ? 'blocked by ownership' : 'done' };
    if (task.includes('subtask (w1)')) return { tool_calls: [{ name: 'edit', args: { path: 'api/a.txt', old_text: 'A', new_text: 'A1' } }] };
    return { tool_calls: [{ name: 'edit', args: { path: 'api/a.txt', old_text: 'A', new_text: 'HACK' } }] }; // w2 tries to edit w1's file
  };
  const llm = new LlmClient({ ...loadConfig('/x', {} as any), endpoint: m.url, retries: 0 });
  const rs = await runWorkers('goal', [st('w1', ['api/**']), st('w2', ['web/**'])], { llm, cwd: d, mode: 'yolo', context: '', governor: new Governor(2) });
  assert.equal(readFileSync(join(d, 'api/a.txt'), 'utf8'), 'A1');
  assert.equal(rs.length, 2);
  assert.match(rs.find((r) => r.id === 'w2')!.summary, /ownership/);
  assert.deepEqual(ownersOf([st('w1', ['api/**']), st('w2', ['web/**'])], ['api/a.txt', 'web/b.txt', 'x.txt']).unowned, ['x.txt']);
  await m.close();
});
