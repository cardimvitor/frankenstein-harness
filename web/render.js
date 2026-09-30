// Pure DOM builders for the UI. No innerHTML anywhere: every piece of model or repository text goes in through
// textContent / text nodes, so content from the repo or the model can never become markup.
export const el = (tag, cls, text) => { const n = document.createElement(tag); if (cls) n.className = cls; if (text !== undefined) n.textContent = text; return n; };

// ---------- inline markdown: `code`, **bold**, *italic*, [text](url) shown as plain text, ![alt](url) as alt text
export function inline(node, text) {
  const re = /(`[^`]+`|\*\*[^*]+\*\*|!\[[^\]]*\]\([^)\s]*\)|\[[^\]]+\]\([^)\s]*\)|\*[^*\s][^*]*\*)/;
  for (const seg of text.split(re)) {
    if (!seg) continue;
    let m;
    if (seg.startsWith('`') && seg.endsWith('`') && seg.length > 2) node.append(el('code', '', seg.slice(1, -1)));
    else if (seg.startsWith('**') && seg.endsWith('**') && seg.length > 4) node.append(el('strong', '', seg.slice(2, -2)));
    else if ((m = /^!\[([^\]]*)\]\(([^)\s]*)\)$/.exec(seg))) node.append(document.createTextNode(m[1] || 'image'));
    else if ((m = /^\[([^\]]+)\]\(([^)\s]*)\)$/.exec(seg))) { node.append(document.createTextNode(m[1])); if (m[2]) node.append(el('span', 'url', ` (${m[2]})`)); } // never an <a>: links are shown, not followed
    else if (/^\*[^*\s][^*]*\*$/.test(seg)) node.append(el('em', '', seg.slice(1, -1)));
    else node.append(document.createTextNode(seg));
  }
  return node;
}

const isRow = (l) => /^\s*\|.*\|\s*$/.test(l);
const isSep = (l) => /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/.test(l) && l.includes('-');
const cells = (l) => l.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map((c) => c.trim());

/** Paragraphs, headings, fenced code (with highlighting), tables, ordered/unordered lists, blockquotes, rules. */
export function renderMarkdown(text) {
  const root = el('div', 'md');
  const lines = text.split('\n');
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const fence = /^\s*```(\S*)/.exec(line);
    if (fence) {
      const body = []; i++;
      while (i < lines.length && !/^\s*```/.test(lines[i])) body.push(lines[i++]);
      i++;
      const pre = el('pre'); pre.append(highlight(fence[1] || '', body.join('\n'))); root.append(pre); continue;
    }
    if (!line.trim()) { i++; continue; }
    const h = /^(#{1,6})\s+(.*)$/.exec(line);
    if (h) { root.append(inline(el(`h${Math.min(h[1].length + 2, 6)}`, 'mdh'), h[2])); i++; continue; }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) { root.append(el('hr')); i++; continue; }
    if (isRow(line) && i + 1 < lines.length && isSep(lines[i + 1])) {
      const table = el('table'); const thead = el('thead'); const tr = el('tr');
      for (const c of cells(line)) tr.append(inline(el('th'), c));
      thead.append(tr); table.append(thead);
      const tbody = el('tbody'); i += 2;
      while (i < lines.length && isRow(lines[i])) { const r = el('tr'); for (const c of cells(lines[i])) r.append(inline(el('td'), c)); tbody.append(r); i++; }
      table.append(tbody); const wrap = el('div', 'tablewrap'); wrap.append(table); root.append(wrap); continue;
    }
    if (/^\s*>\s?/.test(line)) {
      const q = []; while (i < lines.length && /^\s*>\s?/.test(lines[i])) q.push(lines[i++].replace(/^\s*>\s?/, ''));
      root.append(inline(el('blockquote'), q.join(' '))); continue;
    }
    const ul = /^\s*[-*+]\s+/, ol = /^\s*\d+[.)]\s+/;
    if (ul.test(line) || ol.test(line)) {
      const ordered = ol.test(line); const list = el(ordered ? 'ol' : 'ul'); const re = ordered ? ol : ul;
      while (i < lines.length && re.test(lines[i])) list.append(inline(el('li'), lines[i++].replace(re, '')));
      root.append(list); continue;
    }
    const para = [];
    while (i < lines.length && lines[i].trim() && !/^\s*```/.test(lines[i]) && !/^(#{1,6})\s/.test(lines[i]) && !/^\s*>\s?/.test(lines[i]) && !ul.test(lines[i]) && !ol.test(lines[i]) && !(isRow(lines[i]) && isSep(lines[i + 1] ?? ''))) para.push(lines[i++]);
    root.append(inline(el('p'), para.join('\n')));
  }
  return root;
}

// ---------- syntax highlighting (small tokenizer, no dependencies)
const C_LIKE = ['if', 'else', 'for', 'while', 'do', 'switch', 'case', 'break', 'continue', 'return', 'new', 'class', 'struct', 'interface', 'enum', 'public', 'private', 'protected', 'static', 'void', 'int', 'long', 'float', 'double', 'bool', 'boolean', 'string', 'char', 'true', 'false', 'null', 'this', 'try', 'catch', 'finally', 'throw', 'throws', 'using', 'namespace', 'import', 'package', 'extends', 'implements', 'override', 'abstract', 'sealed', 'readonly', 'async', 'await', 'var', 'let', 'const', 'in', 'is', 'as', 'typeof', 'default', 'yield', 'get', 'set'];
const JS = ['function', 'export', 'from', 'of', 'undefined', 'typeof', 'instanceof', 'delete', 'type', 'declare', 'module', 'require', ...C_LIKE];
const LANGS = {
  js: { kw: JS, lc: '//', bc: true, q: '"\'`' }, ts: null, jsx: null, tsx: null, mjs: null, cjs: null,
  py: { kw: ['def', 'class', 'return', 'if', 'elif', 'else', 'for', 'while', 'in', 'not', 'and', 'or', 'is', 'None', 'True', 'False', 'import', 'from', 'as', 'with', 'try', 'except', 'finally', 'raise', 'lambda', 'yield', 'pass', 'break', 'continue', 'async', 'await', 'self', 'global', 'nonlocal', 'assert', 'del'], lc: '#', q: '"\'' },
  cs: { kw: [...C_LIKE, 'partial', 'virtual', 'internal', 'params', 'ref', 'out', 'foreach', 'record', 'where', 'select', 'from', 'var'], lc: '//', bc: true, q: '"\'' },
  java: { kw: C_LIKE, lc: '//', bc: true, q: '"\'' }, go: { kw: ['func', 'package', 'import', 'return', 'if', 'else', 'for', 'range', 'switch', 'case', 'default', 'struct', 'interface', 'type', 'var', 'const', 'map', 'chan', 'go', 'defer', 'select', 'nil', 'true', 'false', 'break', 'continue'], lc: '//', bc: true, q: '"\'`' },
  rs: { kw: ['fn', 'let', 'mut', 'pub', 'use', 'mod', 'struct', 'enum', 'impl', 'trait', 'match', 'if', 'else', 'for', 'while', 'loop', 'return', 'self', 'Self', 'true', 'false', 'async', 'await', 'move', 'ref', 'as', 'in', 'where', 'const', 'static', 'unsafe', 'crate', 'dyn', 'type'], lc: '//', bc: true, q: '"' },
  c: { kw: C_LIKE, lc: '//', bc: true, q: '"\'' },
  sh: { kw: ['if', 'then', 'else', 'elif', 'fi', 'for', 'do', 'done', 'while', 'case', 'esac', 'function', 'return', 'export', 'local', 'in'], lc: '#', q: '"\'' },
  json: { kw: ['true', 'false', 'null'], q: '"' },
  css: { kw: [], bc: true, q: '"\'', at: true },
  html: { kw: [], tags: true, q: '"\'' },
  yaml: { kw: ['true', 'false', 'null'], lc: '#', q: '"\'' },
};
for (const k of ['ts', 'jsx', 'tsx', 'mjs', 'cjs']) LANGS[k] = LANGS.js;
const ALIAS = { javascript: 'js', typescript: 'ts', python: 'py', csharp: 'cs', rust: 'rs', golang: 'go', bash: 'sh', shell: 'sh', zsh: 'sh', htm: 'html', xml: 'html', vue: 'html', svelte: 'html', yml: 'yaml', cpp: 'c', h: 'c', hpp: 'c', cc: 'c', kt: 'java', scss: 'css' };

export function langOf(pathOrLang) {
  const ext = (pathOrLang.includes('.') ? pathOrLang.split('.').pop() : pathOrLang).toLowerCase();
  const k = ALIAS[ext] ?? ext;
  return LANGS[k] ? k : '';
}

const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const compiled = new Map();
function compile(key) {
  const c = LANGS[key]; if (compiled.has(key)) return compiled.get(key);
  const parts = [];
  if (c.bc) parts.push('\\/\\*[^]*?(?:\\*\\/|$)');
  if (c.lc) parts.push(`${escapeRe(c.lc)}.*`);
  if (c.q) parts.push(...[...c.q].map((q) => `${escapeRe(q)}(?:\\\\.|[^${escapeRe(q)}\\\\\\n])*(?:${escapeRe(q)}|$)`));
  if (c.tags) parts.push('<\\/?[A-Za-z][\\w:-]*|\\/?>');
  if (c.at) parts.push('@[\\w-]+');
  parts.push('\\b\\d[\\w.]*\\b', '[A-Za-z_$][\\w$]*');
  const out = { re: new RegExp(parts.map((p) => `(${p})`).join('|'), 'g'), c, kw: new Set(c.kw), nGroups: parts.length };
  compiled.set(key, out); return out;
}

/** One line of code as a DocumentFragment of spans (tok-com, tok-str, tok-num, tok-kw). `block` carries /* state between lines. */
export function highlightLine(lang, line, block = { open: false }) {
  const frag = document.createDocumentFragment();
  const key = langOf(lang);
  if (!key) { frag.append(document.createTextNode(line)); return frag; }
  const { re, c, kw } = compile(key);
  let pos = 0; let src = line;
  if (block.open) {
    const end = src.indexOf('*/');
    if (end < 0) { frag.append(el('span', 'tok-com', src)); return frag; }
    frag.append(el('span', 'tok-com', src.slice(0, end + 2))); pos = end + 2; block.open = false;
  }
  re.lastIndex = pos;
  let m;
  while ((m = re.exec(src)) !== null) {
    if (m.index > pos) frag.append(document.createTextNode(src.slice(pos, m.index)));
    const t = m[0]; let cls = '';
    if (c.bc && t.startsWith('/*')) { cls = 'tok-com'; if (!t.endsWith('*/') || t === '/*/') block.open = true; }
    else if (c.lc && t.startsWith(c.lc)) cls = 'tok-com';
    else if (c.q && c.q.includes(t[0])) cls = 'tok-str';
    else if (c.tags && (t.startsWith('<') || t.startsWith('/>') || t === '>')) cls = 'tok-kw';
    else if (c.at && t.startsWith('@')) cls = 'tok-kw';
    else if (/^\d/.test(t)) cls = 'tok-num';
    else if (kw.has(t)) cls = 'tok-kw';
    frag.append(cls ? el('span', cls, t) : document.createTextNode(t));
    pos = m.index + t.length;
    if (t.length === 0) re.lastIndex++;
  }
  if (pos < src.length) frag.append(document.createTextNode(src.slice(pos)));
  return frag;
}

export function highlight(lang, code) {
  const frag = document.createDocumentFragment(); const block = { open: false };
  code.split('\n').forEach((l, i, a) => { frag.append(highlightLine(lang, l, block)); if (i < a.length - 1) frag.append(document.createTextNode('\n')); });
  return frag;
}

// ---------- files and diffs
/** A file with line numbers and highlighting. */
export function renderCode(path, content) {
  const lang = langOf(path); const block = { open: false };
  const box = el('div', 'code');
  content.split('\n').forEach((l, i, a) => {
    if (i === a.length - 1 && l === '') return;
    const row = el('div', 'cl'); row.append(el('span', 'ln', String(i + 1)));
    const code = el('span', 'cc'); code.append(highlightLine(lang, l, block)); row.append(code); box.append(row);
  });
  return box;
}

/** Unified diff text -> [{path, adds, dels, isNew, isDeleted, binary, hunks: [{header, lines: [{t, o, n, text}]}]}] */
export function parseDiff(text) {
  const files = []; let f = null, h = null, o = 0, n = 0;
  for (const raw of text.split('\n')) {
    let m;
    if (raw.startsWith('diff --git ')) {
      m = /^diff --git a\/(.*) b\/(.*)$/.exec(raw);
      f = { path: m ? m[2] : raw.slice(11), adds: 0, dels: 0, isNew: false, isDeleted: false, binary: false, hunks: [] }; files.push(f); h = null; continue;
    }
    if (!f) continue;
    if (raw.startsWith('new file mode')) f.isNew = true;
    else if (raw.startsWith('deleted file mode')) f.isDeleted = true;
    else if (raw.startsWith('Binary files') || raw.startsWith('GIT binary patch')) f.binary = true;
    else if ((m = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/.exec(raw))) { o = +m[1]; n = +m[2]; h = { header: raw, lines: [] }; f.hunks.push(h); }
    else if (h && raw.startsWith('+')) { h.lines.push({ t: 'add', o: null, n: n++, text: raw.slice(1) }); f.adds++; }
    else if (h && raw.startsWith('-')) { h.lines.push({ t: 'del', o: o++, n: null, text: raw.slice(1) }); f.dels++; }
    else if (h && raw.startsWith(' ')) { h.lines.push({ t: 'ctx', o: o++, n: n++, text: raw.slice(1) }); }
  }
  return files;
}

function fileDiff(f, open) {
  const det = el('details', 'diff-file'); det.open = open;
  det.append(el('summary', '', `${f.path}${f.isNew ? ' (new)' : f.isDeleted ? ' (deleted)' : ''}  +${f.adds} −${f.dels}`));
  const body = el('div', 'diff-body');
  if (f.binary) body.append(el('p', 'muted small', 'Binary file'));
  const lang = langOf(f.path);
  for (const h of f.hunks) {
    body.append(el('div', 'hunk-head', h.header));
    const block = { open: false };
    for (const l of h.lines) {
      const row = el('div', `dl ${l.t}`);
      row.append(el('span', 'ln', l.o ?? ''), el('span', 'ln', l.n ?? ''), el('span', 'sign', l.t === 'add' ? '+' : l.t === 'del' ? '−' : ' '));
      const code = el('span', 'cc'); code.append(highlightLine(lang, l.text, block)); row.append(code); body.append(row);
    }
  }
  det.append(body); return det;
}

/** Per-file, collapsible, highlighted diff. `only` limits it to one path. */
export function renderDiff(text, only) {
  const root = el('div', 'diff');
  const files = parseDiff(text).filter((f) => !only || f.path === only);
  if (!files.length) root.append(el('p', 'muted small', 'No changes.'));
  files.forEach((f, i) => root.append(fileDiff(f, files.length <= 3 || i === 0)));
  return root;
}
