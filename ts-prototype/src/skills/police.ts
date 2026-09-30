import type { SkillStore } from './store.ts';

export interface PoliceConfig { minUses: number; maxWorse: number; promoteN: number }
export const POLICE_DEFAULTS: PoliceConfig = { minUses: 5, maxWorse: 0.25, promoteN: 3 };

export interface SkillHealth { skillId: string; name: string; used: number; failUsed: number; base: number; failBase: number; action: 'none' | 'quarantine' | 'rollback' }

const rate = (f: number, n: number) => (n ? f / n : 0);

/**
 * Internal A/B: compare failure rate of tasks that used a skill with tasks in the same project that did not use it.
 * The user has no debug commands, so the harness polices skill quality itself.
 * - Worse than baseline by `maxWorse` with enough samples: roll back to the previous version if that version's era
 *   was healthier, otherwise quarantine.
 */
export function evaluate(store: SkillStore, cfg: PoliceConfig = POLICE_DEFAULTS): SkillHealth[] {
  const out: SkillHealth[] = [];
  const skills = [...store.builtins, ...store.userSkills()];
  for (const s of skills) {
    if (s.state !== 'active') continue;
    const used = store.db.prepare(`SELECT t.verdict v, ts.skill_version ver FROM task_skills ts JOIN tasks t ON t.id=ts.task_id WHERE ts.skill_id=?`).all(s.id) as any[];
    if (used.length < cfg.minUses) continue;
    const projects = [...new Set((store.db.prepare(`SELECT DISTINCT t.project_id p FROM task_skills ts JOIN tasks t ON t.id=ts.task_id WHERE ts.skill_id=?`).all(s.id) as any[]).map((r) => r.p))];
    const ph = projects.map(() => '?').join(',');
    const base = store.db.prepare(`SELECT verdict v FROM tasks WHERE project_id IN (${ph}) AND id NOT IN (SELECT task_id FROM task_skills WHERE skill_id=?)`).all(...projects, s.id) as any[];
    const failUsed = used.filter((u) => u.v === 'fail').length, failBase = base.filter((b) => b.v === 'fail').length;
    let action: SkillHealth['action'] = 'none';
    if (base.length >= cfg.minUses && rate(failUsed, used.length) - rate(failBase, base.length) >= cfg.maxWorse) {
      const cur = used.filter((u) => u.ver === s.version), prev = used.filter((u) => u.ver === s.version - 1);
      const curFail = rate(cur.filter((u) => u.v === 'fail').length, cur.length);
      const prevFail = rate(prev.filter((u) => u.v === 'fail').length, prev.length);
      if (s.version > 1 && prev.length >= 3 && cur.length >= 3 && curFail > prevFail) { store.rollback(s.id, `fail rate ${(curFail * 100) | 0}% vs ${(prevFail * 100) | 0}% before`); action = 'rollback'; }
      else { store.quarantine(s.id, `tasks using it fail ${(rate(failUsed, used.length) * 100) | 0}% vs ${(rate(failBase, base.length) * 100) | 0}% without`); action = 'quarantine'; }
    }
    out.push({ skillId: s.id, name: s.name, used: used.length, failUsed, base: base.length, failBase, action });
  }
  return out;
}
