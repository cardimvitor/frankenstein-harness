// No innerHTML anywhere: every piece of model/repo text is inserted with textContent (XSS from repo or model content
// would otherwise run with shell access behind this UI). Rendering helpers live in render.js.
import { el, renderMarkdown, renderDiff, renderCode, parseDiff } from './render.js';
const $ = (id) => document.getElementById(id);
const api = async (path, body) => {
  const res = await fetch(path, body === undefined ? { credentials: 'same-origin' } : {
    method: 'POST', credentials: 'same-origin', headers: { 'content-type': 'application/json', 'x-fh': '1' }, body: JSON.stringify(body),
  });
  return { ok: res.ok, status: res.status, json: await res.json().catch(() => ({})) };
};

let progressNode = null, reasoningNode = null, lastEventId = 0, running = false;
const timeline = () => $('timeline');
const scroll = () => window.scrollTo({ top: document.body.scrollHeight });
const add = (node) => { timeline().append(node); scroll(); return node; };
const li = (list, cls, text) => { const n = el('li', cls, text); list.append(n); return n; };
const resetList = (id, empty) => { const l = $(id); l.replaceChildren(); if (empty) li(l, 'muted', empty); return l; };
function setBusy(b, label) {
  const was = running; running = b; const s = $('status');
  if (was && !b) loadSessions();
  s.textContent = b ? (label || 'Working…') : 'Idle'; s.classList.toggle('busy', b);
  $('cancel').hidden = !b; $('send').disabled = b;
}

function askCard(id, kind, p) {
  const card = el('li', 'msg ask'); card.setAttribute('role', 'group');
  const done = (value, note) => { api('/api/answer', { id, value }); card.replaceChildren(el('p', 'muted', note)); };
  if (kind === 'questions') {
    card.append(el('strong', '', 'A few questions before I start'));
    const inputs = p.questions.map((q, i) => {
      const f = el('div', 'field'); const lab = el('label', '', `${i + 1}. ${q}`); const inp = el('input'); inp.id = `q${id}${i}`; lab.htmlFor = inp.id; f.append(lab, inp); card.append(f); return inp;
    });
    const b = el('button', 'primary', 'Continue'); b.type = 'button'; b.onclick = () => done(inputs.map((x) => x.value.trim() || '(use your best judgement)'), 'Answered.');
    card.append(el('div', 'actions')); card.lastChild.append(b);
  } else if (kind === 'plan') {
    card.append(el('strong', '', p.trivial ? 'Plan (small change)' : 'Plan'), el('pre', '', p.plan));
    const fb = el('input'); fb.placeholder = 'Optional feedback to change the plan'; fb.setAttribute('aria-label', 'Plan feedback');
    const go = el('button', 'primary', p.trivial ? 'Go' : 'Approve'); go.type = 'button'; go.onclick = () => done({ ok: true }, 'Plan approved.');
    const rev = el('button', '', 'Revise'); rev.type = 'button'; rev.onclick = () => done({ ok: false, feedback: fb.value.trim() || undefined }, 'Asked for a revised plan.');
    const no = el('button', 'danger', 'Cancel'); no.type = 'button'; no.onclick = () => done({ ok: false }, 'Plan cancelled.');
    const a = el('div', 'actions'); a.append(go, rev, no); card.append(fb, a);
  } else if (kind === 'confirm') {
    card.append(el('strong', '', `Allow ${p.tool}?`), el('pre', '', JSON.stringify(p.args, null, 2)));
    const y = el('button', 'primary', 'Allow once'); y.type = 'button'; y.onclick = () => done(true, `Allowed ${p.tool}.`);
    const n = el('button', 'danger', 'Deny'); n.type = 'button'; n.onclick = () => done(false, `Denied ${p.tool}.`);
    const a = el('div', 'actions'); a.append(y, n); card.append(a);
  } else if (kind === 'reuse') {
    card.append(el('strong', '', 'Reuse skills from another project?'));
    const picks = [];
    for (const o of p.offers) {
      card.append(el('p', 'muted', `"${o.fromLabel}" has ${o.skills.length} skill(s) for a similar stack:`));
      for (const s of o.skills) {
        const row = el('div', 'field'); const cb = el('input'); cb.type = 'checkbox'; cb.id = `r${id}${s.id}`; const lab = el('label', '', `${s.name} — ${s.summary}`); lab.htmlFor = cb.id; row.append(cb, lab); card.append(row); picks.push({ from: o.fromProject, id: s.id, cb });
      }
    }
    const collect = (all) => [...new Set(picks.map((x) => x.from))].map((f) => ({ fromProject: f, ids: picks.filter((x) => x.from === f && (all || x.cb.checked)).map((x) => x.id) }));
    const all = el('button', 'primary', 'Reuse all'); all.type = 'button'; all.onclick = () => done(collect(true), 'Reused all.');
    const sel = el('button', '', 'Reuse selected'); sel.type = 'button'; sel.onclick = () => done(collect(false), 'Reused selected.');
    const no = el('button', '', 'No'); no.type = 'button'; no.onclick = () => done([], 'Not reused.');
    const a = el('div', 'actions'); a.append(all, sel, no); card.append(a);
  }
  return card;
}

const handlers = {
  busy: (d) => { setBusy(d.busy); if (d.busy) { resetList('verify', 'Running…'); resetList('files', 'None'); } },
  notice: (d) => {
    add(el('li', `msg notice ${d.kind}`, d.message));
    if (d.kind === 'skill') { const l = $('skills'); if (l.firstChild?.classList?.contains('muted')) l.replaceChildren(); li(l, '', d.message); }
    if (d.kind === 'verify') { const l = $('verify'); if (l.firstChild?.classList?.contains('muted')) l.replaceChildren(); li(l, /pass|unverified/.test(d.message) ? 'ok' : 'fail', d.message); }
    progressNode = null; reasoningNode = null;
  },
  progress: (d) => {
    if (!progressNode) { const n = el('li', 'msg notice'); n.append(el('span')); progressNode = n.firstChild; add(n); }
    progressNode.textContent += d.d;
  },
  reasoning: (d) => {
    if (!reasoningNode) { const det = el('details'); det.append(el('summary', '', 'Thinking'), el('pre')); reasoningNode = det.querySelector('pre'); add(det); }
    reasoningNode.textContent += d.d;
  },
  tool_start: (d) => { progressNode = null; reasoningNode = null; },
  tool_end: (d) => {
    const det = el('details', `tool ${d.ok ? 'ok' : 'fail'}`); det.append(el('summary', '', `${d.name} · ${d.ok ? 'ok' : 'failed'} · ${d.ms}ms`), el('pre', '', d.output)); add(det);
  },
  ask: (d) => add(askCard(d.id, d.kind, d.payload)),
  result: (r) => {
    progressNode = null; reasoningNode = null;
    const card = el('li', `msg final ${r.verdict}`);
    card.append(renderMarkdown(r.final || r.reason || r.verdict));
    if (r.rejectedPatch) card.append(el('p', 'muted small', `Rejected patch saved: ${r.rejectedPatch}`));
    if (r.timings) card.append(el('p', 'muted small', `${(r.timings.totalMs / 1000).toFixed(1)}s · ${r.rounds} verification round(s) · ${r.llm?.completionTokens ?? 0} tokens out`));
    lastDiff = r.diff || '';
    if (r.diff) { const det = el('details'); det.append(el('summary', '', 'View diff'), renderDiff(r.diff)); card.append(det); }
    add(card);
    changedSet = new Set(r.changed ?? []); loadSideData();
    const files = resetList('files', r.changed?.length ? '' : 'None');
    for (const f of r.changed ?? []) li(files, '', f);
    const v = resetList('verify', r.verify?.rounds?.length ? '' : 'No checks');
    for (const round of r.verify?.rounds ?? []) {
      li(v, round.verdict === 'fail' ? 'fail' : 'ok', `Round ${round.round}: ${round.verdict}`);
      for (const c of round.checks) li(v, c.status === 'pass' ? 'ok' : c.status === 'fail' ? 'fail' : 'skip', `  ${c.name}: ${c.status}`);
    }
  },
};

function connect() {
  const es = new EventSource(`/api/events?after=${lastEventId}`);
  for (const [type, fn] of Object.entries(handlers)) es.addEventListener(type, (e) => { lastEventId = Number(e.lastEventId) || lastEventId; fn(JSON.parse(e.data)); });
  es.onerror = () => { es.close(); setTimeout(connect, 1500); };
}

let changedSet = new Set(), lastDiff = '';
async function loadTree() {
  const r = await api('/api/tree'); if (!r.ok) return;
  const root = {}; 
  for (const f of r.json.files) { let n = root; const parts = f.split('/'); parts.slice(0, -1).forEach((p) => { n = n[p] = n[p] || {}; }); n[parts.at(-1)] = f; }
  const draw = (node, into) => {
    for (const [k, v] of Object.entries(node).sort(([a, x], [b, y]) => (typeof x === 'object') === (typeof y === 'object') ? a.localeCompare(b) : (typeof x === 'object' ? -1 : 1))) {
      if (typeof v === 'string') { const b = el('button', `file${changedSet.has(v) ? ' changed' : ''}`, k); b.type = 'button'; b.title = v; b.onclick = () => openFile(v); into.append(b); continue; }
      const d = el('details'); d.append(el('summary', '', k)); draw(v, d); into.append(d);
    }
  };
  const t = $('tree'); t.replaceChildren(); draw(root, t);
}
async function loadSideData() {
  const a = await api('/api/activity');
  if (a.ok && a.json.length) { const l = resetList('skills', ''); for (const x of a.json.slice(0, 12)) li(l, '', `${x.kind}: ${x.skill} — ${x.reason}`); }
  const h = await api('/api/history');
  if (h.ok && h.json.length) { const l = resetList('history', ''); for (const x of h.json.slice(-12)) { const n = li(l, 'hist', `${x.skill} v${x.version} ${x.hash}\n${x.reason}`); } }
  await loadTree();
  await loadSessions();
}

async function openFile(path, line) {
  const r = await api(`/api/file?path=${encodeURIComponent(path)}`);
  const dlg = $('viewer'), body = $('viewer-body');
  $('viewer-title').textContent = path; body.replaceChildren();
  if (!r.ok) body.append(el('p', 'error', r.json.error || 'Cannot open this file'));
  else if (r.json.binary) body.append(el('p', 'muted', `Binary file (${r.json.size} bytes)`));
  else {
    if (lastDiff && parseDiff(lastDiff).some((f) => f.path === path)) { const d = el('details', 'viewer-changes'); d.open = true; d.append(el('summary', '', 'Changes from the last task'), renderDiff(lastDiff, path)); body.append(d); }
    if (r.json.truncated) body.append(el('p', 'muted small', `Showing the first ${r.json.content.length} characters of ${r.json.size} bytes.`));
    const code = renderCode(path, r.json.content); body.append(code);
    if (line) { const row = code.children[line - 1]; if (row) { row.classList.add('hit'); setTimeout(() => row.scrollIntoView({ block: 'center' }), 0); } }
  }
  if (typeof dlg.showModal === 'function') dlg.showModal(); else dlg.setAttribute('open', '');
}

async function loadSessions() {
  const r = await api('/api/sessions'); if (!r.ok) return;
  const l = resetList('sessions', r.json.length ? '' : 'None yet');
  for (const s of r.json.slice(0, 8)) {
    const row = li(l, s.verdict === 'pass' ? 'ok' : s.verdict ? (s.verdict === 'unverified' ? 'skip' : 'fail') : 'skip', '');
    row.append(el('span', '', `${s.verdict ?? 'interrupted'} · ${s.task}`));
    if (s.resumable) { const b = el('button', 'small-btn', 'Resume'); b.type = 'button'; b.onclick = async () => { const x = await api('/api/resume', { id: s.id, mode: $('mode').value, approval: $('approval').value }); if (!x.ok) add(el('li', 'msg notice warn', x.json.error || 'Could not resume')); }; row.append(b); }
  }
}

async function boot() {
  const st = await api('/api/state');
  if (st.status === 401) { $('login').hidden = false; $('code').focus(); return; }
  $('login').hidden = true; $('app').hidden = false;
  $('model').textContent = st.json.model || '';
  setBusy(!!st.json.busy);
  connect();
  loadSideData();
}

$('login-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const r = await api('/api/auth', { code: $('code').value.trim() });
  if (r.ok) boot(); else { const m = $('login-error'); m.textContent = 'That code is not valid. Check your terminal.'; m.hidden = false; }
});
$('composer').addEventListener('submit', async (e) => {
  e.preventDefault();
  const task = $('task').value.trim(); if (!task || running) return;
  add(el('li', 'msg user', task)); $('task').value = '';
  const r = await api('/api/task', { task, mode: $('mode').value, approval: $('approval').value });
  if (!r.ok) add(el('li', 'msg notice warn', r.json.error || 'Could not start the task'));
});
$('task').addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); $('composer').requestSubmit(); } });
$('cancel').addEventListener('click', () => api('/api/cancel', {}));
$('search-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const q = $('search-q').value.trim(), kind = $('search-kind').value, l = $('search-results');
  if (!q) return;
  l.replaceChildren(el('li', 'muted', 'Searching…'));
  const r = await api(`/api/search?kind=${kind}&q=${encodeURIComponent(q)}`);
  l.replaceChildren();
  if (!r.ok) { l.append(el('li', 'error', r.json.error || 'Search failed')); return; }
  if (kind === 'sessions') {
    if (!r.json.sessions.length) l.append(el('li', 'muted', 'No past tasks match'));
    for (const s of r.json.sessions) { const li2 = el('li', s.verdict === 'pass' ? 'ok' : s.verdict ? 'fail' : 'skip'); li2.append(el('strong', '', `${s.verdict ?? 'interrupted'} · ${s.task}`), el('div', 'muted small', s.snippet)); l.append(li2); }
    return;
  }
  for (const f of r.json.names) { const li2 = el('li'); const b = el('button', 'result-btn', f); b.type = 'button'; b.onclick = () => openFile(f); li2.append(b); l.append(li2); }
  for (const m of r.json.matches) { const li2 = el('li'); const b = el('button', 'result-btn'); b.type = 'button'; b.append(el('span', 'muted', `${m.path}:${m.line}  `), document.createTextNode(m.text)); b.onclick = () => openFile(m.path, m.line); li2.append(b); l.append(li2); }
  if (!r.json.names.length && !r.json.matches.length) l.append(el('li', 'muted', 'Nothing found'));
  if (r.json.truncated) l.append(el('li', 'muted small', 'More matches exist; refine the search.'));
});
$('viewer-close').addEventListener('click', () => $('viewer').close());
boot();
