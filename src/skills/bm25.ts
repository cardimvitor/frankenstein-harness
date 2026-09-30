const STOP = new Set('a an the and or of to in on for with is are be this that it as at by from into can you your we our not do does did make add fix use using create update change should would could please'.split(' '));

export function tokenize(text: string): string[] {
  return text
    .replace(/([a-z])([A-Z])/g, '$1 $2')
    .toLowerCase()
    .split(/[^a-z0-9#@.+]+/)
    .map((t) => t.replace(/^[.+]+|[.+]+$/g, ''))
    .filter((t) => t.length > 1 && !STOP.has(t));
}

export interface Doc { id: string; text: string; boost?: string }

/** Small in-memory BM25 (k1=1.4, b=0.75). Corpora are tens to hundreds of skills, so this runs in well under a millisecond. */
export class Bm25 {
  private docs: { id: string; tf: Map<string, number>; len: number }[] = [];
  private df = new Map<string, number>();
  private avg = 1;
  constructor(docs: Doc[]) {
    for (const d of docs) {
      const toks = [...tokenize(d.text), ...tokenize(d.boost ?? ''), ...tokenize(d.boost ?? '')];
      const tf = new Map<string, number>();
      for (const t of toks) tf.set(t, (tf.get(t) ?? 0) + 1);
      for (const t of tf.keys()) this.df.set(t, (this.df.get(t) ?? 0) + 1);
      this.docs.push({ id: d.id, tf, len: toks.length });
    }
    this.avg = this.docs.reduce((n, d) => n + d.len, 0) / Math.max(1, this.docs.length);
  }
  score(query: string[]): Map<string, number> {
    const N = this.docs.length, out = new Map<string, number>();
    const q = [...new Set(query)];
    for (const d of this.docs) {
      let s = 0;
      for (const t of q) {
        const f = d.tf.get(t);
        if (!f) continue;
        const idf = Math.log(1 + (N - (this.df.get(t) ?? 0) + 0.5) / ((this.df.get(t) ?? 0) + 0.5));
        s += idf * ((f * 2.4) / (f + 1.4 * (0.25 + 0.75 * (d.len / this.avg))));
      }
      if (s > 0) out.set(d.id, s);
    }
    return out;
  }
}
