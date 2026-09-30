import { test } from 'node:test';
import assert from 'node:assert/strict';
import { startMock } from './mock-vllm.ts';
import { LlmClient } from '../src/llm/client.ts';
import { loadConfig } from '../src/config.ts';
import { closeJson, extractContentCalls, parseArgs } from '../src/llm/repair.ts';
import { delta, parsePrometheus, summarize } from '../src/llm/metrics.ts';

const cfgFor = (url: string) => ({ ...loadConfig('/nonexistent', {} as any), endpoint: url, metricsUrl: url.replace('/v1', '/metrics'), retries: 2 });

test('parseArgs: valid, trailing comma, truncated, fenced, think leak', () => {
  assert.deepEqual(parseArgs('{"a":1}').value, { a: 1 });
  assert.equal(parseArgs('{"a":1}').repaired, false);
  assert.deepEqual(parseArgs('{"a":1,}').value, { a: 1 });
  assert.deepEqual(parseArgs('{"path":"x.ts","n":[1,2').value, { path: 'x.ts', n: [1, 2] });
  assert.deepEqual(parseArgs('{"s":"abc').value, { s: 'abc' });
  assert.deepEqual(parseArgs('```json\n{"a":2}\n```').value, { a: 2 });
  const t = parseArgs('<think>hmm</think>{"a":3}');
  assert.deepEqual(t.value, { a: 3 });
  assert.equal(t.thinkLeak, true);
  assert.equal(parseArgs('not json at all').ok, false);
  assert.equal(closeJson('{"a":[1,2,'), '{"a":[1,2]}');
});

test('extractContentCalls: json and qwen3_coder xml', () => {
  const a = extractContentCalls('hi <tool_call>{"name":"read_file","arguments":{"path":"a"}}</tool_call>');
  assert.equal(a.calls[0].name, 'read_file');
  assert.deepEqual(a.calls[0].args, { path: 'a' });
  assert.equal(a.rest, 'hi');
  const b = extractContentCalls('<tool_call><function=edit><parameter=path>a.ts</parameter><parameter=old>x\ny</parameter></function></tool_call>');
  assert.equal(b.calls[0].name, 'edit');
  assert.deepEqual(b.calls[0].args, { path: 'a.ts', old: 'x\ny' });
});

test('client streams content, reasoning and tool calls; sets enable_thinking', async () => {
  const m = await startMock();
  m.queue.push({ reasoning: 'thinking here', content: 'hello world', tool_calls: [{ name: 'read_file', args: { path: 'a.ts' } }] });
  const c = new LlmClient(cfgFor(m.url));
  let streamed = '';
  const r = await c.chat({ messages: [{ role: 'user', content: 'x' }], thinking: 'high', onContent: (d) => (streamed += d) });
  assert.equal(r.content, 'hello world');
  assert.equal(streamed, 'hello world');
  assert.equal(r.reasoning, 'thinking here');
  assert.equal(r.toolCalls[0].name, 'read_file');
  assert.deepEqual(r.toolCalls[0].args, { path: 'a.ts' });
  assert.equal(r.toolCalls[0].parse, 'ok');
  assert.equal(m.requests[0].chat_template_kwargs.enable_thinking, true);
  assert.equal(m.requests[0].top_k, 20);
  await c.chat({ messages: [{ role: 'user', content: 'y' }], thinking: 'off' });
  assert.equal(m.requests[1].chat_template_kwargs.enable_thinking, false);
  assert.equal(c.stats.requests, 2);
  await m.close();
});

test('client repairs malformed streamed args and counts them', async () => {
  const m = await startMock();
  m.queue.push({ tool_calls: [{ name: 'bash', args: '{"command":"ls -la"' }] });
  const c = new LlmClient(cfgFor(m.url));
  const r = await c.chat({ messages: [{ role: 'user', content: 'x' }] });
  assert.equal(r.toolCalls[0].parse, 'repaired');
  assert.deepEqual(r.toolCalls[0].args, { command: 'ls -la' });
  assert.equal(c.stats.repaired, 1);
  assert.equal(c.stats.malformed, 0);
  await m.close();
});

test('client recovers tool call left in content', async () => {
  const m = await startMock();
  m.queue.push({ content: '<tool_call>{"name":"grep","arguments":{"pattern":"foo"}}</tool_call>' });
  const c = new LlmClient(cfgFor(m.url));
  const r = await c.chat({ messages: [{ role: 'user', content: 'x' }] });
  assert.equal(r.toolCalls[0].name, 'grep');
  assert.equal(r.toolCalls[0].fromContent, true);
  assert.equal(r.content, '');
  await m.close();
});

test('client retries 5xx then succeeds; 401 is not retried', async () => {
  const m = await startMock();
  m.queue.push({ status: 503 }, { content: 'fine' });
  const c = new LlmClient({ ...cfgFor(m.url) });
  const r = await c.chat({ messages: [{ role: 'user', content: 'x' }] });
  assert.equal(r.content, 'fine');
  assert.equal(c.stats.retries, 1);
  await m.close();
  const m2 = await startMock({ apiKey: 'secret-key-123' });
  const c2 = new LlmClient(cfgFor(m2.url));
  await assert.rejects(() => c2.chat({ messages: [{ role: 'user', content: 'x' }] }), /401/);
  assert.equal(m2.requests.length, 0);
  process.env.FH_API_KEY = 'secret-key-123';
  const ok = await c2.chat({ messages: [{ role: 'user', content: 'x' }] });
  assert.equal(ok.content, 'ok');
  delete process.env.FH_API_KEY;
  await m2.close();
});

test('client abort cancels the request', async () => {
  const m = await startMock();
  m.queue.push({ delayMs: 2000, content: 'late' });
  const c = new LlmClient(cfgFor(m.url));
  const ac = new AbortController();
  setTimeout(() => ac.abort(), 50);
  await assert.rejects(() => c.chat({ messages: [{ role: 'user', content: 'x' }], signal: ac.signal }), (e: Error) => e.name === 'AbortError' || /abort/i.test(e.message));
  await m.close();
});

test('metrics parse and delta', () => {
  const t1 = 'vllm:prefix_cache_queries_total{model="m"} 100\nvllm:prefix_cache_hits_total{model="m"} 10\nvllm:spec_decode_num_drafts_total 10\nvllm:spec_decode_num_draft_tokens_total 30\nvllm:spec_decode_num_accepted_tokens_total 12\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position="0"} 8\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position="1"} 3\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position="2"} 1\nvllm:kv_cache_usage_perc 0.4\n';
  const t2 = t1.replace('100', '200').replace('} 10\n', '} 60\n').replace('drafts_total 10', 'drafts_total 20').replace('draft_tokens_total 30', 'draft_tokens_total 60').replace('accepted_tokens_total 12', 'accepted_tokens_total 30').replace('position="0"} 8', 'position="0"} 17');
  const a = summarize(parsePrometheus(t1)), b = summarize(parsePrometheus(t2));
  assert.equal(a.kvUsage, 0.4);
  assert.deepEqual(a.specAcceptedPerPos, [8, 3, 1]);
  const d = delta(a, b);
  assert.equal(d.acceptanceRate, 18 / 30);
  assert.equal(d.prefixHitRate, 0.5);
  assert.equal(d.meanAcceptedPerDraft, 1.8);
  assert.equal(d.perPositionAcceptance![0], 0.9);
});
