// Drives the real web UI in headless Chromium against a mock vLLM (no model needed):
//   PLAYWRIGHT_MODULE=$(npm root -g)/playwright node --disable-warning=ExperimentalWarning scripts/ui-smoke.mjs [outDir]
// Checks: login with the one-time code, guided run with plan approval card, streamed events, verified result card,
// diff view, no CSP violations or console errors, light and dark screenshots.
import { createRequire } from 'node:module';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execSync } from 'node:child_process';
import assert from 'node:assert/strict';

const { chromium } = createRequire(import.meta.url)(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
const { startMock } = await import('../test/mock-vllm.ts');
const { loadConfig } = await import('../src/config.ts');
const { startWebServer } = await import('../src/ui/server.ts');
const { Engine, headlessIO } = await import('../src/engine.ts');
const { SkillStore } = await import('../src/skills/store.ts');
const { BUILTIN_TASKS } = await import('../src/eval/corpus.ts');

const out = process.argv[2] ?? join(tmpdir(), 'fh-ui');
mkdirSync(out, { recursive: true });
const m = await startMock();
const d = mkdtempSync(join(tmpdir(), 'fh-ui-repo-'));
for (const [f, b] of Object.entries(BUILTIN_TASKS[0].files)) { mkdirSync(join(d, f, '..'), { recursive: true }); writeFileSync(join(d, f), b); }
execSync('git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -qm b', { cwd: d });
m.fallback = (req) => {
  const p = req.response_format?.json_schema?.schema?.properties;
  if (p?.trivial) return { content: JSON.stringify({ trivial: false, questions: [], enriched: 'Make sumRange inclusive', acceptance: ['tests pass'], plan: [{ step: 'Change the loop bound to <=', files: ['lib/math.js'] }], assumptions: ['inclusive on both ends'], subtasks: [] }) };
  if (p?.verdict) return { content: JSON.stringify({ verdict: 'pass', findings: [] }) };
  if (p?.create !== undefined) return { content: '{"create":false}' };
  if (req.messages.at(-1).role === 'tool') return { content: 'Changed the loop bound to `<=` so **both ends** are included.\n\n- lib/math.js updated\n- tests pass' };
  return { reasoning: 'The loop stops one short of b.', tool_calls: [{ name: 'edit', args: { path: 'lib/math.js', old_text: 'i < b; i++', new_text: 'i <= b; i++' } }] };
};
const cfg = { ...loadConfig('/x', {}), endpoint: m.url, metricsUrl: m.url.replace('/v1', '/metrics'), retries: 0, model: 'qwen-mock' };
const srv = await startWebServer(cfg, d, { engine: new Engine(cfg, headlessIO(), { cwd: d, store: new SkillStore(':memory:') }) });

const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || undefined });
const problems = [];
try {
  for (const scheme of ['light', 'dark']) {
    const ctx = await browser.newContext({ colorScheme: scheme, viewport: { width: 1280, height: 900 } });
    const page = await ctx.newPage();
    // 401s are the expected login probe and the wrong-code attempt
    page.on('console', (msg) => { if (msg.type() === 'error' && !/status of 401/.test(msg.text())) problems.push(`console: ${msg.text()}`); });
    page.on('pageerror', (e) => problems.push(`pageerror: ${e.message}`));
    await page.addInitScript(() => document.addEventListener('securitypolicyviolation', (e) => console.error('CSP violation: ' + e.violatedDirective + ' ' + e.blockedURI)));
    await page.goto(srv.url);
    await page.waitForSelector('#login:not([hidden])');
    await page.fill('#code', 'wrong-code');
    await page.click('#login-form button');
    await page.waitForSelector('#login-error:not([hidden])');
    await page.fill('#code', srv.code());
    await page.click('#login-form button');
    await page.waitForSelector('#app:not([hidden])');
    assert.equal(await page.textContent('#model'), 'qwen-mock');
    if (scheme === 'light') {
      await page.selectOption('#approval', 'auto-edit');
      await page.fill('#task', 'fix sumRange so both ends are included');
      await page.press('#task', 'Enter');
      await page.waitForSelector('.ask');
      assert.match(await page.textContent('.ask'), /Change the loop bound/);
      await page.screenshot({ path: join(out, 'plan-approval-light.png') });
      await page.click('.ask button.primary');
      await page.waitForSelector('.msg.final.pass', { timeout: 20000 });
      assert.match(await page.textContent('.msg.final'), /Verified/);
      assert.ok((await page.innerHTML('.msg.final')).includes('<strong>both ends</strong>'), 'markdown bold rendered as DOM');
      await page.click('.msg.final details summary');
      assert.match(await page.textContent('.msg.final details pre'), /i <= b/);
      assert.equal(await page.locator('#files li').first().textContent(), 'lib/math.js');
      assert.match(await page.textContent('#verify'), /pass/);
      // XSS probe: model text containing markup must render as text
      const injected = await page.evaluate(() => { const li = document.createElement('li'); li.textContent = '<img src=x onerror=alert(1)>'; document.body.append(li); return li.innerHTML; });
      assert.match(injected, /&lt;img/);
      assert.match(readFileSync(join(d, 'lib/math.js'), 'utf8'), /i <= b/);
    }
    await page.screenshot({ path: join(out, `final-${scheme}.png`), fullPage: true });
    await page.setViewportSize({ width: 390, height: 800 });
    await page.screenshot({ path: join(out, `mobile-${scheme}.png`) });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true, `no horizontal scroll on phone width (${scheme})`);
    await ctx.close();
  }
  assert.deepEqual(problems, [], `browser problems: ${problems.join(' | ')}`);
  console.log(`UI smoke OK. Screenshots: ${out}`);
} finally {
  await browser.close(); await srv.close(); await m.close();
}
