import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { loadConfig, authHeaders, type Config } from '../config.ts';
import { Engine, headlessIO, type TaskResult } from '../engine.ts';
import { Checkpoints } from '../session/checkpoint.ts';
import { SkillStore } from '../skills/store.ts';
import { c, ask, terminalIO } from './term.ts';
import { startWebServer } from './server.ts';
import { validateVllm } from '../validate/vllm.ts';
import { runEval } from '../eval/runner.ts';
import { sandboxFor } from '../util/sandbox.ts';
import type { Mode } from '../types.ts';

const HELP = `Frankenstein Harness (fh) — coding agent for Qwen on vLLM

Usage:
  fh                          interactive chat in the current directory
  fh run "<task>" [options]   run one task
  fh serve [--port N]         local web UI (127.0.0.1 only)
  fh doctor                   check endpoint, model, auth, metrics, sandbox
  fh validate-vllm [options]  measure MTP, prefix cache, tool calls, long context, concurrency
  fh eval --tasks <dir> [options]   run the eval corpus (runner: fh | qwen)
  fh undo                     restore the working tree to the last checkpoint
  fh activity                 recent skill activity

Options for run/chat:
  --auto            autonomous: no plan approval or questions, up to 5 verification rounds
  --mode <m>        approval mode: plan | ask | auto-edit (default) | yolo
  --yes             never ask (approve plan and tool calls)
  --plan-only       show the plan and stop
  --commit          git commit if verification passes
  --keep            keep changes even when verification fails
  --sandbox         confine shell commands (bwrap on Linux, Seatbelt on macOS)
  --json            print the machine-readable result
  --cwd <dir>       workspace (default: current directory)
  --thinking        show model reasoning

Environment: FH_ENDPOINT, FH_MODEL, FH_API_KEY (or the variable named by FH_API_KEY_ENV), FH_AUTH_SCHEME, FH_METRICS_URL, FH_HOME`;

export function parseArgs(argv: string[]): { cmd: string; positional: string[]; flags: Record<string, string | boolean> } {
  const flags: Record<string, string | boolean> = {};
  const positional: string[] = [];
  const valued = new Set(['mode', 'cwd', 'port', 'tasks', 'runner', 'out', 'limit', 'repeat', 'max-context', 'concurrency', 'trials', 'qwen-cmd']);
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith('--')) {
      const k = a.slice(2);
      if (k.includes('=')) flags[k.slice(0, k.indexOf('='))] = k.slice(k.indexOf('=') + 1);
      else if (valued.has(k) && i + 1 < argv.length) flags[k] = argv[++i];
      else flags[k] = true;
    } else positional.push(a);
  }
  const cmd = positional.shift() ?? '';
  return { cmd, positional, flags };
}

function printResult(r: TaskResult, json: boolean) {
  if (json) { console.log(JSON.stringify(r, null, 2)); return; }
  const col = r.verdict === 'pass' ? c.green : r.verdict === 'unverified' ? c.yellow : c.red;
  console.log('\n' + col(r.final || r.reason));
  if (r.rejectedPatch) console.log(c.dim(`rejected patch saved to ${r.rejectedPatch}`));
  console.log(c.dim(`${(r.timings.totalMs / 1000).toFixed(1)}s · ${r.rounds} round(s) · ${r.llm.completionTokens} tokens out · ${r.llm.requests} requests${r.llm.repaired ? ` · ${r.llm.repaired} repaired calls` : ''}`));
}

async function doctor(cfg: Config) {
  const ok = (m: string) => console.log(`${c.green('✓')} ${m}`), bad = (m: string) => console.log(`${c.red('✗')} ${m}`);
  console.log(`endpoint ${cfg.endpoint}  model ${cfg.model}  auth ${cfg.authScheme}${cfg.authScheme !== 'none' ? ` (${cfg.apiKeyEnv} ${process.env[cfg.apiKeyEnv] ? 'set' : 'NOT set'})` : ''}`);
  try {
    const r = await fetch(`${cfg.endpoint}/models`, { headers: authHeaders(cfg), signal: AbortSignal.timeout(8000) });
    if (r.status === 401 || r.status === 403) bad(`/models rejected credentials (HTTP ${r.status})`);
    else if (!r.ok) bad(`/models HTTP ${r.status}`);
    else {
      const j: any = await r.json(); const ids = (j.data ?? []).map((m: any) => m.id);
      ok(`models: ${ids.join(', ') || '(none)'}`);
      if (!ids.includes(cfg.model)) bad(`configured model "${cfg.model}" is not served; set FH_MODEL to one of the above`);
      const ml = j.data?.find((m: any) => m.id === cfg.model)?.max_model_len; if (ml) ok(`max_model_len ${ml}`);
    }
  } catch (e) { bad(`cannot reach ${cfg.endpoint}: ${(e as Error).message}`); }
  try { const r = await fetch(cfg.metricsUrl, { headers: authHeaders(cfg), signal: AbortSignal.timeout(4000) }); r.ok ? ok(`metrics at ${cfg.metricsUrl}`) : bad(`metrics HTTP ${r.status} (governor and MTP/prefix-cache measurements need /metrics)`); } catch { bad(`metrics unreachable at ${cfg.metricsUrl}`); }
  const sb = sandboxFor(process.cwd()); (sb.backend === 'none' ? bad : ok)(`shell sandbox: ${sb.backend}`);
  ok(`node ${process.version}, ${process.platform}`);
}

export async function main(argv: string[]): Promise<number> {
  const { cmd, positional, flags } = parseArgs(argv);
  if (flags.help || cmd === 'help' || cmd === '--help') { console.log(HELP); return 0; }
  const cwd = resolve(String(flags.cwd ?? process.cwd()));
  if (!existsSync(cwd)) { console.error(`no such directory: ${cwd}`); return 2; }
  const cfg = loadConfig(cwd);
  const approval = (flags.mode as Mode) ?? 'auto-edit';
  if (!['plan', 'ask', 'auto-edit', 'yolo'].includes(approval)) { console.error(`invalid --mode ${approval}`); return 2; }

  if (cmd === 'doctor') { await doctor(cfg); return 0; }
  if (cmd === 'activity') { for (const a of new SkillStore().activity(30)) console.log(`${new Date(a.ts).toISOString()}  ${a.kind.padEnd(11)} ${a.skill} — ${a.reason}`); return 0; }
  if (cmd === 'undo') {
    const cp = new Checkpoints(cwd);
    const { run } = await import('../util/proc.ts');
    const last = (await run('git for-each-ref --sort=-refname --format=%(objectname) refs/fh/checkpoints --count=1', { cwd })).stdout.trim();
    if (!last) { console.error('no checkpoint found'); return 1; }
    const files = await cp.restore(last); console.log(`restored ${files.length} file(s) to checkpoint ${last.slice(0, 8)}`); return 0;
  }
  if (cmd === 'validate-vllm') return validateVllm(cfg, { out: String(flags.out ?? 'reports'), quick: !!flags.quick, maxContext: Number(flags['max-context'] ?? 0) || undefined, trials: Number(flags.trials ?? 0) || undefined, concurrency: Number(flags.concurrency ?? 0) || undefined });
  if (cmd === 'eval') return runEval(cfg, { tasks: String(flags.tasks ?? 'eval/tasks'), runner: String(flags.runner ?? 'fh') as 'fh' | 'qwen', out: String(flags.out ?? 'reports'), limit: Number(flags.limit ?? 0) || undefined, repeat: Number(flags.repeat ?? 1), qwenCmd: String(flags['qwen-cmd'] ?? 'qwen'), auto: !!flags.auto });
  if (cmd === 'serve') {
    const srv = await startWebServer(cfg, cwd, { port: Number(flags.port ?? 7878), log: (m) => console.log(m) });
    console.log(`Frankenstein Harness UI: ${srv.url}\nOne-time access code: ${c.bold(srv.code())}  (enter it in the page; it works once)\nBound to 127.0.0.1 only. Press Ctrl+C to stop.`);
    await new Promise(() => {});
  }

  const engine = new Engine(cfg, flags.yes ? headlessIO((m) => console.log(c.dim(m))) : terminalIO({ showReasoning: !!flags.thinking }), { cwd });
  if (flags.yes) engine.io = terminalIO({ assumeYes: true, showReasoning: !!flags.thinking });
  const ac = new AbortController();
  process.on('SIGINT', () => { if (ac.signal.aborted) process.exit(130); console.log(c.yellow('\ncancelling… (Ctrl+C again to force)')); ac.abort(); });
  const opts = { mode: (flags.auto ? 'auto' : 'normal') as 'auto' | 'normal', approval, commit: !!flags.commit, planOnly: !!flags['plan-only'], keepOnFail: !!flags.keep, sandbox: !!flags.sandbox, signal: ac.signal };

  if (cmd === 'run') {
    const task = positional.join(' ').trim();
    if (!task) { console.error('usage: fh run "<task>"'); return 2; }
    const r = await engine.runTask(task, opts);
    printResult(r, !!flags.json); await engine.drain();
    return r.verdict === 'pass' || r.verdict === 'planned' ? 0 : r.verdict === 'unverified' ? 3 : 1;
  }
  if (cmd === '' || cmd === 'chat') {
    console.log(c.bold('Frankenstein Harness') + c.dim(`  ${cfg.model} @ ${cfg.endpoint}  ·  ${opts.mode} · ${approval}  ·  /exit to quit`));
    for (;;) {
      const line = (await ask(c.cyan('› '))).trim();
      if (!line) continue;
      if (line === '/exit' || line === '/quit') break;
      if (line === '/undo') { await main(['undo', '--cwd', cwd]); continue; }
      const r = await engine.runTask(line, { ...opts, signal: undefined });
      printResult(r, false); await engine.drain();
    }
    return 0;
  }
  console.error(`unknown command: ${cmd}\n\n${HELP}`);
  return 2;
}
