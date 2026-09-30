// No innerHTML anywhere: every piece of model/repo text is inserted with textContent (XSS from repo or model content
// would otherwise run with shell access behind this UI).
const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => { const n = document.createElement(tag); if (cls) n.className = cls; if (text !== undefined) n.textContent = text; return n; };
const api = async (path, body) => {
  const res = await fetch(path, body === undefined ? { credentials: 'same-origin' } : {
    method: 'POST', credentials: 'same-origin', headers: { 'content-type': 'application/json', 'x-fh': '1' }, body: JSON.stringify(body),
  });
  return { ok: res.ok, status: res.status, json: await res.json().catch(() => ({})) };
};

/** Minimal safe markdown: paragraphs, ``` fences, `code`, **bold**, - lists. No raw HTML, no links. */
function renderMarkdown(text) {
  const root = el('div');
  const parts = text.split(/```[^\n]*\n?/);
  parts.forEach((chunk, i) => {
    if (i % 2 === 1) { root.append(el('pre', '', chunk.replace(/\n$/, ''))); return; }
    for (const block of chunk.split(/\n{2,}/)) {
      if (!block.trim()) continue;
      const lines = block.split('\n');
      if (lines.every((l) => /^\s*[-*]\s+/.test(l))) {
        const ul = el('ul'); for (const l of lines) ul.append(inline(el('li'), l.replace(/^\s*[-*]\s+/, ''))); root.append(ul);
      } else root.append(inline(el('p'), block));
    }
  });
  return root;
}
function inline(node, text) {
  for (const seg of text.split(/(`[^`]+`|\*\*[^*]+\*\*)/)) {
    if (seg.startsWith('`') && seg.endsWith('`') && seg.length > 2) node.append(el('code', '', seg.slice(1, -1)));
    else if (seg.startsWith('**') && seg.endsWith('**') && seg.length > 4) node.append(el('strong', '', seg.slice(2, -2)));
    else node.append(document.createTextNode(seg));
  }
  return node;
}

let progressNode = null, reasoningNode = null, lastEventId = 0, running = false;
const timeline = () => $('timeline');
const scroll = () => window.scrollTo({ top: document.body.scrollHeight });
const add = (node) => { timeline().append(node); scroll(); return node; };
const li = (list, cls, text) => { const n = el('li', cls, text); list.append(n); return n; };
const resetList = (id, empty) => { const l = $(id); l.replaceChildren(); if (empty) li(l, 'muted', empty); return l; };
function setBusy(b, label) {
  running = b; const s = $('status');
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
    if (r.diff) { const det = el('details'); det.append(el('summary', '', 'View diff'), el('pre', '', r.diff)); card.append(det); }
    add(card);
    const files = resetList('files', r.changed?.length ? '' : 'None');
    for (const f of r.changed ?? []) li(files, '', f);
    const v = resetList('verify', 'No checks');
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

async function boot() {
  const st = await api('/api/state');
  if (st.status === 401) { $('login').hidden = false; $('code').focus(); return; }
  $('login').hidden = true; $('app').hidden = false;
  $('model').textContent = st.json.model || '';
  setBusy(!!st.json.busy);
  connect();
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
boot();
