import type { ToolSpec } from '../types.ts';

export interface ToolCtx {
  cwd: string;
  signal?: AbortSignal;
  /** When set, writes are only allowed to paths matching one of these globs (worker file ownership). */
  ownedGlobs?: string[];
  /** Files read this session -> content hash, used to require read-before-edit. */
  readCache: Map<string, string>;
  /** Paths modified this session (workspace-relative). */
  touched: Set<string>;
  /** Opt-in OS sandbox wrapper for shell commands. */
  wrapShell?: (c: { file: string; args: string[] }) => { file: string; args: string[] };
  bashTimeoutMs?: number;
}

export interface ToolResult { ok: boolean; output: string }

export interface Tool {
  spec: ToolSpec;
  /** true when the tool never mutates the workspace (safe to run in parallel, allowed in plan mode) */
  readOnly: boolean;
  execute(args: Record<string, unknown>, ctx: ToolCtx): Promise<ToolResult>;
}

export const str = (a: Record<string, unknown>, k: string, req = true): string => {
  const v = a[k];
  if (typeof v === 'string') return v;
  if (req) throw new Error(`missing or non-string argument: ${k}`);
  return '';
};
export const num = (a: Record<string, unknown>, k: string): number | undefined => {
  const v = a[k];
  if (typeof v === 'number') return v;
  if (typeof v === 'string' && v.trim() !== '' && !Number.isNaN(Number(v))) return Number(v);
  return undefined;
};
