import { createServer, type IncomingMessage, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';

export interface Scripted {
  content?: string;
  reasoning?: string;
  tool_calls?: { name: string; args: string | object; splitArgs?: boolean }[];
  /** raw SSE data payloads to send instead of the built stream */
  rawChunks?: string[];
  status?: number;
  delayMs?: number;
}

export interface MockServer {
  url: string;
  requests: any[];
  queue: Scripted[];
  fallback?: (req: any) => Scripted;
  metrics: string;
  metricsFn?: () => string;
  close(): Promise<void>;
}

const readBody = (req: IncomingMessage) =>
  new Promise<string>((res) => { let b = ''; req.on('data', (c) => (b += c)); req.on('end', () => res(b)); });

export async function startMock(opts: { apiKey?: string } = {}): Promise<MockServer> {
  const m: MockServer = { url: '', requests: [], queue: [], metrics: '', close: async () => {} };
  const server: Server = createServer(async (req, res) => {
    if (opts.apiKey && req.headers.authorization !== `Bearer ${opts.apiKey}`) {
      res.writeHead(401).end(JSON.stringify({ error: { message: 'unauthorized' } }));
      return;
    }
    if (req.url === '/metrics') { res.writeHead(200).end(m.metricsFn ? m.metricsFn() : m.metrics); return; }
    if (req.url === '/v1/models') {
      if (req.headers.authorization === undefined && opts.apiKey === undefined && false) return; res.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ data: [{ id: 'mock-qwen', max_model_len: 32768 }] })); return; }
    if (req.url === '/v1/chat/completions') {
      const body = JSON.parse(await readBody(req));
      m.requests.push(body);
      const s = m.queue.shift() ?? m.fallback?.(body) ?? { content: 'ok' };
      if (s.delayMs) await new Promise((r) => setTimeout(r, s.delayMs));
      if (s.status) { res.writeHead(s.status).end(JSON.stringify({ error: { message: 'scripted' } })); return; }
      if (body.stream === false) {
        const tcs = (s.tool_calls ?? []).map((tc, i) => ({ id: `c${i}`, type: 'function', function: { name: tc.name, arguments: typeof tc.args === 'string' ? tc.args : JSON.stringify(tc.args) } }));
        res.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ choices: [{ message: { role: 'assistant', content: s.content ?? null, tool_calls: tcs.length ? tcs : undefined }, finish_reason: tcs.length ? 'tool_calls' : 'stop' }], usage: { prompt_tokens: 100, completion_tokens: 20 } }));
        return;
      }
      res.writeHead(200, { 'content-type': 'text/event-stream' });
      const send = (delta: object, finish: string | null = null) =>
        res.write(`data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`);
      if (s.rawChunks) for (const c of s.rawChunks) res.write(`data: ${c}\n\n`);
      else {
        if (s.reasoning) for (const p of s.reasoning.match(/.{1,8}/gs) ?? []) send({ reasoning_content: p });
        if (s.content) for (const p of s.content.match(/.{1,8}/gs) ?? []) send({ content: p });
        (s.tool_calls ?? []).forEach((tc, i) => {
          const a = typeof tc.args === 'string' ? tc.args : JSON.stringify(tc.args);
          send({ tool_calls: [{ index: i, id: `c${i}`, type: 'function', function: { name: tc.name, arguments: '' } }] });
          for (const p of a.match(/.{1,6}/gs) ?? ['']) send({ tool_calls: [{ index: i, function: { arguments: p } }] });
        });
        send({}, s.tool_calls?.length ? 'tool_calls' : 'stop');
        res.write(`data: ${JSON.stringify({ choices: [], usage: { prompt_tokens: 100, completion_tokens: 20 } })}\n\n`);
      }
      res.write('data: [DONE]\n\n');
      res.end();
      return;
    }
    res.writeHead(404).end();
  });
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  m.url = `http://127.0.0.1:${(server.address() as AddressInfo).port}/v1`;
  m.close = () => new Promise((r) => { server.closeAllConnections(); server.close(() => r()); });
  return m;
}
