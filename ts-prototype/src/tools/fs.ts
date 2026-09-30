import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { dirname, join } from 'node:path';
import { globToRegExp, inWorkspace, matchesAny, rel } from '../util/paths.ts';
import { clip, run } from '../util/proc.ts';
import { num, str, type Tool, type ToolCtx } from './types.ts';

const hash = (s: string) => createHash('sha1').update(s).digest('hex');
const IGNORE = new Set(['.git', 'node_modules', 'bin', 'obj', 'dist', 'build', '.next', '.venv', '__pycache__', 'target', '.fh', 'coverage']);

function assertWritable(ctx: ToolCtx, abs: string) {
  const r = rel(ctx.cwd, abs);
  if (r.startsWith('.git/') || r === '.git') throw new Error('refusing to modify .git');
  if (ctx.ownedGlobs && !matchesAny(r, ctx.ownedGlobs)) throw new Error(`file ownership: ${r} is not owned by this worker (owned: ${ctx.ownedGlobs.join(', ')})`);
}

export const readFile: Tool = {
  readOnly: true,
  spec: {
    name: 'read_file',
    description: 'Read a text file. Returns numbered lines. Use offset/limit for large files.',
    parameters: { type: 'object', properties: { path: { type: 'string' }, offset: { type: 'integer', description: '1-based first line' }, limit: { type: 'integer' } }, required: ['path'] },
  },
  async execute(a, ctx) {
    const abs = inWorkspace(ctx.cwd, str(a, 'path'));
    if (!existsSync(abs)) return { ok: false, output: `not found: ${str(a, 'path')}` };
    if (statSync(abs).isDirectory()) return { ok: false, output: 'is a directory; use list_files' };
    const text = readFileSync(abs, 'utf8');
    ctx.readCache.set(abs, hash(text));
    const lines = text.split('\n');
    const off = Math.max(1, num(a, 'offset') ?? 1);
    const lim = Math.min(num(a, 'limit') ?? 400, 2000);
    const slice = lines.slice(off - 1, off - 1 + lim);
    const out = slice.map((l, i) => `${off + i}\t${l}`).join('\n');
    const more = off - 1 + lim < lines.length ? `\n… ${lines.length - (off - 1 + lim)} more lines (use offset)` : '';
    return { ok: true, output: clip(out) + more };
  },
};

function walk(root: string, dir: string, out: string[], max: number) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    if (out.length >= max) return;
    if (IGNORE.has(e.name)) continue;
    const p = join(dir, e.name);
    if (e.isDirectory()) walk(root, p, out, max);
    else out.push(rel(root, p));
  }
}

export function listAll(cwd: string, max = 5000): string[] {
  const out: string[] = [];
  walk(cwd, cwd, out, max);
  return out;
}

export const listFiles: Tool = {
  readOnly: true,
  spec: {
    name: 'list_files',
    description: 'List files (recursive, skips node_modules/.git/build dirs). Optional glob like "src/**/*.ts".',
    parameters: { type: 'object', properties: { glob: { type: 'string' }, dir: { type: 'string' } } },
  },
  async execute(a, ctx) {
    const base = a.dir ? inWorkspace(ctx.cwd, str(a, 'dir')) : ctx.cwd;
    const out: string[] = [];
    walk(ctx.cwd, base, out, 5000);
    const g = str(a, 'glob', false);
    const files = g ? out.filter((f) => globToRegExp(g).test(f)) : out;
    return { ok: true, output: files.length ? clip(files.slice(0, 500).join('\n')) + (files.length > 500 ? `\n… ${files.length - 500} more` : '') : '(no files)' };
  },
};

export const grep: Tool = {
  readOnly: true,
  spec: {
    name: 'grep',
    description: 'Search file contents with a regex. Returns path:line:text. Optional glob filter.',
    parameters: { type: 'object', properties: { pattern: { type: 'string' }, glob: { type: 'string' }, ignore_case: { type: 'boolean' } }, required: ['pattern'] },
  },
  async execute(a, ctx) {
    let re: RegExp;
    try { re = new RegExp(str(a, 'pattern'), a.ignore_case ? 'i' : ''); } catch (e) { return { ok: false, output: `bad regex: ${(e as Error).message}` }; }
    const g = str(a, 'glob', false);
    const gre = g ? globToRegExp(g) : undefined;
    const hits: string[] = [];
    for (const f of listAll(ctx.cwd)) {
      if (gre && !gre.test(f)) continue;
      let text: string;
      try { const abs = join(ctx.cwd, f); if (statSync(abs).size > 1_000_000) continue; text = readFileSync(abs, 'utf8'); } catch { continue; }
      if (text.includes('\0')) continue;
      const lines = text.split('\n');
      for (let i = 0; i < lines.length && hits.length < 200; i++) if (re.test(lines[i])) hits.push(`${f}:${i + 1}:${lines[i].slice(0, 240)}`);
      if (hits.length >= 200) break;
    }
    return { ok: true, output: hits.length ? hits.join('\n') : '(no matches)' };
  },
};

/** Whitespace-tolerant locate: exact first, then line-trimmed. Returns [start,end] offsets. */
export function locate(text: string, old: string): { start: number; end: number; count: number; fuzzy: boolean } | undefined {
  let count = 0;
  for (let i = text.indexOf(old); i >= 0; i = text.indexOf(old, i + 1)) count++;
  if (count >= 1) {
    const start = text.indexOf(old);
    return { start, end: start + old.length, count, fuzzy: false };
  }
  const tl = text.split('\n'), ol = old.replace(/\n+$/, '').split('\n');
  const norm = (s: string) => s.trim();
  const matches: number[] = [];
  for (let i = 0; i + ol.length <= tl.length; i++) {
    let ok = true;
    for (let j = 0; j < ol.length; j++) if (norm(tl[i + j]) !== norm(ol[j])) { ok = false; break; }
    if (ok) matches.push(i);
  }
  if (!matches.length) return undefined;
  const i = matches[0];
  const start = tl.slice(0, i).join('\n').length + (i > 0 ? 1 : 0);
  const end = start + tl.slice(i, i + ol.length).join('\n').length;
  return { start, end, count: matches.length, fuzzy: true };
}

export const edit: Tool = {
  readOnly: false,
  spec: {
    name: 'edit',
    description: 'Replace one exact snippet in an existing file (search/replace). old_text must match exactly once; include enough context. Prefer this over rewriting files.',
    parameters: { type: 'object', properties: { path: { type: 'string' }, old_text: { type: 'string' }, new_text: { type: 'string' }, replace_all: { type: 'boolean' } }, required: ['path', 'old_text', 'new_text'] },
  },
  async execute(a, ctx) {
    const abs = inWorkspace(ctx.cwd, str(a, 'path'));
    assertWritable(ctx, abs);
    if (!existsSync(abs)) return { ok: false, output: `not found: ${str(a, 'path')} (use write_file to create)` };
    const text = readFileSync(abs, 'utf8');
    const prev = ctx.readCache.get(abs);
    if (prev && prev !== hash(text)) return { ok: false, output: 'file changed since you read it; read_file again' };
    const oldT = str(a, 'old_text'), newT = str(a, 'new_text');
    if (oldT === newT) return { ok: false, output: 'old_text and new_text are identical' };
    if (oldT === '') return { ok: false, output: 'old_text is empty' };
    let out: string;
    if (a.replace_all === true && text.includes(oldT)) out = text.split(oldT).join(newT);
    else {
      const loc = locate(text, oldT);
      if (!loc) return { ok: false, output: 'old_text not found. Re-read the file and copy the snippet exactly.' };
      if (loc.count > 1) return { ok: false, output: `old_text matches ${loc.count} places; add surrounding lines to make it unique (or set replace_all).` };
      out = text.slice(0, loc.start) + newT + text.slice(loc.end);
    }
    writeFileSync(abs, out);
    ctx.readCache.set(abs, hash(out));
    ctx.touched.add(rel(ctx.cwd, abs));
    return { ok: true, output: `edited ${rel(ctx.cwd, abs)}` };
  },
};

export const writeNew: Tool = {
  readOnly: false,
  spec: {
    name: 'write_file',
    description: 'Create a NEW file. Fails if the file exists (use edit instead).',
    parameters: { type: 'object', properties: { path: { type: 'string' }, content: { type: 'string' } }, required: ['path', 'content'] },
  },
  async execute(a, ctx) {
    const abs = inWorkspace(ctx.cwd, str(a, 'path'));
    assertWritable(ctx, abs);
    if (existsSync(abs)) return { ok: false, output: 'file exists; use edit for changes' };
    mkdirSync(dirname(abs), { recursive: true });
    writeFileSync(abs, str(a, 'content'));
    ctx.touched.add(rel(ctx.cwd, abs));
    return { ok: true, output: `created ${rel(ctx.cwd, abs)}` };
  },
};

export const bash: Tool = {
  readOnly: false,
  spec: {
    name: 'bash',
    description: 'Run a shell command in the workspace (bash; PowerShell on Windows). Use for builds, tests, git. Output is truncated.',
    parameters: { type: 'object', properties: { command: { type: 'string' }, timeout_s: { type: 'integer' } }, required: ['command'] },
  },
  async execute(a, ctx) {
    const cmd = str(a, 'command');
    const t = Math.min((num(a, 'timeout_s') ?? 120) * 1000, ctx.bashTimeoutMs ?? 600_000);
    const r = await run(cmd, { cwd: ctx.cwd, timeoutMs: t, signal: ctx.signal, wrap: ctx.wrapShell });
    const body = clip([r.stdout, r.stderr && `[stderr]\n${r.stderr}`].filter(Boolean).join('\n'));
    const tag = r.aborted ? ' (cancelled)' : r.timedOut ? ` (timed out after ${t / 1000}s)` : '';
    return { ok: r.code === 0 && !r.timedOut && !r.aborted, output: `exit ${r.code}${tag}\n${body}`.trim() };
  },
};
