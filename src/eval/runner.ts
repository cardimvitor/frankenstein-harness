import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import type { Config } from '../config.ts';
import { Engine, headlessIO, type TaskResult } from '../engine.ts';
import { SkillStore } from '../skills/store.ts';
import { run } from '../util/proc.ts';
import { BUILTIN_TASKS, type EvalTask } from './corpus.ts';

export interface EvalRow {
  id: string;
  runner: string;
  rep: number;
  solved: boolean;
  seconds: number;
  verdict?: string;
  rounds?: number;
  toolCalls?: number;
  failedTools?: number;
  repaired?: number;
  malformed?: number;
  thinkLeaks?: number;
  tokensOut?: number;
  tokensIn?: number;
  requests?: number;
  acceptance?: number;
  prefixHit?: number;
  /** verifier said fail/rolled back although the oracle would have passed the original attempt */
  verifierFalsePositive?: boolean;
  /** verifier said pass although the oracle fails */
  verifierFalseNegative?: boolean;
  gateMs?: number;
  reviewer?: { raised: number; valid: number; dropped: number; blockers: number };
  error?: string;
}

export function loadTasks(dir: string): EvalTask[] {
  if (!existsSync(dir)) return BUILTIN_TASKS;
  const out: EvalTask[] = [];
  for (const d of readdirSync(dir, { withFileTypes: true })) {
    const tj = join(dir, d.name, 'task.json');
    if (!d.isDirectory() || !existsSync(tj)) continue;
    const t = JSON.parse(readFileSync(tj, 'utf8'));
    out.push({ id: t.id ?? d.name, prompt: t.prompt, oracle: t.oracle, timeoutS: t.timeoutS, tags: t.tags, files: { __repo__: join(dir, d.name, 'repo') } });
  }
  return out.length ? out : BUILTIN_TASKS;
}

async function materialize(t: EvalTask): Promise<string> {
  const d = mkdtempSync(join(tmpdir(), `fh-eval-${t.id}-`));
  if (t.files.__repo__) cpSync(t.files.__repo__, d, { recursive: true });
  else for (const [f, body] of Object.entries(t.files)) { mkdirSync(join(d, dirname(f)), { recursive: true }); writeFileSync(join(d, f), body); }
  await run('git init -q && git add -A && git -c user.name=eval -c user.email=eval@local commit -qm base', { cwd: d });
  return d;
}

const oracleOk = async (t: EvalTask, cwd: string) => (await run(t.oracle, { cwd, timeoutMs: 120_000 })).code === 0;

export async function runOne(cfg: Config, t: EvalTask, runner: 'fh' | 'qwen', rep: number, o: { qwenCmd: string; auto: boolean }): Promise<EvalRow> {
  const cwd = await materialize(t);
  const t0 = Date.now();
  const row: EvalRow = { id: t.id, runner, rep, solved: false, seconds: 0 };
  try {
    if (runner === 'fh') {
      const engine = new Engine(cfg, headlessIO(), { cwd, store: new SkillStore(':memory:') });
      const r: TaskResult = await engine.runTask(t.prompt, { mode: 'auto', approval: 'yolo', noMine: true });
      Object.assign(row, {
        verdict: r.verdict, rounds: r.rounds, toolCalls: r.llm.toolCalls, failedTools: r.agent?.failedTools, repaired: r.llm.repaired, malformed: r.llm.malformed, thinkLeaks: r.llm.thinkLeaks,
        tokensOut: r.llm.completionTokens, tokensIn: r.llm.promptTokens, requests: r.llm.requests, acceptance: r.metrics?.acceptanceRate, prefixHit: r.metrics?.prefixHitRate, gateMs: r.gate?.ms, reviewer: r.reviewer,
      });
      row.solved = await oracleOk(t, cwd);
      if (r.verdict === 'fail' && r.rolledBack) row.verifierFalsePositive = false; // rolled back: cannot tell; measured separately below
      if (r.verdict === 'pass' && !row.solved) row.verifierFalseNegative = true;
      if (r.verdict === 'error') row.error = r.reason;
    } else {
      const env = { ...process.env, OPENAI_BASE_URL: cfg.endpoint, OPENAI_MODEL: cfg.model, OPENAI_API_KEY: process.env[cfg.apiKeyEnv] ?? 'none' };
      const r = await run(`${o.qwenCmd} -y -p ${JSON.stringify(t.prompt)}`, { cwd, timeoutMs: (t.timeoutS ?? 900) * 1000, env });
      if (r.code !== 0) row.error = (r.stderr || r.stdout).slice(-300);
      row.solved = await oracleOk(t, cwd);
    }
  } catch (e) { row.error = (e as Error).message; }
  row.seconds = (Date.now() - t0) / 1000;
  return row;
}

const median = (a: number[]) => { const s = [...a].sort((x, y) => x - y); return s.length ? (s.length % 2 ? s[(s.length - 1) / 2] : (s[s.length / 2 - 1] + s[s.length / 2]) / 2) : 0; };
const pct = (n: number, d: number) => (d ? `${((100 * n) / d).toFixed(1)}%` : 'n/a');
const sum = (a: (number | undefined)[]) => a.reduce<number>((n, x) => n + (x ?? 0), 0);

export function summarize(rows: EvalRow[]): string {
  const by = new Map<string, EvalRow[]>();
  for (const r of rows) by.set(r.runner, [...(by.get(r.runner) ?? []), r]);
  const lines = ['| runner | tasks | pass rate | median s | tool calls | malformed | repaired | think leaks | verifier FN | avg rounds | tokens out |', '|---|---|---|---|---|---|---|---|---|---|---|'];
  for (const [runner, rs] of by) {
    const calls = sum(rs.map((r) => r.toolCalls));
    lines.push(`| ${runner} | ${rs.length} | ${pct(rs.filter((r) => r.solved).length, rs.length)} | ${median(rs.map((r) => r.seconds)).toFixed(1)} | ${runner === 'fh' ? calls : 'n/a'} | ${runner === 'fh' ? pct(sum(rs.map((r) => r.malformed)), calls) : 'n/a'} | ${runner === 'fh' ? pct(sum(rs.map((r) => r.repaired)), calls) : 'n/a'} | ${runner === 'fh' ? sum(rs.map((r) => r.thinkLeaks)) : 'n/a'} | ${runner === 'fh' ? pct(rs.filter((r) => r.verifierFalseNegative).length, rs.length) : 'n/a'} | ${runner === 'fh' ? (sum(rs.map((r) => r.rounds)) / rs.length).toFixed(2) : 'n/a'} | ${runner === 'fh' ? sum(rs.map((r) => r.tokensOut)) : 'n/a'} |`);
  }
  const fh = by.get('fh');
  if (fh) {
    const rev = fh.map((r) => r.reviewer).filter(Boolean) as NonNullable<EvalRow['reviewer']>[];
    const raised = sum(rev.map((r) => r.raised)), dropped = sum(rev.map((r) => r.dropped));
    const fp = fh.filter((r) => r.verdict === 'fail' && r.solved).length;
    lines.push('', `Verifier: verdict=fail while the oracle passes the final tree: ${fp}/${fh.length}; reviewer findings raised ${raised}, dropped as uncheckable ${dropped} (${pct(dropped, raised)}).`);
    const acc = fh.map((r) => r.acceptance).filter((x): x is number => typeof x === 'number');
    if (acc.length) lines.push(`MTP acceptance (mean over tasks): ${(100 * (sum(acc) / acc.length)).toFixed(1)}%. Prefix-cache hit rate (mean): ${(100 * (sum(fh.map((r) => r.prefixHit)) / Math.max(1, fh.filter((r) => typeof r.prefixHit === 'number').length))).toFixed(1)}%.`);
    lines.push(`Skill gate: mean ${(sum(fh.map((r) => r.gateMs)) / fh.length).toFixed(2)} ms, 0 LLM calls.`);
  }
  return lines.join('\n');
}

export async function runEval(cfg: Config, o: { tasks: string; runner: 'fh' | 'qwen' | 'both'; out: string; limit?: number; repeat: number; qwenCmd: string; auto: boolean }): Promise<number> {
  let tasks = loadTasks(o.tasks);
  if (o.limit) tasks = tasks.slice(0, o.limit);
  const runners: ('fh' | 'qwen')[] = o.runner === 'both' ? ['fh', 'qwen'] : [o.runner];
  const dir = join(o.out, `eval-${new Date().toISOString().replace(/[:.]/g, '-')}`);
  mkdirSync(dir, { recursive: true });
  const rows: EvalRow[] = [];
  for (const t of tasks) for (let rep = 1; rep <= o.repeat; rep++) for (const rn of runners) {
    process.stdout.write(`${rn} ${t.id} #${rep} … `);
    const r = await runOne(cfg, t, rn, rep, o);
    rows.push(r);
    console.log(`${r.solved ? 'solved' : 'failed'} in ${r.seconds.toFixed(1)}s${r.error ? ` (${r.error.slice(0, 80)})` : ''}`);
    writeFileSync(join(dir, 'results.jsonl'), rows.map((x) => JSON.stringify(x)).join('\n') + '\n');
  }
  const md = `# Eval ${new Date().toISOString()}\n\nModel ${cfg.model} @ ${cfg.endpoint}\n\n${summarize(rows)}\n`;
  writeFileSync(join(dir, 'summary.md'), md);
  console.log('\n' + md + `\nSaved to ${dir}`);
  return rows.every((r) => r.solved) ? 0 : 1;
}
