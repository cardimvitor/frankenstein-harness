import type { LlmClient } from '../llm/client.ts';
import type { Mode } from '../types.ts';
import { runAgent, type AgentResult } from '../agent/loop.ts';
import type { Subtask } from '../funnel/intake.ts';
import { planWaves } from './partition.ts';
import type { Governor } from './governor.ts';
import { matchesAny } from '../util/paths.ts';

export interface WorkerResult { id: string; summary: string; touched: string[]; stopped: AgentResult['stopped']; steps: number }

export interface MasterOptions {
  llm: LlmClient;
  cwd: string;
  mode: Mode;
  context: string;
  governor: Governor;
  signal?: AbortSignal;
  contextWindow?: number;
  maxSteps?: number;
  notice?: (m: string) => void;
  wrapShell?: Parameters<typeof runAgent>[1]['wrapShell'];
}

const workerTask = (s: Subtask, all: Subtask[], goal: string) => [
  `You are a worker on a larger task. Overall goal: ${goal}`,
  `Your subtask (${s.id}): ${s.goal}`,
  `You may only modify files matching: ${s.files.join(', ')}. Other files are owned by other workers; read them if needed but do not edit them.`,
  `Other subtasks running or done: ${all.filter((x) => x.id !== s.id).map((x) => `${x.id}: ${x.goal}`).join(' | ') || 'none'}`,
  'Do only your subtask. Finish with a 2-3 line summary of what you changed.',
].join('\n');

/** Master: dispatch narrow-scoped workers wave by wave under the governor; consolidate summaries. Nothing from workers is shown to the user. */
export async function runWorkers(goal: string, subtasks: Subtask[], o: MasterOptions): Promise<WorkerResult[]> {
  const waves = planWaves(subtasks);
  const results: WorkerResult[] = [];
  o.governor.start();
  try {
    for (const [wi, wave] of waves.entries()) {
      if (o.signal?.aborted) break;
      o.notice?.(`wave ${wi + 1}/${waves.length}: ${wave.length} worker(s)`);
      const rs = await Promise.all(wave.map((s) => o.governor.run(async () => {
        const r = await runAgent(workerTask(s, subtasks, goal), {
          llm: o.llm, cwd: o.cwd, mode: o.mode, thinking: 'auto', maxSteps: o.maxSteps ?? 25, signal: o.signal,
          ownedGlobs: s.files, context: o.context, contextWindow: o.contextWindow, wrapShell: o.wrapShell,
        });
        return { id: s.id, summary: r.final || r.error || '', touched: r.touched, stopped: r.stopped, steps: r.steps } as WorkerResult;
      })));
      results.push(...rs);
    }
  } finally { o.governor.stop(); }
  return results;
}

/** Which subtask (worker) owns the files named in a failure report; unmatched failures go to the master's own fixer. */
export function ownersOf(subtasks: Subtask[], files: string[]): { owned: Map<string, string[]>; unowned: string[] } {
  const owned = new Map<string, string[]>();
  const unowned: string[] = [];
  for (const f of files) {
    const s = subtasks.find((t) => matchesAny(f, t.files));
    if (s) owned.set(s.id, [...(owned.get(s.id) ?? []), f]); else unowned.push(f);
  }
  return { owned, unowned };
}
