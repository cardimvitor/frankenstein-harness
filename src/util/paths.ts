import { realpathSync, existsSync } from 'node:fs';
import { dirname, isAbsolute, relative, resolve, sep } from 'node:path';

/** Resolve a user/model path inside the workspace; throws when it escapes (incl. via symlinks). */
export function inWorkspace(root: string, p: string): string {
  const abs = resolve(root, p);
  const rootReal = realpathSync(root);
  let probe = abs;
  while (!existsSync(probe) && dirname(probe) !== probe) probe = dirname(probe);
  const real = realpathSync(probe);
  const rel = relative(rootReal, real);
  if (rel.startsWith('..') || isAbsolute(rel)) throw new Error(`path escapes workspace: ${p}`);
  return abs;
}

export const rel = (root: string, abs: string) => relative(root, abs).split(sep).join('/');

/** Minimal glob -> RegExp (supports **, *, ?). */
export function globToRegExp(glob: string): RegExp {
  let re = '';
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === '*') {
      if (glob[i + 1] === '*') { re += '.*'; i++; if (glob[i + 1] === '/') i++; }
      else re += '[^/]*';
    } else if (c === '?') re += '[^/]';
    else re += c.replace(/[.+^${}()|[\]\\]/g, '\\$&');
  }
  return new RegExp(`^${re}$`);
}

export const matchesAny = (path: string, globs: string[]) => globs.some((g) => globToRegExp(g).test(path) || path.startsWith(g.replace(/\/\*\*$/, '/')));
