import { run } from '../util/proc.ts';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

/** Git-backed per-turn snapshots that do not touch the user's index, branch or stash. */
export class Checkpoints {
  cwd: string;
  constructor(cwd: string) { this.cwd = cwd; }

  async isRepo(): Promise<boolean> {
    return (await run('git rev-parse --is-inside-work-tree', { cwd: this.cwd })).code === 0;
  }

  private async withIndex<T>(fn: (env: NodeJS.ProcessEnv) => Promise<T>): Promise<T> {
    const d = mkdtempSync(join(tmpdir(), 'fh-idx-'));
    try { return await fn({ ...process.env, GIT_INDEX_FILE: join(d, 'index'), GIT_AUTHOR_NAME: 'fh', GIT_AUTHOR_EMAIL: 'fh@local', GIT_COMMITTER_NAME: 'fh', GIT_COMMITTER_EMAIL: 'fh@local' }); }
    finally { rmSync(d, { recursive: true, force: true }); }
  }

  /** Snapshot the working tree (including untracked, respecting .gitignore). Returns commit id. */
  async create(label: string): Promise<string | undefined> {
    if (!(await this.isRepo())) return undefined;
    return this.withIndex(async (env) => {
      const add = await run('git add -A', { cwd: this.cwd, env });
      if (add.code !== 0) return undefined;
      const tree = (await run('git write-tree', { cwd: this.cwd, env })).stdout.trim();
      if (!tree) return undefined;
      const head = await run('git rev-parse -q --verify HEAD', { cwd: this.cwd, env });
      const parent = head.code === 0 ? `-p ${head.stdout.trim()}` : '';
      const c = await run(`git commit-tree ${tree} ${parent} -m ${JSON.stringify('fh checkpoint: ' + label)}`, { cwd: this.cwd, env });
      const id = c.stdout.trim();
      if (!id) return undefined;
      await run(`git update-ref refs/fh/checkpoints/${Date.now()} ${id}`, { cwd: this.cwd, env });
      return id;
    });
  }

  /** Files changed between a checkpoint and the current working tree. */
  async changedSince(id: string): Promise<{ added: string[]; modified: string[]; deleted: string[] }> {
    return this.withIndex(async (env) => {
      await run('git add -A', { cwd: this.cwd, env });
      const r = await run(`git diff --cached --name-status ${id} --`, { cwd: this.cwd, env });
      const out = { added: [] as string[], modified: [] as string[], deleted: [] as string[] };
      for (const l of r.stdout.split('\n').filter(Boolean)) {
        const [s, ...p] = l.split('\t'); const f = p.at(-1)!;
        if (s === 'A') out.added.push(f); else if (s === 'D') out.deleted.push(f); else out.modified.push(f);
      }
      return out;
    });
  }

  /** Unified diff of the working tree vs the checkpoint (new files included). */
  async diffSince(id: string): Promise<string> {
    return this.withIndex(async (env) => {
      await run('git add -A', { cwd: this.cwd, env });
      return (await run(`git diff --cached --no-color -U3 ${id} --`, { cwd: this.cwd, env })).stdout;
    });
  }

  /** Restore the working tree to the checkpoint: revert modifications, restore deletions, remove additions. */
  async restore(id: string): Promise<string[]> {
    const ch = await this.changedSince(id);
    for (const f of [...ch.modified, ...ch.deleted]) await run(`git checkout ${id} -- ${JSON.stringify(f)}`, { cwd: this.cwd });
    for (const f of ch.added) { const p = join(this.cwd, f); if (existsSync(p)) rmSync(p, { force: true }); }
    return [...ch.added, ...ch.modified, ...ch.deleted];
  }
}
