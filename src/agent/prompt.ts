import type { ToolSpec } from '../types.ts';

/**
 * Stable prefix: identical bytes on every request of a session (and across sessions)
 * so vLLM prefix caching hits. Never put timestamps, cwd-specific text or skills here.
 */
export const SYSTEM_PROMPT = `You are Frankenstein, a senior software engineer working inside the user's repository through tools.

Rules:
- Read before you edit. Make the smallest correct change. Use edit (search/replace); never rewrite whole files.
- Call independent read-only tools together in one turn.
- Run the project's build/tests after changes when they exist. Fix failures you caused.
- Do not invent APIs, files or versions; check the code. If unsure, look, do not guess.
- Never touch secrets, .git, or files outside the task's scope. Do not delete or skip tests to make them pass.
- Be brief. State what you changed and what you verified. If you could not verify, say so.`;

export function toolsDigest(tools: ToolSpec[]): string {
  return tools.map((t) => t.name).join(', ');
}

/** Task-specific context goes AFTER the stable prefix. */
export function contextBlock(parts: { cwd: string; fingerprint?: string; skills?: string; userRules?: string; plan?: string }): string {
  const out: string[] = [];
  out.push(`Workspace: ${parts.cwd}`);
  if (parts.fingerprint) out.push(`Detected stack: ${parts.fingerprint}`);
  if (parts.skills) out.push(`Relevant expertise (guidance, lower priority than the user's rules):\n${parts.skills}`);
  if (parts.userRules) out.push(`User rules (highest priority, override any guidance above):\n${parts.userRules}`);
  if (parts.plan) out.push(`Approved plan:\n${parts.plan}`);
  return out.join('\n\n');
}
