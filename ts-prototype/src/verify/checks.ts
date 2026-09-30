import { join } from 'node:path';
import { readFileSync, existsSync } from 'node:fs';
import { run, clip } from '../util/proc.ts';
import { matchesAny } from '../util/paths.ts';
import type { VerifyCmd } from '../fingerprint.ts';
import type { Checkpoints } from '../session/checkpoint.ts';
import { findSecrets } from './secrets.ts';

export interface CheckResult {
  name: string;
  status: 'pass' | 'fail' | 'skipped';
  /** deterministic checks always decide; details are fed back to the worker */
  detail: string;
  ms: number;
  kind: 'build' | 'lint' | 'test' | 'types' | 'diff' | 'secret' | 'tests-integrity' | 'patch';
}

const TEST_FILE = /(^|\/)(tests?|__tests__|spec)(\/|$)|(\.|_)(test|spec)\.[a-z]+$|Tests?\.cs$|(^|\/)test_[^/]+\.py$|_test\.(go|py)$/i;
export const isTestFile = (p: string) => TEST_FILE.test(p);
const SKIP_ADDED = /(\b(it|test|describe)\.skip\b|\bxit\(|\bxdescribe\(|\bxtest\(|@pytest\.mark\.skip|@unittest\.skip|\[Ignore\]|\[Fact\(Skip|t\.Skip\(|\.only\()/;

export function parseDiffAdded(diff: string): { file: string; line: number; text: string }[] {
  const out: { file: string; line: number; text: string }[] = [];
  let file = '', ln = 0;
  for (const l of diff.split('\n')) {
    if (l.startsWith('+++ ')) { file = l.slice(4).replace(/^b\//, ''); continue; }
    const h = l.match(/^@@ -\d+(?:,\d+)? \+(\d+)/);
    if (h) { ln = Number(h[1]); continue; }
    if (l.startsWith('+') && !l.startsWith('+++')) { out.push({ file, line: ln, text: l.slice(1) }); ln++; }
    else if (!l.startsWith('-') && !l.startsWith('\\') && file) ln++;
  }
  return out;
}

export async function runCommands(cwd: string, cmds: VerifyCmd[], signal?: AbortSignal, timeoutMs = 600_000): Promise<CheckResult[]> {
  // build/types/lint in parallel; tests after build only if build did not fail (they usually depend on it)
  const exec = async (c: VerifyCmd): Promise<CheckResult> => {
    const r = await run(c.cmd, { cwd, timeoutMs, signal });
    const ok = r.code === 0 && !r.timedOut;
    return { name: c.name, kind: c.kind, status: ok ? 'pass' : 'fail', ms: r.ms, detail: ok ? 'ok' : clip(`exit ${r.code}${r.timedOut ? ' (timeout)' : ''}\n${r.stdout}\n${r.stderr}`, 6000) };
  };
  const pre = cmds.filter((c) => c.kind !== 'test');
  const tests = cmds.filter((c) => c.kind === 'test');
  const first = await Promise.all(pre.map(exec));
  const buildFailed = first.some((r) => r.status === 'fail' && (r.kind === 'build' || r.kind === 'types'));
  const second = await Promise.all(tests.map((t) => (buildFailed ? Promise.resolve<CheckResult>({ name: t.name, kind: 'test', status: 'skipped', detail: 'skipped: build failed', ms: 0 }) : exec(t))));
  return [...first, ...second];
}

export interface DiffChecks { results: CheckResult[]; diff: string; changed: string[] }

export async function diffChecks(cwd: string, cp: Checkpoints, base: string, opts: { allowedGlobs?: string[] } = {}): Promise<DiffChecks> {
  const t0 = Date.now();
  const ch = await cp.changedSince(base);
  const diff = await cp.diffSince(base);
  const changed = [...ch.added, ...ch.modified, ...ch.deleted];
  const results: CheckResult[] = [];
  const mk = (name: string, kind: CheckResult['kind'], bad: string[], okMsg: string): CheckResult => ({ name, kind, status: bad.length ? 'fail' : 'pass', detail: bad.length ? bad.join('\n') : okMsg, ms: Date.now() - t0 });

  // scope: only intended files
  if (opts.allowedGlobs?.length) {
    const outside = changed.filter((f) => !matchesAny(f, opts.allowedGlobs!));
    results.push(mk('diff scope', 'diff', outside.map((f) => `outside planned scope: ${f}`), `${changed.length} file(s) within scope`));
  } else results.push({ name: 'diff scope', kind: 'diff', status: 'skipped', detail: 'no planned scope', ms: 0 });

  // secrets in added lines
  const added = parseDiffAdded(diff);
  const secretHits = added.flatMap((a) => findSecrets(a.text).map((s) => `${a.file}:${a.line}: possible ${s}`));
  results.push(mk('secret scan', 'secret', secretHits, 'no secrets in added lines'));

  // test integrity
  const bad: string[] = [];
  for (const f of ch.deleted) if (isTestFile(f)) bad.push(`deleted test file: ${f}`);
  for (const a of added) if (isTestFile(a.file) && SKIP_ADDED.test(a.text)) bad.push(`${a.file}:${a.line}: skipped/focused test added`);
  results.push(mk('test integrity', 'tests-integrity', bad, 'no tests deleted or skipped'));

  // patch sanity (whitespace errors / conflict markers)
  const conflict = added.filter((a) => /^(<<<<<<<|>>>>>>>) /.test(a.text) || a.text === '=======').map((a) => `${a.file}:${a.line}: conflict marker`);
  results.push(mk('patch sanity', 'patch', conflict, 'no conflict markers'));
  return { results, diff, changed };
}

export function fileLines(cwd: string, file: string): number | undefined {
  const p = join(cwd, file);
  return existsSync(p) ? readFileSync(p, 'utf8').split('\n').length : undefined;
}
