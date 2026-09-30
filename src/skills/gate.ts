import type { Fingerprint } from '../fingerprint.ts';
import { Bm25, tokenize } from './bm25.ts';
import type { Skill, SkillStore } from './store.ts';
import type { UserConfig, UserSkill } from './usercfg.ts';

export interface GateResult {
  decision: 'use' | 'create';
  selected: Skill[];
  userSkills: UserSkill[];
  /** Set when the deterministic match is unclear; the planner resolves it inside its structured output (no extra turn). */
  ambiguous: string[];
  ms: number;
  llmCalls: 0;
}

const BOOST: Record<string, number> = { project: 1.3, stack: 1.15, global: 1.0 };
const MAX_SELECTED = 3;
const MIN_SCORE = 0.8;
const MIN_TASK_WORDS = 4;

/**
 * Deterministic skill gate: repo fingerprint + BM25. Never calls the model.
 * Stack packs that match the repo fingerprint are always eligible so version guidance is not lost
 * when the task text does not mention the framework.
 */
export function gate(store: SkillStore, fp: Fingerprint, task: string, user: UserConfig): GateResult {
  const t0 = performance.now();
  const pool = store.allForProject(fp);
  const bm = new Bm25(pool.map((s) => ({ id: s.id, text: `${s.name} ${s.summary} ${s.body}`, boost: `${s.name} ${s.keywords} ${s.stack ?? ''}` })));
  const q = [...tokenize(task), ...fp.tokens.flatMap((t) => tokenize(t))];
  const scores = bm.score(q);
  const ranked = pool.map((s) => {
    let sc = (scores.get(s.id) ?? 0) * BOOST[s.scope];
    if (s.scope === 'stack') sc += 1.2; // stack pack matched the repo fingerprint
    if (s.scope === 'project') sc += 0.6;
    return { s, sc };
  }).filter((x) => x.sc >= MIN_SCORE).sort((a, b) => b.sc - a.sc);

  const selected: Skill[] = [];
  // at most one global persona unless the task text strongly matches more
  let globals = 0;
  for (const { s, sc } of ranked) {
    if (selected.length >= MAX_SELECTED) break;
    if (s.scope === 'global' && globals >= 1 && sc < ranked[0].sc * 0.8) continue;
    if (s.scope === 'global') globals++;
    selected.push(s);
  }
  const globalRanked = ranked.filter((x) => x.s.scope === 'global');
  const ambiguous = globalRanked.length >= 2 && globalRanked[0].sc - globalRanked[1].sc < globalRanked[0].sc * 0.1 && globalRanked[0].sc < 3
    ? [globalRanked[0].s.name, globalRanked[1].s.name] : [];

  // user SKILL.md skills matched by description/name
  const ubm = new Bm25(user.skills.map((u) => ({ id: u.path, text: `${u.name} ${u.description} ${u.body.slice(0, 400)}`, boost: `${u.name} ${u.description}` })));
  const us = ubm.score(q);
  const userSkills = user.skills.filter((u) => (us.get(u.path) ?? 0) >= 1.2).slice(0, 3);

  const words = tokenize(task).length;
  const decision: GateResult['decision'] = selected.some((s) => s.scope !== 'global') || userSkills.length || selected.length ? 'use' : words >= MIN_TASK_WORDS ? 'create' : 'use';
  return { decision, selected, userSkills, ambiguous, ms: performance.now() - t0, llmCalls: 0 };
}

/** Render the gate output for the model: after the stable prefix; user rules last and marked higher priority. */
export function renderSkills(g: GateResult): string {
  const parts: string[] = [];
  for (const s of g.selected) parts.push(`## ${s.name}\n${s.body}`);
  return parts.join('\n\n');
}

export function renderUser(user: UserConfig, g: GateResult): string {
  const parts: string[] = [];
  if (user.rules) parts.push(user.rules);
  for (const u of g.userSkills) parts.push(`# Skill: ${u.name}\n${u.body}`);
  return parts.join('\n\n');
}
