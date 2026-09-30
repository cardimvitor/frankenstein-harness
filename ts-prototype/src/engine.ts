import { basename, join } from 'node:path';
import { mkdirSync, writeFileSync } from 'node:fs';
import type { Config } from './config.ts';
import { LlmClient, emptyStats, type LlmStats } from './llm/client.ts';
import { fetchMetrics, delta, type MetricsDelta } from './llm/metrics.ts';
import { runAgent, type AgentResult } from './agent/loop.ts';
import { contextBlock } from './agent/prompt.ts';
import { fingerprint, type Fingerprint } from './fingerprint.ts';
import { Checkpoints } from './session/checkpoint.ts';
import { intake, inspectRepo, renderPlan, type Intake, type Subtask } from './funnel/intake.ts';
import { gate, renderSkills, renderUser, type GateResult } from './skills/gate.ts';
import { loadUserConfig } from './skills/usercfg.ts';
import { SkillStore, type Skill } from './skills/store.ts';
import { acceptReuse, findReuseOffers, type ReuseOffer } from './skills/reuse.ts';
import { mine, promoteEligible } from './skills/miner.ts';
import { evaluate } from './skills/police.ts';
import { verifyLoop, type RoundReport, type VerifyReport } from './verify/rounds.ts';
import { Governor } from './orchestrator/governor.ts';
import { ownersOf, runWorkers, type WorkerResult } from './orchestrator/master.ts';
import { sandboxFor } from './util/sandbox.ts';
import { run } from './util/proc.ts';
import type { Mode, Message } from './types.ts';
import type { Tool } from './tools/types.ts';

export interface IO {
  notice(kind: 'phase' | 'skill' | 'verify' | 'warn' | 'info', msg: string): void;
  progress?(delta: string): void;
  reasoning?(delta: string): void;
  toolStart?(name: string, args: Record<string, unknown>): void;
  toolEnd?(name: string, ok: boolean, output: string, ms: number): void;
  askQuestions(questions: string[]): Promise<string[]>;
  approvePlan(plan: string, trivial: boolean): Promise<{ ok: boolean; feedback?: string }>;
  confirm(tool: Tool, args: Record<string, unknown>): Promise<boolean>;
  offerReuse(offers: ReuseOffer[]): Promise<{ fromProject: string; ids: string[] }[]>;
}

/** Non-interactive IO for eval/CI/auto mode: nothing is ever asked. */
export const headlessIO = (log?: (m: string) => void): IO => ({
  notice: (k, m) => log?.(`[${k}] ${m}`),
  askQuestions: async (q) => q.map(() => '(use your best judgement)'),
  approvePlan: async () => ({ ok: true }),
  confirm: async () => true,
  offerReuse: async () => [],
});

export interface TaskOptions {
  mode: 'normal' | 'auto';
  approval: Mode;
  commit?: boolean;
  planOnly?: boolean;
  /** keep changes even when verification fails (default: roll back to the checkpoint) */
  keepOnFail?: boolean;
  sandbox?: boolean;
  noMine?: boolean;
  signal?: AbortSignal;
}

export interface TaskResult {
  verdict: 'pass' | 'fail' | 'unverified' | 'planned' | 'aborted' | 'error';
  final: string;
  reason: string;
  changed: string[];
  rounds: number;
  rolledBack: boolean;
  rejectedPatch?: string;
  plan?: Intake;
  skillsUsed: string[];
  gate?: { ms: number; llmCalls: number; decision: string };
  timings: { gateMs: number; intakeMs: number; workMs: number; verifyMs: number; totalMs: number };
  llm: LlmStats;
  metrics?: MetricsDelta;
  verify?: VerifyReport;
  workers?: WorkerResult[];
  agent?: { steps: number; toolCalls: number; failedTools: number; stopped: string };
  reviewer?: VerifyReport['reviewer'];
  /** unified diff of what the task changed (kept even when rolled back), clipped */
  diff?: string;
}

/** Scope for the diff-scope check: planned files, their directories, and test files. */
export function allowedFromPlan(plan: { files: string[] }[], subtasks: Subtask[] = []): string[] | undefined {
  const files = [...plan.flatMap((p) => p.files), ...subtasks.flatMap((s) => s.files)].map((f) => f.replace(/^\.\//, '')).filter(Boolean);
  if (!files.length) return undefined;
  const globs = new Set<string>(['**/*.test.*', '**/*.spec.*', 'test/**', 'tests/**', '__tests__/**']);
  for (const f of files) {
    globs.add(f);
    const dir = f.includes('/') ? f.slice(0, f.lastIndexOf('/')) : '';
    if (dir && !f.includes('*')) globs.add(`${dir}/**`);
  }
  return [...globs];
}

const ZERO = { added: [] as string[], modified: [] as string[], deleted: [] as string[] };

export class Engine {
  cfg: Config;
  io: IO;
  cwd: string;
  llm: LlmClient;
  store: SkillStore;
  private pending: Promise<unknown>[] = [];
  constructor(cfg: Config, io: IO, o: { cwd: string; store?: SkillStore; llm?: LlmClient }) {
    this.cfg = cfg; this.io = io; this.cwd = o.cwd;
    this.llm = o.llm ?? new LlmClient(cfg);
    this.store = o.store ?? new SkillStore();
  }

  /** Wait for post-delivery background work (skill mining) so short-lived CLI runs do not drop it. */
  async drain(timeoutMs = 20_000) {
    await Promise.race([Promise.allSettled(this.pending), new Promise((r) => setTimeout(r, timeoutMs).unref())]);
    this.pending = [];
  }

  async runTask(task: string, o: TaskOptions): Promise<TaskResult> {
    const t0 = Date.now();
    const timings = { gateMs: 0, intakeMs: 0, workMs: 0, verifyMs: 0, totalMs: 0 };
    this.llm.stats = emptyStats();
    const m0 = await fetchMetrics(this.cfg);
    const finish = async (r: Omit<TaskResult, 'timings' | 'llm' | 'metrics'>): Promise<TaskResult> => {
      timings.totalMs = Date.now() - t0;
      const m1 = await fetchMetrics(this.cfg);
      return { ...r, timings, llm: { ...this.llm.stats }, metrics: delta(m0, m1) };
    };
    const base = { final: '', reason: '', changed: [] as string[], rounds: 0, rolledBack: false, skillsUsed: [] as string[] };
    const fp: Fingerprint = fingerprint(this.cwd);
    const known = this.store.isKnownProject(fp.projectId);
    this.store.registerProject(fp, basename(this.cwd));

    // reuse offer: normal mode only, asked once, names and summaries only
    if (o.mode === 'normal' && !known) {
      const offers = findReuseOffers(this.store, fp);
      if (offers.length) {
        const picks = await this.io.offerReuse(offers);
        for (const p of picks) {
          const offer = offers.find((x) => x.fromProject === p.fromProject);
          if (offer && p.ids.length) { const n = acceptReuse(this.store, fp, offer, p.ids); this.io.notice('skill', `reused ${n} skill(s) from ${offer.fromLabel}`); }
        }
      }
    }

    // skill gate: deterministic, no model call
    const user = loadUserConfig(this.cwd);
    const g: GateResult = gate(this.store, fp, task, user);
    timings.gateMs = g.ms;
    for (const s of g.selected) this.io.notice('skill', `using ${s.scope} skill "${s.name}"`);

    // intake funnel
    this.io.notice('phase', o.mode === 'auto' ? 'planning (autonomous)' : 'inspecting repository and planning');
    const ti = Date.now();
    const repo = inspectRepo(this.cwd, fp);
    let plan: Intake;
    try {
      plan = await intake({ llm: this.llm, task, repo, mode: o.mode, ambiguous: g.ambiguous, maxSubtasks: this.cfg.maxConcurrency, signal: o.signal });
      if (o.mode === 'normal' && plan.questions.length) {
        const qs = plan.questions;
        const answers = await this.io.askQuestions(qs);
        plan = await intake({ llm: this.llm, task, repo, mode: 'normal', ambiguous: g.ambiguous, maxSubtasks: this.cfg.maxConcurrency, signal: o.signal, answers: qs.map((q, i) => ({ question: q, answer: answers[i] ?? '' })) });
      }
      if (o.mode === 'normal') {
        const ap = await this.io.approvePlan(renderPlan(plan), plan.trivial);
        if (!ap.ok && ap.feedback) plan = await intake({ llm: this.llm, task: `${task}\n\nUser feedback on the plan: ${ap.feedback}`, repo, mode: 'auto', signal: o.signal });
        else if (!ap.ok) return finish({ ...base, verdict: 'aborted', reason: 'plan rejected', plan, skillsUsed: g.selected.map((s) => s.name) });
      }
    } catch (e) {
      if (o.signal?.aborted) return finish({ ...base, verdict: 'aborted', reason: 'cancelled' });
      return finish({ ...base, verdict: 'error', reason: `planning failed: ${(e as Error).message}` });
    }
    timings.intakeMs = Date.now() - ti;
    if (g.ambiguous.length && plan.skillChoice) {
      const drop = g.ambiguous.filter((n) => n !== plan.skillChoice);
      g.selected = g.selected.filter((s) => !drop.includes(s.name));
    }
    const skillsUsed = g.selected.map((s) => s.name);
    if (o.planOnly) return finish({ ...base, verdict: 'planned', reason: 'plan only', plan, skillsUsed });

    // work
    const cp = new Checkpoints(this.cwd);
    const baseCp = (await cp.create('before task')) ?? '';
    const sandbox = o.sandbox ? sandboxFor(this.cwd) : { backend: 'none', wrap: undefined };
    if (o.sandbox) this.io.notice('info', `shell sandbox: ${sandbox.backend}`);
    const context = contextBlock({ cwd: this.cwd, fingerprint: fp.summary, skills: renderSkills(g), userRules: renderUser(user, g), plan: renderPlan(plan) });
    const taskText = `${plan.enriched}\n\nAcceptance criteria:\n${plan.acceptance.map((a) => `- ${a}`).join('\n') || '- the task is complete and existing tests still pass'}`;
    const tw = Date.now();
    let history: Message[] | undefined;
    let agent: AgentResult | undefined;
    let workers: WorkerResult[] | undefined;
    const events = {
      text: (d: string) => this.io.progress?.(d), reasoning: (d: string) => this.io.reasoning?.(d),
      toolStart: (c: { name: string; args: Record<string, unknown> }) => this.io.toolStart?.(c.name, c.args),
      toolEnd: (c: { name: string }, r: { ok: boolean; output: string }, ms: number) => this.io.toolEnd?.(c.name, r.ok, r.output, ms),
      notice: (m: string) => this.io.notice('info', m),
    };
    const agentOpts = (extra: Partial<Parameters<typeof runAgent>[1]> = {}): Parameters<typeof runAgent>[1] => ({
      llm: this.llm, cwd: this.cwd, mode: o.approval, thinking: 'auto', maxSteps: this.cfg.maxSteps, signal: o.signal, context,
      contextWindow: this.cfg.contextWindow, wrapShell: sandbox.wrap, events, confirm: (t, a) => this.io.confirm(t, a), ...extra,
    });
    const gov = new Governor(this.cfg.maxConcurrency, () => fetchMetrics(this.cfg));
    const multi = plan.subtasks.length >= 2 && this.cfg.maxConcurrency > 1;
    this.io.notice('phase', multi ? `working (${plan.subtasks.length} workers, file-ownership partitioned)` : 'working');
    if (multi) {
      workers = await runWorkers(plan.enriched, plan.subtasks, { llm: this.llm, cwd: this.cwd, mode: o.approval, context, governor: gov, signal: o.signal, contextWindow: this.cfg.contextWindow, notice: (m) => this.io.notice('info', m), wrapShell: sandbox.wrap });
    } else {
      agent = await runAgent(taskText, agentOpts());
      history = agent.messages;
      if (agent.stopped === 'error') {
        return finish({ ...base, verdict: 'error', reason: agent.error ?? 'agent error', plan, skillsUsed, agent: { steps: agent.steps, toolCalls: agent.toolCalls, failedTools: agent.failedTools, stopped: agent.stopped } });
      }
    }
    timings.workMs = Date.now() - tw;
    if (o.signal?.aborted) {
      if (baseCp && !o.keepOnFail) await cp.restore(baseCp);
      return finish({ ...base, verdict: 'aborted', reason: 'cancelled', rolledBack: !!baseCp, plan, skillsUsed });
    }

    // verification rounds: deterministic checks decide
    const maxRounds = o.mode === 'auto' ? this.cfg.verifyRoundsAuto : this.cfg.verifyRoundsNormal;
    const tv = Date.now();
    const subtasks = plan.subtasks;
    const report: VerifyReport = await verifyLoop({
      cwd: this.cwd, cp, base: baseCp, fp, llm: this.llm, acceptance: plan.acceptance, allowedGlobs: allowedFromPlan(plan.plan, subtasks), maxRounds, signal: o.signal,
      onRound: (r: RoundReport) => {
        const failed = r.checks.filter((c) => c.status === 'fail').map((c) => c.name);
        this.io.notice('verify', `round ${r.round}: ${r.verdict}${failed.length ? ` (failed: ${failed.join(', ')})` : ''}`);
      },
      fix: async (feedback) => {
        this.io.notice('phase', 'fixing verification failures');
        if (multi) {
          const files = [...feedback.matchAll(/([\w./-]+\.[A-Za-z0-9]+):\d+/g)].map((m) => m[1]);
          const { owned, unowned } = ownersOf(subtasks, files);
          const jobs: Promise<unknown>[] = [];
          for (const [id, fs] of owned) {
            const s = subtasks.find((x) => x.id === id)!;
            jobs.push(gov.run(() => runAgent(`${feedback}\n\nFix only problems in: ${fs.join(', ')}`, agentOpts({ ownedGlobs: s.files, maxSteps: 15 }))));
          }
          if (unowned.length || !owned.size) jobs.push(gov.run(() => runAgent(feedback, agentOpts({ maxSteps: 15 }))));
          await Promise.all(jobs);
        } else {
          const r = await runAgent(feedback, agentOpts({ history, maxSteps: 20 }));
          history = r.messages;
          agent = { ...r, steps: (agent?.steps ?? 0) + r.steps, toolCalls: (agent?.toolCalls ?? 0) + r.toolCalls, failedTools: (agent?.failedTools ?? 0) + r.failedTools };
        }
      },
    });
    timings.verifyMs = Date.now() - tv;
    const ch = baseCp ? await cp.changedSince(baseCp).catch(() => ZERO) : ZERO;
    const changedAll = [...ch.added, ...ch.modified, ...ch.deleted];

    // the verdict gates the outcome: fail -> roll back, keep the patch for inspection
    let rolledBack = false, rejectedPatch: string | undefined;
    const taskDiff = changedAll.length && baseCp ? (await cp.diffSince(baseCp).catch(() => '')).slice(0, 200_000) : '';
    if (report.verdict === 'fail' && baseCp && !o.keepOnFail && changedAll.length) {
      const diff = await cp.diffSince(baseCp);
      await cp.restore(baseCp);
      rolledBack = true;
      mkdirSync(join(this.cwd, '.fh', 'rejected'), { recursive: true });
      rejectedPatch = join('.fh', 'rejected', `${Date.now()}.patch`);
      writeFileSync(join(this.cwd, rejectedPatch), diff);
    } else if ((report.verdict === 'pass' || report.verdict === 'unverified') && o.commit && changedAll.length) {
      await run(`git add -A && git commit -q -m ${JSON.stringify(`fh: ${plan.enriched.split('\n')[0].slice(0, 60)}`)}`, { cwd: this.cwd });
    }

    // the final answer is produced only after the verdict
    const workerSummary = workers?.map((w) => `${w.id}: ${w.summary}`).join('\n') ?? '';
    const agentFinal = agent?.final || workerSummary;
    const banner = report.verdict === 'pass' ? 'Verified' : report.verdict === 'unverified' ? 'UNVERIFIED (no build/test command available)' : `NOT delivered: verification failed${rolledBack ? '; changes rolled back' : ''}`;
    const final = `${banner}\n\n${agentFinal}`.trim();

    // post-delivery: outcome tracking, quality police, mining (off the critical path)
    const used: Skill[] = g.selected;
    this.store.recordTask(`${fp.projectId}-${t0}`, fp.projectId, used, report.verdict, report.rounds.length);
    for (const s of used) this.store.log('used', s.name, `task ${report.verdict}`, fp.projectId);
    if (!o.noMine) {
      const diffForMining = report.verdict === 'pass' && baseCp ? await cp.diffSince(baseCp).catch(() => '') : '';
      this.pending.push((async () => {
        try {
          evaluate(this.store);
          promoteEligible(this.store);
          if (report.verdict === 'pass') {
            const r = await mine(this.store, this.llm, { task, diff: diffForMining, changed: changedAll, fp, used, verdict: report.verdict, rounds: report.rounds.length });
            if (r.action === 'created') this.io.notice('skill', `learned a new project skill: ${r.reason}`);
          }
        } catch { /* background work must never affect the result */ }
      })());
    }
    return finish({
      ...base, verdict: report.verdict, final, reason: report.reason, changed: changedAll, rounds: report.rounds.length, rolledBack, rejectedPatch, plan, skillsUsed,
      gate: { ms: g.ms, llmCalls: g.llmCalls, decision: g.decision }, diff: taskDiff, verify: report, workers, reviewer: report.reviewer,
      agent: agent ? { steps: agent.steps, toolCalls: agent.toolCalls, failedTools: agent.failedTools, stopped: agent.stopped } : undefined,
    });
  }
}
