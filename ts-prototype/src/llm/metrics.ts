import type { Config } from '../config.ts';
import { authHeaders } from '../config.ts';

export interface VllmMetrics {
  kvUsage?: number;
  running?: number;
  waiting?: number;
  prefixHits?: number;
  prefixQueries?: number;
  specDrafts?: number;
  specAccepted?: number;
  specDraftTokens?: number;
  /** accepted tokens per draft position (index 0 = first draft token) */
  specAcceptedPerPos?: number[];
  raw: Record<string, number>;
}

/** Parse Prometheus text, summing series that share a metric name. */
export function parsePrometheus(text: string): Record<string, number> {
  const out: Record<string, number> = {};
  for (const line of text.split('\n')) {
    if (!line || line.startsWith('#')) continue;
    const m = line.match(/^([a-zA-Z_:][\w:]*)(\{[^}]*\})?\s+([-+eE.\dNaInf]+)/);
    if (!m) continue;
    const v = Number(m[3]);
    if (!Number.isFinite(v)) continue;
    const pos = m[2]?.match(/position="(\d+)"/)?.[1];
    const key = pos !== undefined ? `${m[1]}#pos${pos}` : m[1];
    out[key] = (out[key] ?? 0) + v;
  }
  return out;
}

const pick = (raw: Record<string, number>, ...names: string[]) => {
  for (const n of names) for (const k of [n, n.replace(/_total$/, '')]) if (raw[k] !== undefined) return raw[k];
  return undefined;
};

export function summarize(raw: Record<string, number>): VllmMetrics {
  const perPos: number[] = [];
  for (const [k, v] of Object.entries(raw)) {
    const m = k.match(/spec_decode_num_accepted_tokens_per_pos(?:_total)?#pos(\d+)$/);
    if (m) perPos[Number(m[1])] = v;
  }
  return {
    kvUsage: pick(raw, 'vllm:kv_cache_usage_perc', 'vllm:gpu_cache_usage_perc'),
    running: pick(raw, 'vllm:num_requests_running'),
    waiting: pick(raw, 'vllm:num_requests_waiting'),
    prefixHits: pick(raw, 'vllm:prefix_cache_hits_total', 'vllm:gpu_prefix_cache_hits_total'),
    prefixQueries: pick(raw, 'vllm:prefix_cache_queries_total', 'vllm:gpu_prefix_cache_queries_total'),
    specDrafts: pick(raw, 'vllm:spec_decode_num_drafts_total'),
    specAccepted: pick(raw, 'vllm:spec_decode_num_accepted_tokens_total'),
    specDraftTokens: pick(raw, 'vllm:spec_decode_num_draft_tokens_total'),
    specAcceptedPerPos: perPos.length ? perPos : undefined,
    raw,
  };
}

export async function fetchMetrics(cfg: Config, signal?: AbortSignal): Promise<VllmMetrics | undefined> {
  try {
    const res = await fetch(cfg.metricsUrl, { headers: authHeaders(cfg), signal: signal ?? AbortSignal.timeout(3000) });
    if (!res.ok) return undefined;
    return summarize(parsePrometheus(await res.text()));
  } catch {
    return undefined;
  }
}

export interface MetricsDelta {
  prefixHitRate?: number;
  acceptanceRate?: number;
  meanAcceptedPerDraft?: number;
  perPositionAcceptance?: number[];
}

/** Compute deltas between two snapshots (counters are cumulative). */
export function delta(a: VllmMetrics | undefined, b: VllmMetrics | undefined): MetricsDelta {
  if (!a || !b) return {};
  const d: MetricsDelta = {};
  if (a.prefixQueries !== undefined && b.prefixQueries !== undefined && b.prefixHits !== undefined && a.prefixHits !== undefined) {
    const q = b.prefixQueries - a.prefixQueries;
    if (q > 0) d.prefixHitRate = (b.prefixHits - a.prefixHits) / q;
  }
  if (a.specDraftTokens !== undefined && b.specDraftTokens !== undefined && a.specAccepted !== undefined && b.specAccepted !== undefined) {
    const dt = b.specDraftTokens - a.specDraftTokens;
    if (dt > 0) d.acceptanceRate = (b.specAccepted - a.specAccepted) / dt;
  }
  if (a.specDrafts !== undefined && b.specDrafts !== undefined && a.specAccepted !== undefined && b.specAccepted !== undefined) {
    const dd = b.specDrafts - a.specDrafts;
    if (dd > 0) d.meanAcceptedPerDraft = (b.specAccepted - a.specAccepted) / dd;
    if (b.specAcceptedPerPos && dd > 0) {
      d.perPositionAcceptance = b.specAcceptedPerPos.map((v, i) => (v - (a.specAcceptedPerPos?.[i] ?? 0)) / dd);
    }
  }
  return d;
}
