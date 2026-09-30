import type { Subtask } from '../funnel/intake.ts';

const prefix = (g: string) => g.replace(/\*.*$/, '').replace(/\/?$/, '/').replace(/^\.\//, '');

/** Two ownership globs overlap when one directory prefix contains the other or the paths are identical. */
export function globsOverlap(a: string, b: string): boolean {
  if (a === b) return true;
  const isFile = (g: string) => !g.includes('*') && /\.[A-Za-z0-9]+$/.test(g);
  if (isFile(a) && isFile(b)) return a === b;
  const pa = isFile(a) ? a : prefix(a), pb = isFile(b) ? b : prefix(b);
  return pa.startsWith(pb) || pb.startsWith(pa);
}

export const subtasksOverlap = (a: Subtask, b: Subtask) => a.files.some((x) => b.files.some((y) => globsOverlap(x, y)));

/**
 * Fan out only when file ownership does not overlap: returns waves (arrays run in parallel; waves run in order).
 * Overlapping or dependent subtasks land in separate waves. A malformed plan (unknown deps, cycles, no files)
 * falls back to fully serial execution.
 */
export function planWaves(subtasks: Subtask[]): Subtask[][] {
  const ids = new Set(subtasks.map((s) => s.id));
  const bad = subtasks.some((s) => !s.files.length || s.deps.some((d) => !ids.has(d) || d === s.id)) || ids.size !== subtasks.length;
  if (bad) return subtasks.map((s) => [s]);
  const done = new Set<string>();
  const waves: Subtask[][] = [];
  let left = [...subtasks];
  while (left.length) {
    const wave: Subtask[] = [];
    for (const s of left) {
      if (!s.deps.every((d) => done.has(d))) continue;
      if (wave.some((w) => subtasksOverlap(w, s))) continue;
      wave.push(s);
    }
    if (!wave.length) return subtasks.map((s) => [s]); // cycle
    wave.forEach((w) => done.add(w.id));
    left = left.filter((s) => !wave.includes(s));
    waves.push(wave);
  }
  return waves;
}
