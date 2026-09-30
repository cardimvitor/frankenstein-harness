export interface Frontmatter { meta: Record<string, string>; body: string }

/** Minimal `key: value` frontmatter (enough for SKILL.md). */
export function parseFrontmatter(text: string): Frontmatter {
  const m = text.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/);
  if (!m) return { meta: {}, body: text };
  const meta: Record<string, string> = {};
  for (const line of m[1].split(/\r?\n/)) {
    const kv = line.match(/^([A-Za-z_][\w-]*):\s*(.*)$/);
    if (kv) meta[kv[1]] = kv[2].replace(/^["']|["']$/g, '').trim();
  }
  return { meta, body: m[2].trim() };
}

/** "8,9,10" or "17+" -> predicate. Empty means any. */
export function versionMatcher(spec: string | undefined): (v: string | undefined) => boolean {
  if (!spec) return () => true;
  const parts = spec.split(',').map((s) => s.trim()).filter(Boolean);
  return (v) => {
    if (!v) return false;
    const n = Number(v);
    return parts.some((p) => (p.endsWith('+') ? n >= Number(p.slice(0, -1)) : String(n) === p));
  };
}
