import { DatabaseSync } from 'node:sqlite';
import { mkdirSync, readdirSync, readFileSync } from 'node:fs';
import { createHash, randomUUID } from 'node:crypto';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { dataDir } from '../config.ts';
import { parseFrontmatter, versionMatcher } from './frontmatter.ts';
import { validateSkillBody } from './validate.ts';
import type { Fingerprint } from '../fingerprint.ts';

export type Scope = 'project' | 'stack' | 'global';
export type Verdict = 'pass' | 'fail' | 'unverified';

export interface Skill {
  id: string;
  name: string;
  scope: Scope;
  /** stack id (e.g. react) or token (react@18) for stack scope */
  stack?: string;
  versions?: string;
  projectId?: string;
  source: 'builtin' | 'auto' | 'reused';
  origin?: string;
  summary: string;
  keywords: string;
  body: string;
  version: number;
  hash: string;
  state: 'active' | 'quarantined';
}

const h = (s: string) => createHash('sha256').update(s).digest('hex').slice(0, 16);
const BUILTIN_DIR = join(dirname(fileURLToPath(import.meta.url)), 'builtin');

export function loadBuiltins(dir = BUILTIN_DIR): Skill[] {
  const out: Skill[] = [];
  for (const f of readdirSync(dir).filter((x) => x.endsWith('.md')).sort()) {
    const { meta, body } = parseFrontmatter(readFileSync(join(dir, f), 'utf8'));
    if (!meta.name) continue;
    out.push({
      id: `builtin:${meta.name}`, name: meta.name, scope: (meta.scope as Scope) ?? 'global', stack: meta.stack, versions: meta.versions,
      source: 'builtin', summary: meta.summary ?? '', keywords: meta.keywords ?? '', body, version: 1, hash: h(body), state: 'active',
    });
  }
  return out;
}

export function skillApplies(s: Skill, fp: Fingerprint): boolean {
  if (s.state !== 'active') return false;
  if (s.scope === 'global') return true;
  if (s.scope === 'project') return s.projectId === fp.projectId;
  const st = s.stack ?? '';
  const [id, ver] = st.split('@');
  const vm = versionMatcher(s.versions);
  return fp.stacks.some((x) => x.id === id && (ver ? x.version === ver : vm(x.version)));
}

export class SkillStore {
  db: DatabaseSync;
  builtins: Skill[];
  constructor(path = join(dataDir(), 'skills.db'), builtins = loadBuiltins()) {
    if (path !== ':memory:') mkdirSync(dirname(path), { recursive: true });
    this.db = new DatabaseSync(path);
    this.builtins = builtins;
    this.db.exec(`
      PRAGMA journal_mode=WAL;
      CREATE TABLE IF NOT EXISTS skills(id TEXT PRIMARY KEY, name TEXT, scope TEXT, stack TEXT, versions TEXT, project_id TEXT, source TEXT, origin TEXT, summary TEXT, keywords TEXT, body TEXT, version INTEGER, hash TEXT, state TEXT, created INTEGER, updated INTEGER);
      CREATE TABLE IF NOT EXISTS skill_versions(skill_id TEXT, version INTEGER, hash TEXT, body TEXT, diff TEXT, reason TEXT, ts INTEGER);
      CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY, label TEXT, stack_key TEXT, tokens TEXT, first_seen INTEGER, last_seen INTEGER);
      CREATE TABLE IF NOT EXISTS tasks(id TEXT PRIMARY KEY, project_id TEXT, verdict TEXT, rounds INTEGER, ts INTEGER);
      CREATE TABLE IF NOT EXISTS task_skills(task_id TEXT, skill_id TEXT, skill_version INTEGER);
      CREATE TABLE IF NOT EXISTS activity(ts INTEGER, kind TEXT, skill TEXT, reason TEXT, project_id TEXT);
      CREATE TABLE IF NOT EXISTS suppressed(skill_id TEXT PRIMARY KEY, reason TEXT, ts INTEGER);
    `);
  }
  close() { this.db.close(); }

  // ---- activity log (shown as notices in CLI/web; never as skill content)
  log(kind: 'created' | 'improved' | 'used' | 'reused' | 'quarantined' | 'rolled_back' | 'promoted', skill: string, reason: string, projectId?: string) {
    this.db.prepare('INSERT INTO activity VALUES (?,?,?,?,?)').run(Date.now(), kind, skill, reason, projectId ?? null);
  }
  activity(limit = 50): { ts: number; kind: string; skill: string; reason: string }[] {
    return this.db.prepare('SELECT ts,kind,skill,reason FROM activity ORDER BY ts DESC LIMIT ?').all(limit) as any;
  }

  // ---- projects
  registerProject(fp: Fingerprint, label: string) {
    const now = Date.now();
    this.db.prepare('INSERT INTO projects VALUES (?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET stack_key=excluded.stack_key,tokens=excluded.tokens,last_seen=excluded.last_seen,label=excluded.label')
      .run(fp.projectId, label, fp.stackKey, JSON.stringify(fp.tokens), now, now);
  }
  isKnownProject(id: string): boolean { return !!this.db.prepare('SELECT 1 FROM projects WHERE id=?').get(id); }
  otherProjects(exclude: string): { id: string; label: string; tokens: string[] }[] {
    return (this.db.prepare('SELECT id,label,tokens FROM projects WHERE id<>?').all(exclude) as any[]).map((r) => ({ id: r.id, label: r.label, tokens: JSON.parse(r.tokens) }));
  }

  // ---- skills
  private row(r: any): Skill {
    return { id: r.id, name: r.name, scope: r.scope, stack: r.stack ?? undefined, versions: r.versions ?? undefined, projectId: r.project_id ?? undefined, source: r.source, origin: r.origin ?? undefined, summary: r.summary, keywords: r.keywords, body: r.body, version: r.version, hash: r.hash, state: r.state };
  }
  get(id: string): Skill | undefined {
    if (id.startsWith('builtin:')) return this.builtins.find((b) => b.id === id);
    const r = this.db.prepare('SELECT * FROM skills WHERE id=?').get(id);
    return r ? this.row(r) : undefined;
  }
  userSkills(projectId?: string): Skill[] {
    const rows = this.db.prepare('SELECT * FROM skills').all().map((r) => this.row(r));
    return projectId ? rows.filter((s) => s.scope !== 'project' || s.projectId === projectId) : rows;
  }
  allForProject(fp: Fingerprint): Skill[] {
    const sup = new Set((this.db.prepare('SELECT skill_id FROM suppressed').all() as any[]).map((r) => r.skill_id));
    return [...this.builtins, ...this.userSkills(fp.projectId)].filter((s) => !sup.has(s.id) && skillApplies(s, fp));
  }

  add(s: Omit<Skill, 'id' | 'hash' | 'version' | 'state'>, reason: string): { ok: true; id: string } | { ok: false; reason: string } {
    const v = validateSkillBody(s.body, s.summary, s.name);
    if (!v.ok) return { ok: false, reason: v.reason! };
    if (this.userSkills().some((x) => x.name.toLowerCase() === s.name.toLowerCase() && x.projectId === s.projectId && x.scope === s.scope)) return { ok: false, reason: 'duplicate name' };
    const id = 'u:' + randomUUID().slice(0, 8), now = Date.now(), hash = h(s.body);
    this.db.prepare('INSERT INTO skills VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)').run(id, s.name, s.scope, s.stack ?? null, s.versions ?? null, s.projectId ?? null, s.source, s.origin ?? null, s.summary, s.keywords, s.body, 1, hash, 'active', now, now);
    this.db.prepare('INSERT INTO skill_versions VALUES (?,?,?,?,?,?,?)').run(id, 1, hash, s.body, '', reason, now);
    this.log(s.source === 'reused' ? 'reused' : 'created', s.name, reason, s.projectId);
    return { ok: true, id };
  }

  improve(id: string, body: string, reason: string): { ok: boolean; reason?: string } {
    const cur = this.get(id);
    if (!cur || cur.source === 'builtin') return { ok: false, reason: 'not found or immutable' };
    const v = validateSkillBody(body, cur.summary, cur.name);
    if (!v.ok) return { ok: false, reason: v.reason };
    if (h(body) === cur.hash) return { ok: false, reason: 'no change' };
    const ver = cur.version + 1, now = Date.now();
    const diff = simpleDiff(cur.body, body);
    this.db.prepare('UPDATE skills SET body=?,hash=?,version=?,updated=? WHERE id=?').run(body, h(body), ver, now, id);
    this.db.prepare('INSERT INTO skill_versions VALUES (?,?,?,?,?,?,?)').run(id, ver, h(body), body, diff, reason, now);
    this.log('improved', cur.name, reason, cur.projectId);
    return { ok: true };
  }

  versions(id: string) { return this.db.prepare('SELECT version,hash,diff,reason,ts FROM skill_versions WHERE skill_id=? ORDER BY version').all(id) as any[]; }

  quarantine(id: string, reason: string) {
    const s = this.get(id); if (!s) return;
    if (s.source === 'builtin') this.db.prepare('INSERT OR REPLACE INTO suppressed VALUES (?,?,?)').run(id, reason, Date.now());
    else this.db.prepare("UPDATE skills SET state='quarantined' WHERE id=?").run(id);
    this.log('quarantined', s.name, reason, s.projectId);
  }

  rollback(id: string, reason: string): boolean {
    const s = this.get(id);
    if (!s || s.source === 'builtin' || s.version < 2) return false;
    const prev = this.db.prepare('SELECT body FROM skill_versions WHERE skill_id=? AND version=?').get(id, s.version - 1) as any;
    if (!prev) return false;
    const ver = s.version + 1, now = Date.now();
    this.db.prepare('UPDATE skills SET body=?,hash=?,version=?,updated=? WHERE id=?').run(prev.body, h(prev.body), ver, now, id);
    this.db.prepare('INSERT INTO skill_versions VALUES (?,?,?,?,?,?,?)').run(id, ver, h(prev.body), prev.body, simpleDiff(s.body, prev.body), `rollback: ${reason}`, now);
    this.log('rolled_back', s.name, reason, s.projectId);
    return true;
  }

  promote(id: string, scope: Scope, stack: string | undefined, reason: string) {
    const s = this.get(id); if (!s || s.source === 'builtin') return;
    this.db.prepare('UPDATE skills SET scope=?,stack=?,project_id=? WHERE id=?').run(scope, stack ?? null, null, id);
    this.log('promoted', s.name, `${s.scope} -> ${scope}: ${reason}`, s.projectId);
  }

  // ---- outcomes
  recordTask(taskId: string, projectId: string, skills: Skill[], verdict: Verdict, rounds: number) {
    this.db.prepare('INSERT OR REPLACE INTO tasks VALUES (?,?,?,?,?)').run(taskId, projectId, verdict, rounds, Date.now());
    for (const s of skills) this.db.prepare('INSERT INTO task_skills VALUES (?,?,?)').run(taskId, s.id, s.version);
  }
}

export function simpleDiff(a: string, b: string): string {
  const A = a.split('\n'), B = b.split('\n'), sa = new Set(A), sb = new Set(B);
  return [...A.filter((l) => !sb.has(l)).map((l) => `- ${l}`), ...B.filter((l) => !sa.has(l)).map((l) => `+ ${l}`)].join('\n');
}
