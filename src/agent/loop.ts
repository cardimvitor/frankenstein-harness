import type { LlmResult, Message, Mode, ThinkingLevel, ToolCall } from '../types.ts';
import type { LlmClient } from '../llm/client.ts';
import { ALL_TOOLS, type Tool, type ToolCtx, type ToolResult } from '../tools/index.ts';
import { decide } from './permissions.ts';
import { SYSTEM_PROMPT } from './prompt.ts';
import { compact } from './context.ts';
import { redact } from '../config.ts';

export interface AgentEvents {
  text?(delta: string): void;
  reasoning?(delta: string): void;
  step?(n: number, level: ThinkingLevel): void;
  toolStart?(call: ToolCall): void;
  toolEnd?(call: ToolCall, r: ToolResult, ms: number): void;
  notice?(msg: string): void;
  llm?(r: LlmResult): void;
}

export interface AgentOptions {
  llm: LlmClient;
  cwd: string;
  mode: Mode;
  thinking?: ThinkingLevel | 'auto';
  maxSteps?: number;
  signal?: AbortSignal;
  tools?: Tool[];
  ownedGlobs?: string[];
  confirm?: (tool: Tool, args: Record<string, unknown>) => Promise<boolean>;
  events?: AgentEvents;
  /** Task-specific context inserted as the second system message (after the stable prefix). */
  context?: string;
  history?: Message[];
  contextWindow?: number;
  wrapShell?: ToolCtx['wrapShell'];
  readCache?: Map<string, string>;
  touched?: Set<string>;
}

export interface AgentResult {
  final: string;
  messages: Message[];
  steps: number;
  touched: string[];
  stopped: 'done' | 'max_steps' | 'loop' | 'aborted' | 'error';
  error?: string;
  toolCalls: number;
  failedTools: number;
}

export async function runAgent(task: string, o: AgentOptions): Promise<AgentResult> {
  const tools = o.tools ?? ALL_TOOLS;
  const byName = new Map(tools.map((t) => [t.spec.name, t]));
  const specs = tools.map((t) => t.spec);
  const ctx: ToolCtx = {
    cwd: o.cwd, signal: o.signal, ownedGlobs: o.ownedGlobs, readCache: o.readCache ?? new Map(), touched: o.touched ?? new Set(), wrapShell: o.wrapShell,
  };
  const messages: Message[] = o.history ? [...o.history] : [{ role: 'system', content: SYSTEM_PROMPT }, ...(o.context ? [{ role: 'system' as const, content: o.context }] : [])];
  messages.push({ role: 'user', content: task });
  const maxSteps = o.maxSteps ?? 40;
  const recent: string[] = [];
  let toolCalls = 0, failedTools = 0, consecutiveFail = 0, final = '', stopped: AgentResult['stopped'] = 'max_steps', error: string | undefined;
  const budget = Math.floor((o.contextWindow ?? 131072) * 0.72);

  let step = 0;
  for (; step < maxSteps; step++) {
    if (o.signal?.aborted) { stopped = 'aborted'; break; }
    const level: ThinkingLevel = o.thinking === 'auto' || o.thinking === undefined ? (step === 0 || consecutiveFail >= 2 ? 'high' : 'off') : o.thinking;
    o.events?.step?.(step, level);
    const c = compact(messages, budget);
    if (c.pruned) { messages.splice(0, messages.length, ...c.messages); o.events?.notice?.(`context compacted (${c.pruned} items)`); }
    let r: LlmResult;
    try {
      r = await o.llm.chat({ messages, tools: specs, thinking: level, signal: o.signal, onContent: o.events?.text, onReasoning: o.events?.reasoning });
    } catch (e) {
      if (o.signal?.aborted || (e as Error).name === 'AbortError') { stopped = 'aborted'; break; }
      stopped = 'error'; error = redact((e as Error).message); break;
    }
    o.events?.llm?.(r);
    messages.push({
      role: 'assistant', content: r.content || null,
      ...(r.toolCalls.length ? { tool_calls: r.toolCalls.map((t) => ({ id: t.id, type: 'function' as const, function: { name: t.name, arguments: t.parse === 'malformed' ? '{}' : JSON.stringify(t.args) } })) } : {}),
    });
    if (!r.toolCalls.length) {
      if (r.finish === 'length') { messages.push({ role: 'user', content: 'Your reply was cut off. Continue briefly.' }); continue; }
      final = r.content; stopped = 'done'; break;
    }

    const runOne = async (call: ToolCall): Promise<ToolResult> => {
      const tool = byName.get(call.name);
      if (!tool) return { ok: false, output: `unknown tool "${call.name}". Available: ${[...byName.keys()].join(', ')}` };
      if (call.parse === 'malformed') return { ok: false, output: 'tool arguments were not valid JSON; resend the call with valid JSON arguments' };
      const d = decide(o.mode, tool, call.args);
      if (!d.allow) {
        if (d.ask && o.confirm && (await o.confirm(tool, call.args))) { /* approved */ }
        else return { ok: false, output: d.ask ? 'denied by user' : d.reason };
      }
      const sig = call.name + JSON.stringify(call.args);
      recent.push(sig); if (recent.length > 12) recent.shift();
      const reps = recent.filter((s) => s === sig).length;
      let res: ToolResult;
      const t0 = Date.now();
      o.events?.toolStart?.(call);
      try { res = await tool.execute(call.args, ctx); } catch (e) { res = { ok: false, output: (e as Error).message }; }
      if (reps >= 3) res.output += '\n[You have repeated this exact call several times. Try a different approach.]';
      o.events?.toolEnd?.(call, res, Date.now() - t0);
      return { ok: res.ok, output: res.output };
    };

    const allRead = r.toolCalls.every((t) => byName.get(t.name)?.readOnly);
    const results = allRead ? await Promise.all(r.toolCalls.map(runOne)) : await r.toolCalls.reduce<Promise<ToolResult[]>>(async (p, c) => { const a = await p; a.push(await runOne(c)); return a; }, Promise.resolve([]));
    let loopStop = false;
    r.toolCalls.forEach((call, i) => {
      toolCalls++;
      if (!results[i].ok) { failedTools++; consecutiveFail++; } else consecutiveFail = 0;
      messages.push({ role: 'tool', tool_call_id: call.id, content: results[i].output });
    });
    for (const s of new Set(recent)) if (recent.filter((x) => x === s).length >= 5) loopStop = true;
    if (loopStop) { stopped = 'loop'; error = 'stopped: repeated identical tool calls'; break; }
  }
  if (step >= maxSteps && stopped === 'max_steps') error = `reached step limit (${maxSteps})`;
  return { final, messages, steps: step, touched: [...ctx.touched], stopped, error, toolCalls, failedTools };
}
