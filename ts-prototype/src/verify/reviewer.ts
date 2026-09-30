import { readFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import type { LlmClient } from '../llm/client.ts';
import { clip } from '../util/proc.ts';

export interface Finding { file: string; line: number; severity: 'blocker' | 'major' | 'minor'; claim: string; quote: string }

export const REVIEW_SCHEMA = {
  type: 'object',
  properties: {
    verdict: { type: 'string', enum: ['pass', 'fail'] },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          file: { type: 'string' }, line: { type: 'integer' }, severity: { type: 'string', enum: ['blocker', 'major', 'minor'] },
          claim: { type: 'string' }, quote: { type: 'string', description: 'verbatim text of the cited line' },
        },
        required: ['file', 'line', 'severity', 'claim', 'quote'],
      },
    },
  },
  required: ['verdict', 'findings'],
};

export const CHECKLISTS = [
  'Correctness: does the change do what the task and acceptance criteria require? Look for logic errors, wrong conditions, off-by-one, unhandled null/undefined.',
  'Edge cases and error handling: empty input, failures, concurrency, resource cleanup, missing tests for the new behavior.',
  'Security: injection, path traversal, unsafe deserialization, secrets, authz checks, unsafe shell/SQL construction.',
  'Regressions and scope: broken callers, changed public API, unrelated edits, inconsistent style with surrounding code.',
  'Final pass: completeness against every acceptance criterion, leftover debug code, performance traps (N+1, quadratic loops).',
];

const norm = (s: string) => s.replace(/\s+/g, ' ').trim();

/** Drop findings that cannot be checked: unknown file, out-of-range line, or quote not near that line. */
export function validateFindings(cwd: string, raw: unknown, changed: string[]): { valid: Finding[]; dropped: number } {
  const arr = Array.isArray(raw) ? raw : [];
  const valid: Finding[] = [];
  let dropped = 0;
  for (const f of arr as Partial<Finding>[]) {
    const ok = typeof f?.file === 'string' && Number.isInteger(f.line) && typeof f.quote === 'string' && f.quote.trim().length >= 3
      && typeof f.claim === 'string' && ['blocker', 'major', 'minor'].includes(String(f.severity));
    if (!ok) { dropped++; continue; }
    const file = f.file!.replace(/^\.\//, '');
    const p = join(cwd, file);
    if (!changed.includes(file) || !existsSync(p)) { dropped++; continue; }
    const lines = readFileSync(p, 'utf8').split('\n');
    const line = f.line!;
    if (line < 1 || line > lines.length) { dropped++; continue; }
    const window = norm(lines.slice(Math.max(0, line - 2), line + 1).join(' '));
    if (!window.includes(norm(f.quote!))) { dropped++; continue; }
    valid.push({ file, line, severity: f.severity as Finding['severity'], claim: f.claim!.slice(0, 400), quote: f.quote!.slice(0, 200) });
  }
  return { valid, dropped };
}

export async function review(llm: LlmClient, cwd: string, o: { diff: string; changed: string[]; acceptance: string[]; checklist: string; signal?: AbortSignal }) {
  const prompt = [
    'You are a strict code reviewer. Review ONLY the diff below against the task criteria and the checklist.',
    'Report only concrete defects you can point to. For each, give file, line (line number in the NEW file), severity, a one-sentence claim, and quote = the verbatim text of that line.',
    'If there are no defects, verdict "pass" with an empty findings list. Do not report style preferences as blockers.',
    `Checklist for this round: ${o.checklist}`,
    `Acceptance criteria:\n${o.acceptance.map((a) => `- ${a}`).join('\n') || '- (none given)'}`,
    `Diff:\n${clip(o.diff, 24000)}`,
  ].join('\n\n');
  const { value, raw } = await llm.json<{ verdict?: string; findings?: unknown }>({
    messages: [{ role: 'system', content: 'You output only JSON matching the schema.' }, { role: 'user', content: prompt }],
    thinking: 'high', maxTokens: 3000, jsonSchema: REVIEW_SCHEMA, signal: o.signal,
  });
  if (!value) return { valid: [] as Finding[], dropped: 0, malformed: true, raw };
  const v = validateFindings(cwd, value.findings, o.changed);
  return { ...v, malformed: false, raw };
}
