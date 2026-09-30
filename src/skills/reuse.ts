import { similarity, type Fingerprint } from '../fingerprint.ts';
import type { Skill, SkillStore } from './store.ts';

export interface ReuseOffer {
  fromProject: string;
  fromLabel: string;
  similarity: number;
  /** names and one-line summaries only, never bodies */
  skills: { id: string; name: string; summary: string }[];
}

/**
 * On a new/unfamiliar repo: find project-scoped skills from other projects with a similar fingerprint.
 * Returns an offer (names and summaries only) to ask the user once.
 */
export function findReuseOffers(store: SkillStore, fp: Fingerprint, minSim = 0.6): ReuseOffer[] {
  if (store.userSkills(fp.projectId).some((s) => s.scope === 'project' && s.projectId === fp.projectId)) return [];
  const offers: ReuseOffer[] = [];
  for (const p of store.otherProjects(fp.projectId)) {
    const sim = similarity(p.tokens, fp.tokens);
    if (sim < minSim) continue;
    const skills = store.userSkills().filter((s) => s.scope === 'project' && s.projectId === p.id && s.state === 'active' && s.source !== 'builtin');
    if (skills.length) offers.push({ fromProject: p.id, fromLabel: p.label, similarity: sim, skills: skills.map((s) => ({ id: s.id, name: s.name, summary: s.summary })) });
  }
  return offers.sort((a, b) => b.similarity - a.similarity);
}

/** Copy (not link) chosen skills into the new project, tagged with origin. */
export function acceptReuse(store: SkillStore, fp: Fingerprint, offer: ReuseOffer, ids: string[]): number {
  let n = 0;
  for (const id of ids) {
    const s = store.get(id) as Skill | undefined;
    if (!s || !offer.skills.some((x) => x.id === id)) continue;
    const r = store.add({ name: s.name, scope: 'project', projectId: fp.projectId, source: 'reused', origin: `${offer.fromLabel}`, summary: s.summary, keywords: s.keywords, body: s.body }, `copied from ${offer.fromLabel}`);
    if (r.ok) n++;
  }
  return n;
}
