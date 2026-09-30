import type { Message } from '../types.ts';

export const estTokens = (m: Message[]): number =>
  Math.ceil(m.reduce((n, x) => n + (x.content?.length ?? 0) + (x.tool_calls ? JSON.stringify(x.tool_calls).length : 0) + 8, 0) / 3.4);

/**
 * Deterministic compaction (no extra LLM call): prune old tool outputs first,
 * then drop the oldest turns after the first user message. Keeps tool_call/result pairs intact.
 */
export function compact(messages: Message[], budgetTokens: number, keepRecentTools = 6): { messages: Message[]; pruned: number } {
  let pruned = 0;
  let out = messages.map((m) => ({ ...m }));
  if (estTokens(out) <= budgetTokens) return { messages: out, pruned };
  const toolIdx = out.map((m, i) => (m.role === 'tool' ? i : -1)).filter((i) => i >= 0);
  for (const i of toolIdx.slice(0, Math.max(0, toolIdx.length - keepRecentTools))) {
    const len = out[i].content?.length ?? 0;
    if (len > 200) { out[i].content = `[output pruned: ${len} chars]`; pruned++; }
  }
  // drop oldest complete assistant(+tool results) groups, keep system + first user + recent tail
  while (estTokens(out) > budgetTokens && out.length > 6) {
    const start = out.findIndex((m, i) => i >= 2 && m.role === 'assistant');
    if (start < 0) break;
    let end = start + 1;
    while (end < out.length && out[end].role === 'tool') end++;
    if (end >= out.length - 2) break;
    out.splice(start, end - start);
    pruned++;
  }
  if (pruned) {
    const at = out.findIndex((m, i) => i >= 1 && m.role !== 'system');
    out.splice(at + 1, 0, { role: 'user', content: '[Earlier steps were compacted to fit context. Re-read files if you need exact content.]' });
  }
  return { messages: out, pruned };
}
