import { execSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { LlmClient } from '../llm/client.ts';
import type { Fingerprint } from '../fingerprint.ts';
import { listAll } from '../tools/fs.ts';
import { clip } from '../util/proc.ts';

export interface Subtask { id: string; goal: string; files: string[]; deps: string[] }
export interface PlanStep { step: string; files: string[] }
export interface Intake {
  trivial: boolean;
  questions: string[];
  enriched: string;
  acceptance: string[];
  plan: PlanStep[];
  assumptions: string[];
  subtasks: Subtask[];
  skillChoice?: string;
}

export const INTAKE_SCHEMA = {
  type: 'object',
  properties: {
    trivial: { type: 'boolean', description: 'true for a small, obvious, low-risk change (one file, clear intent)' },
    questions: { type: 'array', items: { type: 'string' }, description: 'only questions whose answers change the implementation; max 5' },
    enriched: { type: 'string', description: 'the task rewritten with constraints made explicit' },
    acceptance: { type: 'array', items: { type: 'string' } },
    plan: { type: 'array', items: { type: 'object', properties: { step: { type: 'string' }, files: { type: 'array', items: { type: 'string' } } }, required: ['step', 'files'] } },
    assumptions: { type: 'array', items: { type: 'string' } },
    subtasks: { type: 'array', items: { type: 'object', properties: { id: { type: 'string' }, goal: { type: 'string' }, files: { type: 'array', items: { type: 'string' } }, deps: { type: 'array', items: { type: 'string' } } }, required: ['id', 'goal', 'files', 'deps'] }, description: 'ONLY when the work splits into independent parts touching different files; otherwise empty' },
    skill_choice: { type: 'string', description: 'when asked to choose between candidate guidance notes, the chosen name' },
  },
  required: ['trivial', 'questions', 'enriched', 'acceptance', 'plan', 'assumptions', 'subtasks'],
};

/** Deterministic repo inspection that feeds the planner (no model calls). */
export function inspectRepo(cwd: string, fp: Fingerprint): string {
  const files = listAll(cwd, 3000);
  const tree = files.slice(0, 80).join('\n') + (files.length > 80 ? `\n… ${files.length - 80} more files` : '');
  let git = '';
  try { git = execSync('git status --short | head -15', { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim(); } catch { /* not a repo */ }
  const readme = ['README.md', 'readme.md', 'README'].map((f) => join(cwd, f)).find(existsSync);
  const r = readme ? readFileSync(readme, 'utf8').slice(0, 800) : '';
  return [
    `Stack: ${fp.summary}`,
    `Verification commands: ${fp.verify.map((v) => `${v.name} (${v.cmd})`).join('; ') || 'none detected'}`,
    `Files:\n${tree}`, git ? `Uncommitted changes:\n${git}` : '', r ? `README excerpt:\n${r}` : '',
  ].filter(Boolean).join('\n\n');
}

export interface IntakeArgs {
  llm: LlmClient;
  task: string;
  repo: string;
  mode: 'normal' | 'auto';
  answers?: { question: string; answer: string }[];
  ambiguous?: string[];
  maxSubtasks?: number;
  signal?: AbortSignal;
}

const norm = (v: any): Intake => ({
  trivial: !!v?.trivial,
  questions: Array.isArray(v?.questions) ? v.questions.filter((q: unknown) => typeof q === 'string' && q.trim()).slice(0, 5) : [],
  enriched: typeof v?.enriched === 'string' && v.enriched.trim() ? v.enriched : '',
  acceptance: Array.isArray(v?.acceptance) ? v.acceptance.filter((x: unknown) => typeof x === 'string') : [],
  plan: Array.isArray(v?.plan) ? v.plan.filter((p: any) => p && typeof p.step === 'string').map((p: any) => ({ step: p.step, files: Array.isArray(p.files) ? p.files.filter((f: unknown) => typeof f === 'string') : [] })) : [],
  assumptions: Array.isArray(v?.assumptions) ? v.assumptions.filter((x: unknown) => typeof x === 'string') : [],
  subtasks: Array.isArray(v?.subtasks) ? v.subtasks.filter((s: any) => s && typeof s.id === 'string' && typeof s.goal === 'string' && Array.isArray(s.files)).map((s: any) => ({ id: s.id, goal: s.goal, files: s.files.filter((f: unknown) => typeof f === 'string'), deps: Array.isArray(s.deps) ? s.deps.filter((d: unknown) => typeof d === 'string') : [] })) : [],
  skillChoice: typeof v?.skill_choice === 'string' ? v.skill_choice : undefined,
});

/** One structured planner call (thinking on). Normal mode may return questions; auto mode never asks, it records assumptions. */
export async function intake(a: IntakeArgs): Promise<Intake> {
  const rules = a.mode === 'auto'
    ? 'Do NOT ask questions. Resolve ambiguity yourself and list each assumption in "assumptions".'
    : 'Ask at most 5 questions, only when the answer changes the implementation and cannot be found in the repository. If you can infer it, do not ask.';
  const prompt = [
    'You are the planning stage of a coding agent. Inspect the summary below and produce a structured plan.',
    rules,
    'Write acceptance criteria that can be checked (behavior, tests, files that must not change).',
    'Plan steps must name the files they touch. Set trivial=true only for a small obvious change.',
    `Only split into "subtasks" when the parts are independent and touch DISJOINT files; otherwise leave subtasks empty.${a.maxSubtasks ? ` At most ${a.maxSubtasks}.` : ''}`,
    a.ambiguous?.length ? `If relevant, pick one guidance note and put its name in skill_choice: ${a.ambiguous.join(' | ')}` : '',
    a.answers?.length ? `Answers already given:\n${a.answers.map((x) => `Q: ${x.question}\nA: ${x.answer}`).join('\n')}\nDo not ask further questions.` : '',
    `Task:\n${a.task}`, `Repository:\n${clip(a.repo, 7000)}`,
  ].filter(Boolean).join('\n\n');
  const { value } = await a.llm.json({
    messages: [{ role: 'system', content: 'You output only JSON matching the schema.' }, { role: 'user', content: prompt }],
    thinking: 'high', maxTokens: 3500, jsonSchema: INTAKE_SCHEMA, signal: a.signal,
  });
  const out = norm(value);
  if (!out.enriched) out.enriched = a.task;
  if (a.mode === 'auto' || a.answers?.length) out.questions = [];
  return out;
}

export function renderPlan(i: Intake): string {
  const lines = i.plan.map((p, n) => `${n + 1}. ${p.step}${p.files.length ? `  [${p.files.join(', ')}]` : ''}`);
  const acc = i.acceptance.length ? `\nDone when:\n${i.acceptance.map((x) => `- ${x}`).join('\n')}` : '';
  const ass = i.assumptions.length ? `\nAssumptions:\n${i.assumptions.map((x) => `- ${x}`).join('\n')}` : '';
  return `${lines.join('\n') || '(no steps)'}${acc}${ass}`;
}
