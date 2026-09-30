import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { authHeaders, type Config } from '../config.ts';
import { LlmClient } from '../llm/client.ts';
import { delta, fetchMetrics, type VllmMetrics } from '../llm/metrics.ts';
import { ALL_TOOLS } from '../tools/index.ts';
import type { ToolSpec } from '../types.ts';

export type Status = 'pass' | 'warn' | 'fail' | 'info' | 'skipped';
export interface Probe { name: string; status: Status; summary: string; data?: Record<string, unknown> }

export interface ValidateOptions { out: string; quick?: boolean; maxContext?: number; trials?: number; concurrency?: number }

const TOOLS: ToolSpec[] = ALL_TOOLS.map((t) => t.spec);
const pct = (x: number) => `${(100 * x).toFixed(1)}%`;
const median = (a: number[]) => { const s = [...a].sort((x, y) => x - y); return s.length ? s[Math.floor(s.length / 2)] : 0; };
const p95 = (a: number[]) => { const s = [...a].sort((x, y) => x - y); return s.length ? s[Math.min(s.length - 1, Math.ceil(0.95 * s.length) - 1)] : 0; };
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Deterministic filler of roughly `tokens` tokens; the real count is read back from usage. */
export function filler(tokens: number, seed = 0): string {
  const out: string[] = [];
  let chars = 0;
  for (let i = 0; chars < tokens * 3.4; i++) {
    const line = `Record ${seed}-${i}: unit=${(i * 7919 + seed) % 997} status=${['ready', 'queued', 'done', 'held'][(i + seed) % 4]} note=alpha${i % 13} beta${(i * 3) % 17}.`;
    out.push(line); chars += line.length + 1;
  }
  return out.join('\n');
}

const TOOL_SCENARIOS: { name: string; prompt: string; expect: string; required: string[] }[] = [
  { name: 'read', prompt: 'Read the file src/app.ts using the tool.', expect: 'read_file', required: ['path'] },
  { name: 'edit-multiline', prompt: 'In lib/util.js replace the two-line snippet "const a = 1;\\nconst b = \\"two\\";" with "const a = 10;\\nconst b = \\"three\\"; // updated". Use the edit tool.', expect: 'edit', required: ['path', 'old_text', 'new_text'] },
  { name: 'bash-quotes', prompt: 'Run this exact shell command with the bash tool: grep -rn "TODO(\\"x\\")" src | head -5', expect: 'bash', required: ['command'] },
  { name: 'unicode-write', prompt: 'Create the new file docs/notes.md containing the text: Olá, mundo — “aspas” and a tab\\there. Use write_file.', expect: 'write_file', required: ['path', 'content'] },
  { name: 'grep', prompt: 'Search the repo for the regex function\\s+\\w+\\( in *.ts files using grep.', expect: 'grep', required: ['pattern'] },
];

async function rawChat(cfg: Config, body: Record<string, unknown>, signal?: AbortSignal) {
  const res = await fetch(`${cfg.endpoint}/chat/completions`, { method: 'POST', headers: { 'content-type': 'application/json', ...authHeaders(cfg) }, body: JSON.stringify({ model: cfg.model, ...body }), signal: signal ?? AbortSignal.timeout(300_000) });
  return { status: res.status, json: await res.json().catch(() => ({})) as any };
}

async function probe(name: string, fn: () => Promise<Omit<Probe, 'name'>>): Promise<Probe> {
  const t0 = Date.now();
  process.stdout.write(`• ${name} … `);
  try {
    const r = await fn();
    console.log(`${r.status} (${((Date.now() - t0) / 1000).toFixed(1)}s) ${r.summary}`);
    return { name, ...r };
  } catch (e) {
    const msg = (e as Error).message;
    console.log(`fail ${msg}`);
    return { name, status: 'fail', summary: msg };
  }
}

export async function validateVllm(cfg: Config, o: ValidateOptions): Promise<number> {
  const llm = new LlmClient({ ...cfg, retries: 1 });
  const results: Probe[] = [];
  const add = async (name: string, fn: () => Promise<Omit<Probe, 'name'>>) => { results.push(await probe(name, fn)); };
  console.log(`Validating ${cfg.model} @ ${cfg.endpoint}${o.quick ? ' (quick)' : ''}\n`);

  let maxLen = 0;
  await add('connectivity and auth', async () => {
    const r = await fetch(`${cfg.endpoint}/models`, { headers: authHeaders(cfg), signal: AbortSignal.timeout(10_000) });
    if (!r.ok) return { status: 'fail', summary: `/models HTTP ${r.status}` };
    const j: any = await r.json();
    const m = (j.data ?? []).find((x: any) => x.id === cfg.model);
    if (!m) return { status: 'fail', summary: `model "${cfg.model}" not served; available: ${(j.data ?? []).map((x: any) => x.id).join(', ')}` };
    maxLen = m.max_model_len ?? 0;
    const noAuth = cfg.authScheme === 'none' ? undefined : await fetch(`${cfg.endpoint}/models`, { signal: AbortSignal.timeout(10_000) }).then((x) => x.status).catch(() => 0);
    return { status: noAuth === 200 ? 'warn' : 'pass', summary: `model ok, max_model_len ${maxLen || 'unknown'}${noAuth !== undefined ? `, unauthenticated request -> HTTP ${noAuth}${noAuth === 200 ? ' (endpoint is NOT protected)' : ''}` : ''}`, data: { maxLen, unauthenticatedStatus: noAuth } };
  });
  if (results[0].status === 'fail') return finish(results, cfg, o);

  const m0 = await fetchMetrics(cfg);
  await add('metrics endpoint', async () => {
    if (!m0) return { status: 'warn', summary: `no /metrics at ${cfg.metricsUrl}: MTP acceptance, prefix-cache and governor measurements will be skipped` };
    const have = { spec: m0.specAccepted !== undefined, prefix: m0.prefixQueries !== undefined, kv: m0.kvUsage !== undefined, perPos: !!m0.specAcceptedPerPos };
    return { status: have.spec && have.prefix && have.kv ? 'pass' : 'warn', summary: `spec-decode ${have.spec}, per-position ${have.perPos}, prefix-cache ${have.prefix}, kv usage ${have.kv}`, data: have };
  });

  await add('streaming speed (thinking off/on)', async () => {
    const data: Record<string, unknown> = {};
    for (const th of ['off', 'high'] as const) {
      const rs = [];
      for (let i = 0; i < 3; i++) {
        const r = await llm.chat({ messages: [{ role: 'user', content: 'Write a TypeScript function that debounces another function, with a short explanation.' }], thinking: th, maxTokens: 400 });
        const genMs = Math.max(1, r.totalMs - r.ttftMs);
        rs.push({ ttft: r.ttftMs, tps: (r.usage.completion_tokens * 1000) / genMs, reasoning: r.reasoning.length > 0, think: r.thinkLeak });
      }
      data[th] = { ttftMs: median(rs.map((x) => x.ttft)), tokPerSec: +median(rs.map((x) => x.tps)).toFixed(1), reasoningField: rs.some((x) => x.reasoning), thinkLeak: rs.some((x) => x.think) };
    }
    const off: any = data.off, on: any = data.high;
    const bad = off.reasoningField ? 'thinking-off still returned reasoning (enable_thinking not honored)' : !on.reasoningField ? 'thinking-on returned no reasoning_content (check --reasoning-parser qwen3)' : '';
    return { status: bad ? 'warn' : 'pass', summary: `off: ${off.tokPerSec} tok/s TTFT ${off.ttftMs}ms; on: ${on.tokPerSec} tok/s TTFT ${on.ttftMs}ms${bad ? `; ${bad}` : ''}`, data };
  });

  const trials = o.trials ?? (o.quick ? 10 : 40);
  await add(`tool-call reliability (${trials} calls x thinking off/on)`, async () => {
    const stat = { total: 0, called: 0, right: 0, argsOk: 0, ok: 0, repaired: 0, malformed: 0, fromContent: 0, leak: 0 };
    const perScenario: Record<string, { n: number; ok: number }> = {};
    const per = Math.max(1, Math.round(trials / (TOOL_SCENARIOS.length * 2)));
    for (const th of ['off', 'high'] as const) for (const sc of TOOL_SCENARIOS) for (let i = 0; i < per; i++) {
      const r = await llm.chat({ messages: [{ role: 'system', content: 'You are a coding agent. Always respond by calling the appropriate tool.' }, { role: 'user', content: sc.prompt }], tools: TOOLS, thinking: th, maxTokens: 900, temperature: 0.6 });
      stat.total++;
      const call = r.toolCalls[0];
      const s = (perScenario[sc.name] ??= { n: 0, ok: 0 }); s.n++;
      if (!call) continue;
      stat.called++;
      if (call.name === sc.expect) stat.right++;
      if (call.parse === 'ok') stat.ok++; if (call.parse === 'repaired') stat.repaired++; if (call.parse === 'malformed') stat.malformed++;
      if (call.fromContent) stat.fromContent++;
      if (r.thinkLeak) stat.leak++;
      if (sc.required.every((k) => typeof call.args[k] === 'string' && (call.args[k] as string).length > 0) && call.parse !== 'malformed') { stat.argsOk++; s.ok++; }
    }
    const malformedRate = stat.malformed / stat.total, repairedRate = (stat.repaired + stat.malformed) / stat.total;
    const st: Status = malformedRate >= 0.01 ? 'fail' : repairedRate > 0.05 || stat.leak > 0 ? 'warn' : 'pass';
    return { status: st, summary: `calls ${pct(stat.called / stat.total)}, right tool ${pct(stat.right / stat.total)}, args valid ${pct(stat.argsOk / stat.total)}, server-clean ${pct(stat.ok / stat.total)}, repaired ${pct(stat.repaired / stat.total)}, malformed ${pct(malformedRate)} (target <1%), think-leak ${stat.leak}`, data: { ...stat, malformedRate, repairedRate, perScenario } };
  });

  await add('streamed vs non-streamed tool call (temperature 0, MTP correctness)', async () => {
    let same = 0, n = 0;
    for (const sc of TOOL_SCENARIOS.slice(0, o.quick ? 2 : 5)) {
      const msgs = [{ role: 'system', content: 'You are a coding agent. Always respond by calling the appropriate tool.' }, { role: 'user', content: sc.prompt }];
      const streamed = await llm.chat({ messages: msgs as any, tools: TOOLS, thinking: 'off', temperature: 0, maxTokens: 600 });
      const body = llm.buildBody({ messages: msgs as any, tools: TOOLS, thinking: 'off', temperature: 0, maxTokens: 600 });
      const { json } = await rawChat(cfg, { ...body, stream: false, stream_options: undefined });
      const tc = json.choices?.[0]?.message?.tool_calls?.[0];
      n++;
      if (tc && streamed.toolCalls[0] && tc.function?.name === streamed.toolCalls[0].name) {
        try { if (JSON.stringify(JSON.parse(tc.function.arguments)) === JSON.stringify(streamed.toolCalls[0].args)) same++; } catch { /* mismatch */ }
      }
    }
    return { status: same === n ? 'pass' : same >= n - 1 ? 'warn' : 'fail', summary: `${same}/${n} identical`, data: { same, n } };
  });

  await add('thinking must not leak into tool arguments or content', async () => {
    let leaks = 0, badContent = 0, n = 0;
    for (let i = 0; i < (o.quick ? 4 : 10); i++) {
      const r = await llm.chat({ messages: [{ role: 'user', content: `Think carefully about which file could contain the bug in the parser (attempt ${i}), then call read_file on src/parser.ts.` }], tools: TOOLS, thinking: 'high', maxTokens: 1500 });
      n++; if (r.thinkLeak) leaks++;
      if (/<think>|<\/think>/.test(r.content)) badContent++;
    }
    return { status: leaks || badContent ? 'fail' : 'pass', summary: `${leaks}/${n} tool args with think tags, ${badContent}/${n} content with think tags`, data: { leaks, badContent, n } };
  });

  await add('MTP acceptance (structured vs prose)', async () => {
    if (!m0 || m0.specAccepted === undefined) return { status: 'skipped', summary: 'no spec-decode metrics' };
    const run = async (prompts: string[], structured: boolean) => {
      const a = await fetchMetrics(cfg);
      for (const p of prompts) await llm.chat({ messages: [{ role: 'user', content: p }], thinking: 'off', maxTokens: 350, ...(structured ? { jsonSchema: { type: 'object', properties: { items: { type: 'array', items: { type: 'object', properties: { id: { type: 'integer' }, name: { type: 'string' }, active: { type: 'boolean' } }, required: ['id', 'name', 'active'] } } }, required: ['items'] } } : {}) });
      return delta(a, await fetchMetrics(cfg));
    };
    const n = o.quick ? 3 : 6;
    const prose = await run(Array.from({ length: n }, (_, i) => `Explain in a few paragraphs why idempotency matters in distributed systems (variation ${i}).`), false);
    const json = await run(Array.from({ length: n }, (_, i) => `Return 12 sample records as JSON items with id, name and active (variation ${i}).`), true);
    const code = await run(Array.from({ length: n }, (_, i) => `Write a complete TypeScript class Stack<T> with push, pop, peek, size and isEmpty, plus doc comments (variation ${i}).`), false);
    const f = (d: any) => (d.acceptanceRate === undefined ? 'n/a' : pct(d.acceptanceRate));
    return { status: 'info', summary: `acceptance: prose ${f(prose)}, json ${f(json)}, code ${f(code)}; per-position (code): ${(code.perPositionAcceptance ?? []).map((x) => pct(x)).join(' / ') || 'n/a'}`, data: { prose, json, code } };
  });

  await add('prefix cache (stable prefix first vs variable first)', async () => {
    if (!m0 || m0.prefixQueries === undefined) return { status: 'skipped', summary: 'no prefix-cache metrics' };
    const prefix = `You are a coding agent.\n${filler(o.quick ? 1500 : 4000, 1)}`;
    const ask = async (system: string, user: string) => { const r = await llm.chat({ messages: [{ role: 'system', content: system }, { role: 'user', content: user }], thinking: 'off', maxTokens: 16 }); return r.ttftMs; };
    const a0 = await fetchMetrics(cfg);
    const cold = await ask(prefix, 'Task 0: say ok.');
    const warm: number[] = []; for (let i = 1; i <= 4; i++) warm.push(await ask(prefix, `Task ${i}: say ok.`));
    const stable = delta(a0, await fetchMetrics(cfg));
    const b0 = await fetchMetrics(cfg);
    const varFirst: number[] = []; for (let i = 0; i < 4; i++) varFirst.push(await ask(`Session ${Date.now()}-${i}.\n${filler(o.quick ? 1500 : 4000, 2)}`, 'say ok.'));
    const variable = delta(b0, await fetchMetrics(cfg));
    const ok = (stable.prefixHitRate ?? 0) > (variable.prefixHitRate ?? 0);
    return { status: ok ? 'pass' : 'warn', summary: `stable prefix: hit ${stable.prefixHitRate === undefined ? 'n/a' : pct(stable.prefixHitRate)}, TTFT cold ${cold}ms -> warm ${median(warm)}ms; variable-first: hit ${variable.prefixHitRate === undefined ? 'n/a' : pct(variable.prefixHitRate)}, TTFT ${median(varFirst)}ms${ok ? '' : ' (prefix caching not visibly effective: is --enable-prefix-caching on?)'}`, data: { stable, variable, coldMs: cold, warmMs: warm, varFirstMs: varFirst } };
  });

  await add('long-context retrieval and latency', async () => {
    const limit = o.maxContext ?? (maxLen ? Math.floor(maxLen * 0.8) : 32000);
    const sizes = (o.quick ? [4000, 16000] : [4000, 16000, 32000, 64000, 100000, 128000]).filter((s) => s <= limit);
    const rows: Record<string, unknown>[] = [];
    for (const size of sizes) {
      const secret = `KX-${Math.floor(Math.random() * 90000 + 10000)}`;
      const lines = filler(size, size).split('\n'); lines.splice(Math.floor(lines.length * 0.4), 0, `The maintenance passphrase is ${secret}.`);
      const t0 = Date.now();
      const r = await llm.chat({ messages: [{ role: 'user', content: `${lines.join('\n')}\n\nWhat is the maintenance passphrase? Answer with the passphrase only.` }], thinking: 'off', maxTokens: 30 });
      rows.push({ target: size, promptTokens: r.usage.prompt_tokens, correct: r.content.includes(secret), ttftMs: r.ttftMs, totalMs: Date.now() - t0 });
    }
    const wrong = rows.filter((r) => !r.correct).length;
    return { status: wrong ? 'warn' : 'pass', summary: rows.map((r) => `${r.promptTokens} tok: ${r.correct ? 'ok' : 'MISS'} TTFT ${r.ttftMs}ms`).join('; ') || 'no size fits max_model_len', data: { rows } };
  });

  await add('concurrency and KV pressure (governor calibration)', async () => {
    const levels = o.concurrency ? [1, o.concurrency] : o.quick ? [1, 2, 4] : [1, 2, 4, 8];
    const ctx = o.quick ? 4000 : 8000;
    const table: Record<string, unknown>[] = [];
    let single = 0, recommended = 1;
    for (const k of levels) {
      const samples: VllmMetrics[] = [];
      let stop = false;
      const sampler = (async () => { while (!stop) { const m = await fetchMetrics(cfg); if (m) samples.push(m); await sleep(500); } })();
      const t0 = Date.now();
      const lat = await Promise.all(Array.from({ length: k }, async (_, i) => { const t = Date.now(); const r = await llm.chat({ messages: [{ role: 'user', content: `${filler(ctx, 100 + i + k * 10)}\n\nSummarize the records above in one sentence.` }], thinking: 'off', maxTokens: 96 }); return { ms: Date.now() - t, out: r.usage.completion_tokens }; }));
      stop = true; await sampler;
      const wall = (Date.now() - t0) / 1000;
      const kv = Math.max(0, ...samples.map((s) => s.kvUsage ?? 0)), waiting = Math.max(0, ...samples.map((s) => s.waiting ?? 0));
      const row = { concurrency: k, p95Ms: p95(lat.map((x) => x.ms)), throughputTokPerS: +(lat.reduce((n, x) => n + x.out, 0) / wall).toFixed(1), peakKv: +kv.toFixed(2), peakWaiting: waiting };
      if (k === 1) single = row.p95Ms;
      if (k > 1 && row.p95Ms <= single * 2.2 && waiting === 0 && kv < 0.85) recommended = k;
      table.push(row);
    }
    return { status: 'info', summary: `recommended FH maxConcurrency (config) = ${recommended}. ${table.map((r: any) => `k=${r.concurrency}: p95 ${r.p95Ms}ms, ${r.throughputTokPerS} tok/s, KV ${r.peakKv}, waiting ${r.peakWaiting}`).join(' | ')}`, data: { table, recommended } };
  });

  await add('cancellation frees the server', async () => {
    const before = await fetchMetrics(cfg);
    const ac = new AbortController();
    const p = llm.chat({ messages: [{ role: 'user', content: 'Write a very long essay about the history of computing, at least 3000 words.' }], thinking: 'off', maxTokens: 4000, signal: ac.signal }).catch(() => undefined);
    await sleep(1500); ac.abort(); await p;
    if (!before || before.running === undefined) return { status: 'skipped', summary: 'no running-requests metric' };
    let running = Infinity;
    for (let i = 0; i < 20 && running > (before.running ?? 0); i++) { await sleep(500); running = (await fetchMetrics(cfg))?.running ?? Infinity; }
    return { status: running <= (before.running ?? 0) ? 'pass' : 'fail', summary: running <= (before.running ?? 0) ? 'aborted request was released within 10s' : `still ${running} running request(s) 10s after abort`, data: { before: before.running, after: running } };
  });

  await add('error handling (bad model, oversized request)', async () => {
    const bad = await rawChat(cfg, { model: 'no-such-model', messages: [{ role: 'user', content: 'x' }], max_tokens: 1 });
    const huge = await rawChat(cfg, { messages: [{ role: 'user', content: filler(Math.max(50000, (maxLen || 32768) + 20000)) }], max_tokens: 1 });
    return { status: bad.status >= 400 && huge.status >= 400 ? 'pass' : 'warn', summary: `unknown model -> HTTP ${bad.status}; oversized prompt -> HTTP ${huge.status} (${String(huge.json?.error?.message ?? '').slice(0, 90)})`, data: { bad: bad.status, huge: huge.status } };
  });

  return finish(results, cfg, o);
}

function finish(results: Probe[], cfg: Config, o: ValidateOptions): number {
  const dir = join(o.out, `vllm-${new Date().toISOString().replace(/[:.]/g, '-')}`);
  mkdirSync(dir, { recursive: true });
  const icon: Record<Status, string> = { pass: 'PASS', warn: 'WARN', fail: 'FAIL', info: 'INFO', skipped: 'SKIP' };
  const md = [`# vLLM validation`, '', `Model \`${cfg.model}\` at \`${cfg.endpoint}\` — ${new Date().toISOString()}`, '', '| Probe | Result | Summary |', '|---|---|---|', ...results.map((r) => `| ${r.name} | ${icon[r.status]} | ${r.summary.replace(/\|/g, '/')} |`), ''].join('\n');
  writeFileSync(join(dir, 'report.json'), JSON.stringify({ model: cfg.model, endpoint: cfg.endpoint, at: new Date().toISOString(), results }, null, 2));
  writeFileSync(join(dir, 'report.md'), md);
  const fails = results.filter((r) => r.status === 'fail').length, warns = results.filter((r) => r.status === 'warn').length;
  console.log(`\n${fails} failed, ${warns} warnings. Report: ${dir}/report.md`);
  return fails ? 1 : 0;
}
