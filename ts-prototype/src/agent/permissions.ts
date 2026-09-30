import type { Mode } from '../types.ts';
import type { Tool } from '../tools/types.ts';

export type Decision = { allow: true } | { allow: false; reason: string; ask?: boolean };

/** Commands that are refused in every mode, including yolo. */
const HARD_DENY: [RegExp, string][] = [
  [/\brm\s+(-[a-zA-Z]*r[a-zA-Z]*f|-[a-zA-Z]*f[a-zA-Z]*r)\s+(\/|~|\$HOME|\*)(\s|$)/, 'recursive delete of root/home'],
  [/\bmkfs(\.\w+)?\b|\bdd\s+.*of=\/dev\//, 'disk-destroying command'],
  [/:\(\)\s*\{.*\};\s*:/, 'fork bomb'],
  [/\b(curl|wget)\b[^|;]*\|\s*(sudo\s+)?(ba|z)?sh\b/, 'pipe download to shell'],
  [/\bgit\s+push\b.*(--force|-f)\b/, 'force push'],
  [/\bchmod\s+-R\s+777\s+\//, 'chmod -R 777 /'],
  [/(^|[\s;&|])(shutdown|reboot|halt|poweroff)\b/, 'system power command'],
  [/\bformat\s+[a-z]:/i, 'format drive'],
];

/** Read-only shell commands that never need approval. */
const SAFE_SHELL = /^\s*(ls|cat|head|tail|wc|pwd|echo|git\s+(status|diff|log|show|branch|rev-parse)|rg|grep|find|which|node\s+-v|npm\s+(ls|view)|dotnet\s+--(info|version|list-sdks))\b[^|;&><`$]*$/;

export function hardDeny(command: string): string | undefined {
  for (const [re, why] of HARD_DENY) if (re.test(command)) return why;
  return undefined;
}

export function decide(mode: Mode, tool: Tool, args: Record<string, unknown>): Decision {
  if (tool.spec.name === 'bash') {
    const cmd = String(args.command ?? '');
    const deny = hardDeny(cmd);
    if (deny) return { allow: false, reason: `blocked: ${deny}` };
    if (mode === 'plan') return SAFE_SHELL.test(cmd) ? { allow: true } : { allow: false, reason: 'plan mode is read-only; only read-only shell commands are allowed' };
    if (mode === 'yolo') return { allow: true };
    if (SAFE_SHELL.test(cmd)) return { allow: true };
    return { allow: false, reason: 'shell command needs approval', ask: true };
  }
  if (tool.readOnly) return { allow: true };
  if (mode === 'plan') return { allow: false, reason: 'plan mode is read-only' };
  if (mode === 'ask') return { allow: false, reason: 'edit needs approval', ask: true };
  return { allow: true };
}
