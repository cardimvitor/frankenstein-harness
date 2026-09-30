import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { startMock } from './mock-vllm.ts';
import { loadConfig } from '../src/config.ts';
import { validateVllm, filler } from '../src/validate/vllm.ts';
import { main, parseArgs } from '../src/ui/cli.ts';

test('filler produces roughly the requested size deterministically', () => {
  assert.equal(filler(1000, 3), filler(1000, 3));
  assert.ok(filler(1000, 3).length > 3000 && filler(1000, 3).length < 4200);
});

test('validate-vllm runs every probe against a scripted server and writes json+markdown reports', async () => {
  const m = await startMock();
  let reqs = 0, queries = 0, hits = 0; const seen = new Set<string>();
  m.metricsFn = () => `vllm:kv_cache_usage_perc 0.2\nvllm:num_requests_running 0\nvllm:num_requests_waiting 0\nvllm:prefix_cache_queries_total ${queries}\nvllm:prefix_cache_hits_total ${hits}\nvllm:spec_decode_num_drafts_total ${reqs * 10}\nvllm:spec_decode_num_draft_tokens_total ${reqs * 30}\nvllm:spec_decode_num_accepted_tokens_total ${reqs * 20}\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position="0"} ${reqs * 9}\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position="1"} ${reqs * 7}\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position="2"} ${reqs * 4}\n`;
  m.fallback = (req: any) => {
    reqs++;
    const text = req.messages.map((x: any) => x.content).join('\n') as string;
    const key = text.slice(0, 200); queries += 100; if (seen.has(key)) hits += 90; seen.add(key);
    const needle = text.match(/passphrase is (KX-\d+)/)?.[1];
    if (needle) return { content: needle };
    if (/Read the file/.test(text)) return { tool_calls: [{ name: 'read_file', args: { path: 'src/app.ts' } }] };
    if (/two-line snippet/.test(text)) return { tool_calls: [{ name: 'edit', args: { path: 'lib/util.js', old_text: 'const a = 1;\nconst b = "two";', new_text: 'const a = 10;\nconst b = "three"; // updated' } }] };
    if (/Run this exact shell/.test(text)) return { tool_calls: [{ name: 'bash', args: { command: 'grep -rn "TODO(\\"x\\")" src | head -5' } }] };
    if (/Create the new file/.test(text)) return { tool_calls: [{ name: 'write_file', args: { path: 'docs/notes.md', content: 'Olá, mundo — “aspas”' } }] };
    if (/Search the repo/.test(text)) return { tool_calls: [{ name: 'grep', args: { pattern: 'function\\s+\\w+\\(' } }] };
    if (/Think carefully/.test(text)) return { reasoning: 'hmm', tool_calls: [{ name: 'read_file', args: { path: 'src/parser.ts' } }] };
    if (req.chat_template_kwargs?.enable_thinking) return { reasoning: 'thinking', content: 'answer' };
    return { content: '{"items":[]}' };
  };
  const out = mkdtempSync(join(tmpdir(), 'fh-r-'));
  const cfg = { ...loadConfig('/x', {} as any), endpoint: m.url, metricsUrl: m.url.replace('/v1', '/metrics'), model: 'mock-qwen', retries: 0 };
  const code = await validateVllm(cfg, { out, quick: true, trials: 10, maxContext: 16000, concurrency: 2 });
  const dir = join(out, readdirSync(out)[0]);
  const rep = JSON.parse(readFileSync(join(dir, 'report.json'), 'utf8'));
  const by = Object.fromEntries(rep.results.map((r: any) => [r.name.split(' (')[0], r]));
  assert.equal(code, 0, JSON.stringify(rep.results.filter((r: any) => r.status === 'fail')));
  assert.equal(by['connectivity and auth'].status, 'warn'); // the mock has no auth, so it is reported as unprotected
  assert.match(by['connectivity and auth'].summary, /NOT protected/);
  assert.equal(by['metrics endpoint'].status, 'pass');
  assert.match(by['tool-call reliability'].summary, /malformed 0\.0%/);
  assert.equal(by['tool-call reliability'].data.right, by['tool-call reliability'].data.total);
  assert.equal(by['thinking must not leak into tool arguments or content'].status, 'pass');
  assert.equal(by['streamed vs non-streamed tool call'].status, 'pass');
  assert.match(by['MTP acceptance'].summary, /66\.7%/); // 20/30 accepted
  assert.equal(by['prefix cache'].status, 'pass');
  assert.match(by['long-context retrieval and latency'].summary, /ok/); assert.doesNotMatch(by['long-context retrieval and latency'].summary, /MISS/);
  assert.ok(by['concurrency and KV pressure'].data.recommended >= 1);
  assert.match(readFileSync(join(dir, 'report.md'), 'utf8'), /\| tool-call reliability/);
  await m.close();
});

test('validate-vllm fails fast with a clear message when the model is not served', async () => {
  const m = await startMock();
  const cfg = { ...loadConfig('/x', {} as any), endpoint: m.url, model: 'wrong-model', retries: 0 };
  const out = mkdtempSync(join(tmpdir(), 'fh-r-'));
  assert.equal(await validateVllm(cfg, { out, quick: true }), 1);
  const rep = JSON.parse(readFileSync(join(out, readdirSync(out)[0], 'report.json'), 'utf8'));
  assert.match(rep.results[0].summary, /not served; available: mock-qwen/);
  await m.close();
});

test('cli argument parsing', () => {
  const a = parseArgs(['run', 'fix', 'the', 'bug', '--auto', '--mode', 'yolo', '--cwd=/tmp/x', '--json']);
  assert.equal(a.cmd, 'run'); assert.deepEqual(a.positional, ['fix', 'the', 'bug']);
  assert.deepEqual(a.flags, { auto: true, mode: 'yolo', cwd: '/tmp/x', json: true });
});

test('cli: help and bad usage return codes', async () => {
  const log = console.log; console.log = () => {}; const err = console.error; console.error = () => {};
  try {
    assert.equal(await main(['--help']), 0);
    assert.equal(await main(['run']), 2);
    assert.equal(await main(['run', 'x', '--mode', 'nope']), 2);
    assert.equal(await main(['bogus']), 2);
  } finally { console.log = log; console.error = err; }
});
