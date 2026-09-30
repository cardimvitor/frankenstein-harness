// Standalone mock vLLM used to dry-run scripts/vps-validate.sh without a model:
//   node test/run-mock.ts <port>
// It answers the validator's probes and solves the js-off-by-one eval task, so the whole pipeline can be exercised.
import { startMock } from './mock-vllm.ts';

const port = Number(process.argv[2] ?? 18000);
const m = await startMock({ port });
let reqs = 0, queries = 0, hits = 0;
const seen = new Set<string>();
m.metricsFn = () => `vllm:kv_cache_usage_perc 0.2\nvllm:num_requests_running 0\nvllm:num_requests_waiting 0\nvllm:prefix_cache_queries_total ${queries}\nvllm:prefix_cache_hits_total ${hits}\nvllm:spec_decode_num_drafts_total ${reqs * 10}\nvllm:spec_decode_num_draft_tokens_total ${reqs * 30}\nvllm:spec_decode_num_accepted_tokens_total ${reqs * 20}\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position="0"} ${reqs * 9}\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position="1"} ${reqs * 7}\nvllm:spec_decode_num_accepted_tokens_per_pos_total{position="2"} ${reqs * 4}\n`;
m.fallback = (req: any) => {
  reqs++;
  const text = req.messages.map((x: any) => String(x.content ?? '')).join('\n');
  const key = text.slice(0, 200); queries += 100; if (seen.has(key)) hits += 90; seen.add(key);
  const p = req.response_format?.json_schema?.schema?.properties;
  if (p?.trivial) return { content: JSON.stringify({ trivial: true, questions: [], enriched: 'Fix sumRange to be inclusive', acceptance: ['tests pass'], plan: [{ step: 'fix loop bound', files: ['lib/math.js'] }], assumptions: [], subtasks: [] }) };
  if (p?.verdict) return { content: JSON.stringify({ verdict: 'pass', findings: [] }) };
  if (p?.create !== undefined) return { content: '{"create":false}' };
  const needle = text.match(/passphrase is (KX-\d+)/)?.[1];
  if (needle) return { content: needle };
  const last = req.messages.at(-1);
  if (last.role === 'tool') return { content: 'Fixed the loop bound.' };
  if (/sumRange/.test(text) && last.role === 'user') return { tool_calls: [{ name: 'edit', args: { path: 'lib/math.js', old_text: 'i < b; i++', new_text: 'i <= b; i++' } }] };
  if (/Read the file/.test(text)) return { tool_calls: [{ name: 'read_file', args: { path: 'src/app.ts' } }] };
  if (/two-line snippet/.test(text)) return { tool_calls: [{ name: 'edit', args: { path: 'lib/util.js', old_text: 'const a = 1;\nconst b = "two";', new_text: 'const a = 10;\nconst b = "three"; // updated' } }] };
  if (/Run this exact shell/.test(text)) return { tool_calls: [{ name: 'bash', args: { command: 'grep -rn "TODO(\\"x\\")" src | head -5' } }] };
  if (/Create the new file/.test(text)) return { tool_calls: [{ name: 'write_file', args: { path: 'docs/notes.md', content: 'Olá, mundo' } }] };
  if (/Search the repo/.test(text)) return { tool_calls: [{ name: 'grep', args: { pattern: 'function\\s+\\w+\\(' } }] };
  if (/Think carefully/.test(text)) return { reasoning: 'hmm', tool_calls: [{ name: 'read_file', args: { path: 'src/parser.ts' } }] };
  return { content: '{"items":[]}' };
};
console.log(`mock vLLM on ${m.url}`);
