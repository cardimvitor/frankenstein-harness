import type { LlmClient } from '../llm/client.ts';
import type { Fingerprint } from '../fingerprint.ts';
import type { Checkpoints } from '../session/checkpoint.ts';
import { diffChecks, runCommands, type CheckResult } from './checks.ts';
import { CHECKLISTS, review, type Finding } from './reviewer.ts';

export type Verdict = 'pass' | 'fail' | 'unverified';

export interface RoundReport {
  round: number;
  checks: CheckResult[];
  findings: Finding[];
  droppedFindings: number;
  reviewerRan: boolean;
  verdict: Verdict;
}

export interface VerifyReport {
  verdict: Verdict;
  rounds: RoundReport[];
  reason: string;
  changed: string[];
  reviewer: { raised: number; valid: number; dropped: number; blockers: number };
}

export interface VerifyOptions {
  cwd: string;
  cp: Checkpoints;
  base: string;
  fp: Fingerprint;
  llm?: LlmClient;
  acceptance: string[];
  allowedGlobs?: string[];
  maxRounds: number;
  signal?: AbortSignal;
  /** Called between rounds with feedback; must apply fixes to the working tree. */
  fix: (feedback: string, round: number) => Promise<void>;
  onRound?: (r: RoundReport) => void;
  onCheck?: (name: string, status: string) => void;
}

const isBlocking = (f: Finding, round: number, maxRounds: number) => f.severity === 'blocker' || (f.severity === 'major' && maxRounds > 2 && round > 1);

export function feedbackFrom(r: RoundReport): string {
  const parts: string[] = [];
  for (const c of r.checks) if (c.status === 'fail') parts.push(`CHECK FAILED: ${c.name}\n${c.detail}`);
  for (const f of r.findings) parts.push(`REVIEW (${f.severity}) ${f.file}:${f.line}: ${f.claim}`);
  return `Verification round ${r.round} did not pass. Fix these problems (only what is listed), then stop:\n\n${parts.join('\n\n')}`;
}

/** Rounds of parallel deterministic checks + an LLM review; deterministic results decide. */
export async function verifyLoop(o: VerifyOptions): Promise<VerifyReport> {
  const rounds: RoundReport[] = [];
  const stat = { raised: 0, valid: 0, dropped: 0, blockers: 0 };
  const hasRunnable = o.fp.verify.some((c) => c.kind === 'test' || c.kind === 'build' || c.kind === 'types');
  for (let round = 1; round <= o.maxRounds; round++) {
    if (o.signal?.aborted) break;
    const [cmdRes, dc] = await Promise.all([runCommands(o.cwd, o.fp.verify, o.signal), diffChecks(o.cwd, o.cp, o.base, { allowedGlobs: o.allowedGlobs })]);
    const checks = [...cmdRes, ...dc.results];
    checks.forEach((c) => o.onCheck?.(c.name, c.status));
    let findings: Finding[] = [], dropped = 0, ran = false;
    if (o.llm && dc.diff.trim() && dc.changed.length) {
      try {
        const r = await review(o.llm, o.cwd, { diff: dc.diff, changed: dc.changed.filter((f) => !dc.results.some(() => false)), acceptance: o.acceptance, checklist: CHECKLISTS[Math.min(round - 1, CHECKLISTS.length - 1)], signal: o.signal });
        findings = r.valid; dropped = r.dropped; ran = !r.malformed;
        stat.raised += r.valid.length + r.dropped; stat.valid += r.valid.length; stat.dropped += r.dropped;
      } catch (e) { if ((e as Error).name === 'AbortError') break; /* reviewer failure never blocks: deterministic checks decide */ }
    }
    const blocking = findings.filter((f) => isBlocking(f, round, o.maxRounds));
    stat.blockers += blocking.length;
    const detFail = checks.some((c) => c.status === 'fail');
    const verdict: Verdict = detFail || blocking.length ? 'fail' : hasRunnable ? 'pass' : 'unverified';
    const report: RoundReport = { round, checks, findings: blocking.length ? blocking : findings.filter((f) => f.severity !== 'minor'), droppedFindings: dropped, reviewerRan: ran, verdict };
    rounds.push(report); o.onRound?.(report);
    if (verdict !== 'fail') {
      return { verdict, rounds, changed: dc.changed, reviewer: stat, reason: verdict === 'pass' ? `passed in round ${round}` : 'no build/test command available: result is unverified' };
    }
    if (round < o.maxRounds) await o.fix(feedbackFrom(report), round);
  }
  const last = rounds.at(-1);
  return { verdict: 'fail', rounds, changed: [], reviewer: stat, reason: last ? `still failing after ${rounds.length} round(s): ${last.checks.filter((c) => c.status === 'fail').map((c) => c.name).concat(last.findings.map((f) => `${f.file}:${f.line}`)).join(', ')}` : 'aborted' };
}
