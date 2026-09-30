// Builds SUMMARY.md from the artifacts written by scripts/vps-validate.sh and grades them against the success criteria.
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

const dir = process.argv[2];
const failedSteps = (process.argv[3] ?? '').trim();
const read = (p: string) => (existsSync(p) ? readFileSync(p, 'utf8') : '');
const jsonIn = (sub: string, file: string) => {
  const d = readdirSync(dir).find((x) => x.startsWith(sub));
  return d && existsSync(join(dir, d, file)) ? JSON.parse(readFileSync(join(dir, d, file), 'utf8')) : undefined;
};
const rowsIn = (): any[] => {
  const d = readdirSync(dir).find((x) => x.startsWith('eval-'));
  const f = d && join(dir, d, 'results.jsonl');
  return f && existsSync(f) ? read(f).split('\n').filter(Boolean).map((l) => JSON.parse(l)) : [];
};

const vllm = jsonIn('vllm-', 'report.json');
const rows = rowsIn();
const unit = read(join(dir, 'unit.log'));
const web = read(join(dir, 'web-security-tests.txt'));
const smoke = read(join(dir, 'web-smoke.txt'));
const probe = (n: string) => vllm?.results?.find((r: any) => r.name.startsWith(n));

type Grade = 'PASS' | 'FAIL' | 'NO DATA';
const lines: string[] = [];
const grade = (name: string, g: Grade, detail: string) => lines.push(`| ${name} | ${g} | ${detail.replace(/\|/g, '/')} |`);

const tc = probe('tool-call reliability');
grade('Malformed tool calls < 1%', !tc ? 'NO DATA' : tc.data.malformedRate < 0.01 ? 'PASS' : 'FAIL', tc ? `malformed ${(tc.data.malformedRate * 100).toFixed(2)}%, repaired-or-malformed ${(tc.data.repairedRate * 100).toFixed(1)}%, ${tc.data.total} calls` : 'validate-vllm did not run');
const leak = probe('thinking must not leak');
grade('No thinking leaked into tool args', !leak ? 'NO DATA' : leak.status === 'pass' ? 'PASS' : 'FAIL', leak?.summary ?? '');

const fh = rows.filter((r) => r.runner === 'fh'), qw = rows.filter((r) => r.runner === 'qwen');
const rate = (rs: any[]) => (rs.length ? rs.filter((r) => r.solved).length / rs.length : 0);
const med = (a: number[]) => { const s = [...a].sort((x, y) => x - y); return s.length ? s[Math.floor(s.length / 2)] : 0; };
if (fh.length && qw.length) {
  grade('Pass rate >= plain Qwen Code (same model)', rate(fh) >= rate(qw) ? 'PASS' : 'FAIL', `fh ${(rate(fh) * 100).toFixed(0)}% vs qwen ${(rate(qw) * 100).toFixed(0)}% over ${fh.length} runs each (smoke corpus; use SWE-bench/Terminal-Bench subsets for the real margin)`);
  grade('Median task time no worse', med(fh.map((r) => r.seconds)) <= med(qw.map((r) => r.seconds)) * 1.0 ? 'PASS' : 'FAIL', `fh ${med(fh.map((r) => r.seconds)).toFixed(1)}s vs qwen ${med(qw.map((r) => r.seconds)).toFixed(1)}s (fh includes planning and verification rounds)`);
} else if (fh.length) {
  grade('Pass rate / time vs plain Qwen Code', 'NO DATA', `fh solved ${(rate(fh) * 100).toFixed(0)}% in median ${med(fh.map((r) => r.seconds)).toFixed(1)}s; baseline not run`);
} else grade('Pass rate / time vs plain Qwen Code', 'NO DATA', 'eval did not run');

if (fh.length) {
  const vfn = fh.filter((r) => r.verifierFalseNegative).length;
  const vfp = fh.filter((r) => r.verdict === 'fail' && r.solved).length;
  grade('Verifier false positives < 10%', (vfp / fh.length) < 0.1 ? 'PASS' : 'FAIL', `verdict=fail but oracle passes: ${vfp}/${fh.length}; verdict=pass but oracle fails: ${vfn}/${fh.length}`);
  grade('Skill gate adds no LLM call', fh.every((r) => r.gateMs !== undefined) && /gate: deterministic, fast, zero LLM calls[^\n]*/.test(unit) && !/not ok .*gate/.test(unit) ? 'PASS' : 'NO DATA', `mean gate ${(fh.reduce((n, r) => n + (r.gateMs ?? 0), 0) / fh.length).toFixed(2)} ms`);
} else {
  grade('Verifier false positives < 10%', 'NO DATA', 'eval did not run');
  grade('Skill gate adds no LLM call', /not ok .*gate/.test(unit) ? 'FAIL' : /ok .*gate/.test(unit) ? 'PASS' : 'NO DATA', 'unit test');
}

const sec = web && !/not ok/.test(web) && /forged Host -> 403/.test(smoke) && /API without cookie -> 401/.test(smoke) && /foreign Origin POST -> 403/.test(smoke);
grade('Web UI security checklist (Host/Origin, cookie, CSP, loopback)', !web && !smoke ? 'NO DATA' : sec ? 'PASS' : 'FAIL', 'unit tests + live HTTP smoke test');
grade('Harness self-tests', !unit ? 'NO DATA' : /^# fail 0/m.test(unit) ? 'PASS' : 'FAIL', (unit.match(/^# (tests|pass|fail) \d+/gm) ?? []).join(', '));

const out: string[] = [`# Frankenstein Harness validation summary`, '', `Run directory: \`${dir}\``, ''];
out.push('| Criterion | Result | Detail |', '|---|---|---|', ...lines, '');
if (failedSteps) out.push(`Steps that reported problems: ${failedSteps}`, '');
if (vllm) {
  out.push('## vLLM measurements', '', '| Probe | Result | Summary |', '|---|---|---|', ...vllm.results.map((r: any) => `| ${r.name} | ${r.status.toUpperCase()} | ${String(r.summary).replace(/\|/g, '/')} |`), '');
  const conc = probe('concurrency');
  if (conc?.data?.recommended) out.push(`Suggested \`maxConcurrency\` for \`.fh/config.json\`: **${conc.data.recommended}**`, '');
}
const evalDir = readdirSync(dir).find((x) => x.startsWith('eval-'));
if (evalDir && existsSync(join(dir, evalDir, 'summary.md'))) out.push('## Eval', '', read(join(dir, evalDir, 'summary.md')).replace(/^# .*\n/, ''), '');
out.push('## Not covered by this run', '', '- SWE-bench Verified / Terminal-Bench subsets and your recorded .NET/React/Angular tasks (load with `--tasks <dir>`, see docs/EVAL.md): the built-in corpus is a smoke test.', '- Windows and macOS execution (run the same script on those hosts; Windows sandboxing is not implemented).', '');
console.log(out.join('\n'));
