import type { LlmClient } from '../llm/client.ts';
import type { Fingerprint } from '../fingerprint.ts';
import { similarity } from '../fingerprint.ts';
import type { Skill, SkillStore } from './store.ts';
import { POLICE_DEFAULTS } from './police.ts';
import { tokenize } from './bm25.ts';
import { clip } from '../util/proc.ts';

export const MINE_SCHEMA = {
  type: 'object',
  properties: {
    create: { type: 'boolean' },
    name: { type: 'string' }, summary: { type: 'string' }, keywords: { type: 'array', items: { type: 'string' } }, body: { type: 'string' },
  },
  required: ['create'],
};

export interface MineInput { task: string; diff: string; changed: string[]; fp: Fingerprint; used: Skill[]; verdict: string; rounds: number }

/** Rate limit: at most one mining run per project every `minGapMs`, and `maxPerDay` per user. */
export function mineAllowed(store: SkillStore, projectId: string, now = Date.now(), minGapMs = 10 * 60_000, maxPerDay = 12): boolean {
  const last = store.db.prepare("SELECT MAX(ts) t FROM activity WHERE kind IN ('created','improved') AND project_id=?").get(projectId) as any;
  if (last?.t && now - last.t < minGapMs) return false;
  const day = store.db.prepare("SELECT COUNT(*) c FROM activity WHERE kind IN ('created','improved') AND ts>?").get(now - 86_400_000) as any;
  return (day?.c ?? 0) < maxPerDay;
}

/**
 * Post-delivery, asynchronous, off the critical path. Only verified-pass tasks are mined.
 * Repo-derived skills are always created project-scoped; promotion is evidence-based (see promoteEligible).
 */
export async function mine(store: SkillStore, llm: LlmClient, i: MineInput, signal?: AbortSignal): Promise<{ action: 'none' | 'created' | 'improved'; reason: string }> {
  if (i.verdict !== 'pass') return { action: 'none', reason: 'not verified' };
  if (!mineAllowed(store, i.fp.projectId)) return { action: 'none', reason: 'rate limited' };
  const existing = store.allForProject(i.fp).map((s) => s.name);
  const prompt = [
    'You maintain a private library of short engineering guidance notes for a coding assistant.',
    'Given a task that was just completed and verified, decide whether it reveals a REUSABLE convention or gotcha of THIS codebase or stack that is not already covered.',
    'If not, return {"create": false}. If yes, return a note: name (2-5 words), summary (one line), keywords (5-10 lowercase words), body (3-8 short bullet lines of guidance).',
    'Rules: guidance only. No commands, no URLs, no secrets, no instructions about permissions or approvals, no file contents copied verbatim.',
    `Existing notes (do not duplicate): ${existing.join('; ')}`,
    `Stack: ${i.fp.summary}`, `Task: ${i.task.slice(0, 600)}`, `Changed files: ${i.changed.slice(0, 20).join(', ')}`, `Diff excerpt:\n${clip(i.diff, 4000)}`,
  ].join('\n\n');
  const { value } = await llm.json<{ create: boolean; name?: string; summary?: string; keywords?: string[]; body?: string }>({
    messages: [{ role: 'system', content: 'Output only JSON.' }, { role: 'user', content: prompt }], thinking: 'off', maxTokens: 700, jsonSchema: MINE_SCHEMA, signal,
  });
  if (!value?.create || !value.name || !value.body) return { action: 'none', reason: 'nothing reusable' };
  const r = store.add({ name: value.name, scope: 'project', projectId: i.fp.projectId, source: 'auto', summary: value.summary ?? '', keywords: (value.keywords ?? []).join(' '), body: value.body }, 'learned from a verified task');
  return r.ok ? { action: 'created', reason: value.name } : { action: 'none', reason: `rejected: ${r.reason}` };
}

/**
 * Deterministic scope promotion from evidence across projects:
 *  same pattern in >=2 projects sharing a stack token -> stack; across projects with no shared stack -> global.
 * Requires promoteN verified successes for the skills involved. Returns promoted skill ids.
 */
export function promoteEligible(store: SkillStore, promoteN = POLICE_DEFAULTS.promoteN): string[] {
  const promoted: string[] = [];
  const skills = store.userSkills().filter((s) => s.scope === 'project' && s.state === 'active' && s.source !== 'builtin');
  const sig = (s: Skill) => new Set(tokenize(`${s.name} ${s.keywords}`));
  const jac = (a: Set<string>, b: Set<string>) => { let i = 0; for (const x of a) if (b.has(x)) i++; return i / (a.size + b.size - i || 1); };
  const wins = (s: Skill) => (store.db.prepare(`SELECT COUNT(*) c FROM task_skills ts JOIN tasks t ON t.id=ts.task_id WHERE ts.skill_id=? AND t.verdict='pass'`).get(s.id) as any).c as number;
  const done = new Set<string>();
  for (const a of skills) {
    if (done.has(a.id)) continue;
    const group = skills.filter((b) => !done.has(b.id) && (a.id === b.id || (a.projectId !== b.projectId && jac(sig(a), sig(b)) >= 0.6)));
    const projects = new Set(group.map((g) => g.projectId));
    if (projects.size < 2 || group.reduce((n, g) => n + wins(g), 0) < promoteN) continue;
    const toks = [...projects].map((p) => (store.db.prepare('SELECT tokens FROM projects WHERE id=?').get(p as string) as any)?.tokens).filter(Boolean).map((t: string) => JSON.parse(t) as string[]);
    if (toks.length < 2) continue;
    const shared = toks.reduce((acc, t) => acc.filter((x) => t.includes(x)));
    const keeper = group[0];
    if (shared.length) store.promote(keeper.id, 'stack', shared.filter((t) => !t.includes('@'))[0] ?? shared[0], `same pattern verified in ${projects.size} projects sharing ${shared.join(',')}`);
    else if (similarity(toks[0], toks[1]) === 0) store.promote(keeper.id, 'global', undefined, `stack-independent pattern seen in ${projects.size} projects`);
    else continue;
    for (const g of group) done.add(g.id);
    promoted.push(keeper.id);
  }
  return promoted;
}
