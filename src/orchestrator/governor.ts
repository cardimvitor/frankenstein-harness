import type { VllmMetrics } from '../llm/metrics.ts';

/**
 * Adaptive concurrency for workers. Too many parallel long contexts evict each other's prefix cache and
 * slow the user down, so the limit follows vLLM's KV-cache usage and queue depth.
 */
export class Governor {
  limit: number;
  readonly max: number;
  private active = 0;
  private waiters: (() => void)[] = [];
  private timer?: NodeJS.Timeout;
  history: { limit: number; kv?: number; waiting?: number }[] = [];
  private readMetrics?: () => Promise<VllmMetrics | undefined>;
  constructor(max: number, readMetrics?: () => Promise<VllmMetrics | undefined>, start = 2) {
    this.readMetrics = readMetrics;
    this.max = Math.max(1, max);
    this.limit = Math.min(this.max, Math.max(1, start));
  }

  /** Pure decision so it can be tested without timers. */
  static next(limit: number, max: number, m: { kvUsage?: number; waiting?: number; running?: number } | undefined): number {
    if (!m) return limit;
    if ((m.kvUsage ?? 0) > 0.85 || (m.waiting ?? 0) > 0) return Math.max(1, limit - 1);
    if ((m.kvUsage ?? 1) < 0.5 && (m.waiting ?? 0) === 0) return Math.min(max, limit + 1);
    return limit;
  }

  async tick() {
    const m = await this.readMetrics?.();
    this.limit = Governor.next(this.limit, this.max, m);
    this.history.push({ limit: this.limit, kv: m?.kvUsage, waiting: m?.waiting });
    this.drain();
  }
  start(ms = 2000) { if (this.readMetrics && !this.timer) { this.timer = setInterval(() => void this.tick(), ms); this.timer.unref(); } }
  stop() { clearInterval(this.timer); this.timer = undefined; }

  private drain() { while (this.active < this.limit && this.waiters.length) { this.active++; this.waiters.shift()!(); } }
  async acquire(): Promise<() => void> {
    await new Promise<void>((res) => { this.waiters.push(res); this.drain(); });
    let released = false;
    return () => { if (released) return; released = true; this.active--; this.drain(); };
  }
  async run<T>(fn: () => Promise<T>): Promise<T> { const rel = await this.acquire(); try { return await fn(); } finally { rel(); } }
}
