/** Repair helpers for malformed tool-call JSON emitted by Qwen models. */

export interface ParseResult {
  ok: boolean;
  value?: Record<string, unknown>;
  repaired: boolean;
  thinkLeak: boolean;
}

const THINK_RE = /<think>[\s\S]*?(<\/think>|$)/gi;

export function stripThink(s: string): { text: string; leaked: boolean } {
  const leaked = /<\/?think>/i.test(s);
  return { text: s.replace(THINK_RE, '').replace(/<\/?think>/gi, ''), leaked };
}

function tryParse(s: string): Record<string, unknown> | undefined {
  try {
    const v = JSON.parse(s);
    return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : undefined;
  } catch {
    return undefined;
  }
}

/** Close unterminated strings/brackets, drop trailing commas. */
export function closeJson(s: string): string {
  const stack: string[] = [];
  let inStr = false;
  let esc = false;
  let out = '';
  for (const ch of s) {
    out += ch;
    if (inStr) {
      if (esc) esc = false;
      else if (ch === '\\') esc = true;
      else if (ch === '"') inStr = false;
      continue;
    }
    if (ch === '"') inStr = true;
    else if (ch === '{') stack.push('}');
    else if (ch === '[') stack.push(']');
    else if (ch === '}' || ch === ']') stack.pop();
  }
  if (inStr) {
    if (esc) out = out.slice(0, -1);
    out += '"';
  }
  out = out.replace(/,\s*$/, '');
  while (stack.length) {
    out = out.replace(/,\s*$/, '');
    out += stack.pop();
  }
  return out;
}

export function parseArgs(raw: string): ParseResult {
  const { text, leaked } = stripThink(raw ?? '');
  let s = text.trim();
  if (s === '') return { ok: true, value: {}, repaired: false, thinkLeak: leaked };
  let v = tryParse(s);
  if (v) return { ok: true, value: v, repaired: leaked, thinkLeak: leaked };
  // code fences
  const fence = s.match(/```(?:json)?\s*([\s\S]*?)```/);
  if (fence) s = fence[1].trim();
  // take from first { to last }
  const first = s.indexOf('{');
  if (first > 0) s = s.slice(first);
  const last = s.lastIndexOf('}');
  if (last > 0 && last < s.length - 1) s = s.slice(0, last + 1);
  v = tryParse(s);
  if (!v) v = tryParse(s.replace(/,\s*([}\]])/g, '$1'));
  if (!v) v = tryParse(closeJson(s));
  if (!v) v = tryParse(closeJson(s.replace(/,\s*([}\]])/g, '$1')));
  // single quotes -> double quotes as a last resort (only when no double quotes present)
  if (!v && !s.includes('"') && s.includes("'")) v = tryParse(closeJson(s.replace(/'/g, '"')));
  return v ? { ok: true, value: v, repaired: true, thinkLeak: leaked } : { ok: false, repaired: false, thinkLeak: leaked };
}

export interface ContentCall { name: string; args: Record<string, unknown>; raw: string }

/**
 * Recover tool calls the server failed to parse and left in content:
 *  <tool_call>{"name":..,"arguments":{..}}</tool_call>
 *  <tool_call><function=name><parameter=k>v</parameter></function></tool_call>  (qwen3_coder XML)
 */
export function extractContentCalls(content: string): { calls: ContentCall[]; rest: string } {
  const calls: ContentCall[] = [];
  let rest = content;
  const re = /<tool_call>([\s\S]*?)(?:<\/tool_call>|$)/g;
  for (const m of content.matchAll(re)) {
    const body = m[1].trim();
    let call: ContentCall | undefined;
    const fn = body.match(/<function=([^>\s]+)>([\s\S]*?)(?:<\/function>|$)/);
    if (fn) {
      const args: Record<string, unknown> = {};
      for (const p of fn[2].matchAll(/<parameter=([^>\s]+)>([\s\S]*?)(?:<\/parameter>|(?=<parameter=)|$)/g)) {
        args[p[1]] = p[2].replace(/^\n/, '').replace(/\n$/, '');
      }
      call = { name: fn[1], args, raw: m[0] };
    } else {
      const r = parseArgs(body);
      if (r.ok && r.value && typeof r.value.name === 'string') {
        const a = r.value.arguments ?? r.value.parameters ?? {};
        call = { name: r.value.name, args: typeof a === 'string' ? (parseArgs(a).value ?? {}) : (a as Record<string, unknown>), raw: m[0] };
      }
    }
    if (call) {
      calls.push(call);
      rest = rest.replace(m[0], '');
    }
  }
  return { calls, rest: rest.trim() };
}
