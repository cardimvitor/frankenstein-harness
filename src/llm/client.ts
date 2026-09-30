import type { ChatOptions, LlmResult, ToolCall, Usage } from '../types.ts';
import { authHeaders, redact, type Config } from '../config.ts';
import { extractContentCalls, parseArgs, stripThink } from './repair.ts';

export class LlmError extends Error {
  status?: number;
  retryable: boolean;
  constructor(msg: string, retryable: boolean, status?: number) {
    super(msg);
    this.retryable = retryable;
    this.status = status;
  }
}

export interface LlmStats {
  requests: number;
  promptTokens: number;
  completionTokens: number;
  cachedTokens: number;
  toolCalls: number;
  repaired: number;
  malformed: number;
  thinkLeaks: number;
  retries: number;
  totalMs: number;
}

export const emptyStats = (): LlmStats => ({
  requests: 0, promptTokens: 0, completionTokens: 0, cachedTokens: 0,
  toolCalls: 0, repaired: 0, malformed: 0, thinkLeaks: 0, retries: 0, totalMs: 0,
});

const sleep = (ms: number, signal?: AbortSignal) =>
  new Promise<void>((res, rej) => {
    const t = setTimeout(res, ms);
    signal?.addEventListener('abort', () => { clearTimeout(t); rej(new DOMException('aborted', 'AbortError')); }, { once: true });
  });

/** OpenAI-compatible streaming client tuned for vLLM + Qwen (qwen3 reasoning parser, qwen3_coder tool parser). */
export class LlmClient {
  cfg: Config;
  stats: LlmStats = emptyStats();
  constructor(cfg: Config) {
    this.cfg = cfg;
  }

  buildBody(o: ChatOptions): Record<string, unknown> {
    const thinking = o.thinking ?? 'off';
    const s = this.cfg.sampling[thinking === 'off' ? 'instant' : 'thinking'];
    const body: Record<string, unknown> = {
      model: this.cfg.model,
      messages: o.messages,
      stream: true,
      stream_options: { include_usage: true },
      max_tokens: o.maxTokens ?? this.cfg.maxOutputTokens,
      temperature: o.temperature ?? s.temperature,
      top_p: s.top_p,
      top_k: s.top_k,
      chat_template_kwargs: { enable_thinking: thinking !== 'off' },
    };
    if (s.presence_penalty !== undefined) body.presence_penalty = s.presence_penalty;
    if (o.stop) body.stop = o.stop;
    if (o.tools?.length) {
      body.tools = o.tools.map((t) => ({ type: 'function', function: { name: t.name, description: t.description, parameters: t.parameters } }));
      body.tool_choice = 'auto';
    }
    if (o.jsonSchema) body.response_format = { type: 'json_schema', json_schema: { name: 'out', schema: o.jsonSchema, strict: true } };
    return body;
  }

  async chat(o: ChatOptions): Promise<LlmResult> {
    let attempt = 0;
    for (;;) {
      try {
        const r = await this.once(o);
        return r;
      } catch (e) {
        const err = e as Error;
        if (err.name === 'AbortError' || o.signal?.aborted) throw err;
        const retryable = err instanceof LlmError ? err.retryable : true;
        if (!retryable || attempt >= this.cfg.retries) throw err;
        attempt++;
        this.stats.retries++;
        await sleep(Math.min(8000, 500 * 2 ** attempt) + Math.random() * 200, o.signal);
      }
    }
  }

  private async once(o: ChatOptions): Promise<LlmResult> {
    const t0 = Date.now();
    const ctl = new AbortController();
    const onAbort = () => ctl.abort();
    o.signal?.addEventListener('abort', onAbort, { once: true });
    const overall = setTimeout(() => ctl.abort(new Error('request timeout')), this.cfg.requestTimeoutMs);
    let idle: NodeJS.Timeout | undefined;
    const bump = () => {
      clearTimeout(idle);
      idle = setTimeout(() => ctl.abort(new Error('stream idle timeout')), this.cfg.idleTimeoutMs);
    };
    try {
      let res: Response;
      try {
        bump();
        res = await fetch(this.cfg.endpoint + '/chat/completions', {
          method: 'POST',
          headers: { 'content-type': 'application/json', accept: 'text/event-stream', ...authHeaders(this.cfg) },
          body: JSON.stringify(this.buildBody(o)),
          signal: ctl.signal,
        });
      } catch (e) {
        if (o.signal?.aborted) throw e;
        throw new LlmError(`network error: ${redact((e as Error).message)}`, true);
      }
      if (!res.ok) {
        const txt = redact(await res.text().catch(() => ''));
        throw new LlmError(`HTTP ${res.status}: ${txt.slice(0, 500)}`, res.status === 429 || res.status >= 500, res.status);
      }
      return await this.consume(res, o, t0, bump);
    } catch (e) {
      if (ctl.signal.aborted && !o.signal?.aborted) {
        throw new LlmError(String((ctl.signal.reason as Error)?.message ?? 'aborted'), true);
      }
      throw e;
    } finally {
      clearTimeout(overall);
      clearTimeout(idle);
      o.signal?.removeEventListener('abort', onAbort);
    }
  }

  private async consume(res: Response, o: ChatOptions, t0: number, bump: () => void): Promise<LlmResult> {
    let content = '';
    let reasoning = '';
    let finish = '';
    let ttft = -1;
    let usage: Usage = { prompt_tokens: 0, completion_tokens: 0 };
    const acc = new Map<number, { id: string; name: string; args: string }>();
    const dec = new TextDecoder();
    let buf = '';
    const reader = res.body!.getReader();
    const handle = (line: string) => {
      if (!line.startsWith('data:')) return;
      const data = line.slice(5).trim();
      if (!data || data === '[DONE]') return;
      let j: any;
      try { j = JSON.parse(data); } catch { return; }
      if (j.error) throw new LlmError(String(j.error.message ?? JSON.stringify(j.error)), false);
      if (j.usage) {
        usage = {
          prompt_tokens: j.usage.prompt_tokens ?? 0,
          completion_tokens: j.usage.completion_tokens ?? 0,
          cached_tokens: j.usage.prompt_tokens_details?.cached_tokens,
        };
      }
      const ch = j.choices?.[0];
      if (!ch) return;
      const d = ch.delta ?? {};
      const rs = d.reasoning_content ?? d.reasoning;
      if (typeof rs === 'string' && rs) {
        if (ttft < 0) ttft = Date.now() - t0;
        reasoning += rs;
        o.onReasoning?.(rs);
      }
      if (typeof d.content === 'string' && d.content) {
        if (ttft < 0) ttft = Date.now() - t0;
        content += d.content;
        o.onContent?.(d.content);
      }
      for (const tc of d.tool_calls ?? []) {
        if (ttft < 0) ttft = Date.now() - t0;
        const idx = tc.index ?? 0;
        const cur = acc.get(idx) ?? { id: '', name: '', args: '' };
        if (tc.id) cur.id = tc.id;
        if (tc.function?.name) cur.name += tc.function.name;
        if (tc.function?.arguments) cur.args += tc.function.arguments;
        acc.set(idx, cur);
      }
      if (ch.finish_reason) finish = ch.finish_reason;
    };
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      bump();
      buf += dec.decode(value, { stream: true });
      let nl: number;
      while ((nl = buf.indexOf('\n')) >= 0) {
        handle(buf.slice(0, nl).replace(/\r$/, ''));
        buf = buf.slice(nl + 1);
      }
    }
    if (buf.trim()) handle(buf.trim());

    const toolCalls: ToolCall[] = [];
    let repaired = 0, malformed = 0, thinkLeak = false;
    let n = 0;
    for (const [, c] of [...acc.entries()].sort((a, b) => a[0] - b[0])) {
      if (!c.name) continue;
      const p = parseArgs(c.args);
      if (p.thinkLeak) thinkLeak = true;
      if (p.repaired) repaired++;
      if (!p.ok) malformed++;
      toolCalls.push({
        id: c.id || `call_${Date.now().toString(36)}_${n++}`,
        name: stripThink(c.name).text.trim(),
        rawArgs: c.args,
        args: p.value ?? {},
        parse: p.ok ? (p.repaired ? 'repaired' : 'ok') : 'malformed',
      });
    }
    // Recover tool calls left in content by a failed server-side parser.
    if (toolCalls.length === 0 && /<tool_call>/.test(content)) {
      const { calls, rest } = extractContentCalls(content);
      if (calls.length) {
        content = rest;
        repaired += calls.length;
        for (const c of calls) {
          toolCalls.push({ id: `call_c${Date.now().toString(36)}_${n++}`, name: c.name, rawArgs: JSON.stringify(c.args), args: c.args, parse: 'repaired', fromContent: true });
        }
      }
    }
    // Reasoning that leaked into visible content (no reasoning parser configured).
    const sc = stripThink(content);
    if (sc.leaked) { thinkLeak = true; reasoning += content.match(/<think>([\s\S]*?)(<\/think>|$)/i)?.[1] ?? ''; content = sc.text.trim(); }

    const totalMs = Date.now() - t0;
    this.stats.requests++;
    this.stats.promptTokens += usage.prompt_tokens;
    this.stats.completionTokens += usage.completion_tokens;
    this.stats.cachedTokens += usage.cached_tokens ?? 0;
    this.stats.toolCalls += toolCalls.length;
    this.stats.repaired += repaired;
    this.stats.malformed += malformed;
    if (thinkLeak) this.stats.thinkLeaks++;
    this.stats.totalMs += totalMs;
    return { content, reasoning, toolCalls, finish, usage, ttftMs: ttft < 0 ? totalMs : ttft, totalMs, repaired, malformed, thinkLeak };
  }

  /**
   * Structured (JSON) call, parsed with repair. Some vLLM versions apply guided decoding from the first token, which
   * fights the reasoning block; if the constrained call yields no usable JSON, retry once without the schema and with
   * thinking off, asking for JSON in the prompt.
   */
  async json<T = Record<string, unknown>>(o: ChatOptions): Promise<{ value: T | undefined; raw: LlmResult; fallback: boolean }> {
    const parse = (t: string) => (t.trim() ? parseArgs(t) : { ok: false as const, value: undefined, repaired: false, thinkLeak: false });
    let raw = await this.chat(o);
    let p = parse(raw.content);
    if (!p.ok && (o.jsonSchema || (o.thinking && o.thinking !== 'off'))) {
      const hint = o.jsonSchema ? `\n\nReply with ONLY a JSON object matching this JSON schema, no prose and no code fences:\n${JSON.stringify(o.jsonSchema)}` : '\n\nReply with ONLY a JSON object, no prose.';
      const msgs = o.messages.map((m, i) => (i === o.messages.length - 1 && m.role === 'user' ? { ...m, content: `${m.content ?? ''}${hint}` } : m));
      raw = await this.chat({ ...o, messages: msgs, jsonSchema: undefined, thinking: 'off' });
      p = parse(raw.content);
      return { value: p.ok ? (p.value as T) : undefined, raw, fallback: true };
    }
    return { value: p.ok ? (p.value as T) : undefined, raw, fallback: false };
  }
}
