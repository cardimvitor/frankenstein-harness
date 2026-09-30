import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { listAll } from './tools/fs.ts';

export interface StackTag { id: string; version?: string }
export interface Fingerprint {
  stacks: StackTag[];
  /** stable key of stack ids+major versions, used for stack-scope matching */
  stackKey: string;
  /** sorted tokens used for similarity */
  tokens: string[];
  projectId: string;
  verify: VerifyCmd[];
  summary: string;
}
export interface VerifyCmd { name: string; cmd: string; kind: 'build' | 'lint' | 'test' | 'types' }

const major = (v?: string) => v?.replace(/^[^\d]*/, '').split('.')[0];
const readJson = (p: string): any => { try { return JSON.parse(readFileSync(p, 'utf8')); } catch { return undefined; } };
const read = (p: string) => { try { return readFileSync(p, 'utf8'); } catch { return ''; } };

export function detectDotnet(files: string[], cwd: string): StackTag[] {
  const tags: StackTag[] = [];
  const proj = files.filter((f) => /\.(cs|fs|vb)proj$/.test(f));
  if (!proj.length) return tags;
  const fw = new Set<string>();
  for (const p of proj.slice(0, 50)) {
    const x = read(join(cwd, p));
    for (const m of x.matchAll(/<TargetFrameworks?>([^<]+)</g)) m[1].split(';').forEach((t) => fw.add(t.trim()));
    const old = x.match(/<TargetFrameworkVersion>v?([\d.]+)</);
    if (old) fw.add(`net${old[1].replace('.', '')}`);
  }
  for (const t of fw) {
    let m: RegExpMatchArray | null;
    if ((m = t.match(/^net(\d+)\.\d+/))) tags.push({ id: 'dotnet', version: m[1] }); // net8.0 -> 8
    else if ((m = t.match(/^net(4\d+)$/))) tags.push({ id: 'dotnet-framework', version: m[1] }); // net48
    else if ((m = t.match(/^netcoreapp(\d)/))) tags.push({ id: 'dotnet', version: m[1] });
    else if (/^netstandard/.test(t)) tags.push({ id: 'dotnet-standard', version: t.replace('netstandard', '') });
  }
  if (!tags.length) tags.push({ id: 'dotnet' });
  return tags;
}

export function detectNode(pkg: any, files: string[]): StackTag[] {
  if (!pkg) return [];
  const deps = { ...pkg.dependencies, ...pkg.devDependencies };
  const tags: StackTag[] = [{ id: 'node' }];
  if (deps.react) tags.push({ id: 'react', version: major(deps.react) });
  if (deps['@angular/core']) tags.push({ id: 'angular', version: major(deps['@angular/core']) });
  if (deps.angular && /^[^\d]*1\./.test(String(deps.angular))) tags.push({ id: 'angularjs', version: '1' });
  if (deps.vue) tags.push({ id: 'vue', version: major(deps.vue) });
  if (deps.typescript || files.some((f) => f === 'tsconfig.json')) tags.push({ id: 'typescript' });
  return tags;
}

export function detectVerify(cwd: string, files: string[], pkg: any): VerifyCmd[] {
  const out: VerifyCmd[] = [];
  const cfg = readJson(join(cwd, '.fh', 'verify.json'));
  if (Array.isArray(cfg)) return cfg as VerifyCmd[];
  if (pkg?.scripts) {
    const pm = existsSync(join(cwd, 'pnpm-lock.yaml')) ? 'pnpm' : existsSync(join(cwd, 'yarn.lock')) ? 'yarn' : 'npm';
    const run = (s: string) => (pm === 'npm' ? `npm run ${s}` : `${pm} ${s}`);
    for (const s of ['typecheck', 'tsc', 'type-check']) if (pkg.scripts[s]) { out.push({ name: s, cmd: run(s), kind: 'types' }); break; }
    if (pkg.scripts.build) out.push({ name: 'build', cmd: run('build'), kind: 'build' });
    if (pkg.scripts.lint) out.push({ name: 'lint', cmd: run('lint'), kind: 'lint' });
    if (pkg.scripts.test && !/no test specified/.test(pkg.scripts.test)) out.push({ name: 'test', cmd: pm === 'npm' ? 'npm test --silent' : `${pm} test`, kind: 'test' });
  }
  const sln = files.find((f) => /\.sln$/.test(f)) ?? files.find((f) => /\.csproj$/.test(f));
  if (sln && !files.some((f) => /packages\.config$/.test(f) && false)) {
    out.push({ name: 'dotnet build', cmd: `dotnet build ${JSON.stringify(sln)} --nologo -v q`, kind: 'build' });
    if (files.some((f) => /Tests?\.csproj$|\.Tests?\//.test(f))) out.push({ name: 'dotnet test', cmd: `dotnet test ${JSON.stringify(sln)} --nologo -v q`, kind: 'test' });
  }
  if (files.some((f) => f === 'pyproject.toml' || f === 'pytest.ini' || f === 'setup.py') && files.some((f) => /(^|\/)(test_.*|.*_test)\.py$/.test(f))) out.push({ name: 'pytest', cmd: 'python3 -m pytest -q', kind: 'test' });
  if (files.includes('go.mod')) { out.push({ name: 'go build', cmd: 'go build ./...', kind: 'build' }); out.push({ name: 'go test', cmd: 'go test ./...', kind: 'test' }); }
  if (files.includes('Cargo.toml')) { out.push({ name: 'cargo check', cmd: 'cargo check --quiet', kind: 'build' }); out.push({ name: 'cargo test', cmd: 'cargo test --quiet', kind: 'test' }); }
  return out;
}

export function fingerprint(cwd: string): Fingerprint {
  let files: string[] = [];
  try { files = listAll(cwd, 4000); } catch { /* empty */ }
  const pkg = readJson(join(cwd, 'package.json'));
  const stacks: StackTag[] = [...detectDotnet(files, cwd), ...detectNode(pkg, files)];
  if (files.some((f) => f.endsWith('.py'))) stacks.push({ id: 'python' });
  if (files.includes('go.mod')) stacks.push({ id: 'go' });
  if (files.includes('Cargo.toml')) stacks.push({ id: 'rust' });
  const seen = new Set<string>();
  const uniq = stacks.filter((s) => { const k = `${s.id}@${s.version ?? ''}`; if (seen.has(k)) return false; seen.add(k); return true; });
  const tokens = uniq.flatMap((s) => (s.version ? [s.id, `${s.id}@${s.version}`] : [s.id])).sort();
  const stackKey = tokens.filter((t) => t.includes('@') || !uniq.some((s) => s.id === t && s.version)).join('+') || 'unknown';
  const gitRemote = (() => { try { return read(join(cwd, '.git', 'config')).match(/url = (.+)/)?.[1] ?? ''; } catch { return ''; } })();
  const projectId = createHash('sha1').update(gitRemote || cwd).digest('hex').slice(0, 16);
  const layout = readdirSync(cwd, { withFileTypes: true }).filter((e) => e.isDirectory() && !e.name.startsWith('.') && e.name !== 'node_modules').map((e) => e.name).sort().slice(0, 12);
  return {
    stacks: uniq, stackKey, tokens, projectId, verify: detectVerify(cwd, files, pkg),
    summary: `${uniq.map((s) => (s.version ? `${s.id} ${s.version}` : s.id)).join(', ') || 'unknown stack'}${layout.length ? `; dirs: ${layout.join(', ')}` : ''}`,
  };
}

/** Jaccard similarity of stack tokens. */
export function similarity(a: string[], b: string[]): number {
  const A = new Set(a), B = new Set(b);
  if (!A.size || !B.size) return 0;
  let i = 0; for (const x of A) if (B.has(x)) i++;
  return i / (A.size + B.size - i);
}
