// Drives the real web UI (served by the Rust binary) in headless Chromium against the mock vLLM (no model needed):
//   cargo build --release
//   PLAYWRIGHT_MODULE=$(npm root -g)/playwright node scripts/ui-smoke.mjs [outDir]
// Checks: login with the one-time code, guided run with the plan-approval card, streamed events, verified result card,
// diff view, no CSP violations or console errors, light and dark screenshots, no horizontal scroll at phone width.
import { createRequire } from 'node:module';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execSync, spawn } from 'node:child_process';
import assert from 'node:assert/strict';

const { chromium } = createRequire(import.meta.url)(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
const FH = process.env.FH_BIN ?? './target/release/fh';
const out = process.argv[2] ?? join(tmpdir(), 'fh-ui');
mkdirSync(out, { recursive: true });

const repo = mkdtempSync(join(tmpdir(), 'fh-ui-repo-'));
writeFileSync(join(repo, 'mathx.py'), 'def sum_range(a, b):\n    total = 0\n    for i in range(a, b):\n        total += i\n    return total\n');
writeFileSync(join(repo, 'test_mathx.py'), "import unittest\nfrom mathx import sum_range\n\nclass T(unittest.TestCase):\n    def test_inclusive(self):\n        self.assertEqual(sum_range(1, 4), 10)\n        self.assertEqual(sum_range(3, 3), 3)\n");
execSync('git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -qm b', { cwd: repo });

const mockPort = 20000 + Math.floor(Math.random() * 10000), uiPort = mockPort + 1;
const env = { ...process.env, FH_ENDPOINT: `http://127.0.0.1:${mockPort}/v1`, FH_MODEL: 'mock-qwen', FH_AUTH_SCHEME: 'none', FH_HOME: mkdtempSync(join(tmpdir(), 'fh-home-')) };
const mock = spawn(FH, ['mock-server', '--port', String(mockPort)], { env, stdio: 'ignore' });
const ui = spawn(FH, ['serve', '--port', String(uiPort), '--cwd', repo], { env });
let log = '';
ui.stdout.on('data', (d) => (log += d));
for (let i = 0; i < 100 && !/One-time access code: (\S+)/.test(log); i++) await new Promise((r) => setTimeout(r, 100));
const code = log.match(/One-time access code: (\S+)/)?.[1];
assert.ok(code, `no access code in server output: ${log}`);
const url = `http://127.0.0.1:${uiPort}/`;

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
    await page.goto(url);
    await page.waitForSelector('#login:not([hidden])');
    await page.fill('#code', 'wrong-code');
    await page.click('#login-form button');
    await page.waitForSelector('#login-error:not([hidden])');
    const currentCode = scheme === 'light' ? code : (await (await fetch(url + 'api/state')).status, null);
    if (scheme === 'light') {
      await page.fill('#code', currentCode);
      await page.click('#login-form button');
      await page.waitForSelector('#app:not([hidden])');
      assert.equal(await page.textContent('#model'), 'mock-qwen');
      await page.fill('#task', 'fix sum_range so both ends are included');
      await page.press('#task', 'Enter');
      await page.waitForSelector('.ask');
      assert.match(await page.textContent('.ask'), /fix loop bound/);
      await page.screenshot({ path: join(out, 'plan-approval-light.png') });
      await page.click('.ask button.primary');
      await page.waitForSelector('.msg.final.pass', { timeout: 30000 });
      assert.match(await page.textContent('.msg.final'), /Verified/);
      await page.click('.msg.final details summary');
      assert.match(await page.textContent('.msg.final .diff'), /b \+ 1/);
      assert.equal(await page.locator('#files li').first().textContent(), 'mathx.py');
      assert.match(await page.textContent('#verify'), /pass/);
      // per-file, highlighted diff
      assert.match(await page.textContent('.msg.final .diff-file summary'), /mathx\.py.*\+1 −1/);
      assert.ok((await page.locator('.msg.final .dl.add').count()) >= 1 && (await page.locator('.msg.final .dl.del').count()) >= 1);
      assert.ok((await page.locator('.msg.final .dl .tok-kw').count()) >= 1, 'diff lines are syntax highlighted');
      // file viewer from the workspace tree, with the last task's changes on top
      await page.click('#tree button.file:has-text("mathx.py")');
      await page.waitForSelector('dialog#viewer[open]');
      assert.ok((await page.locator('#viewer .code .cl').count()) >= 5);
      assert.ok((await page.locator('#viewer .code .tok-kw').count()) >= 1);
      assert.equal(await page.locator('#viewer .viewer-changes').count(), 1);
      await page.screenshot({ path: join(out, 'file-viewer-light.png') });
      await page.click('#viewer-close');
      await page.waitForSelector('dialog#viewer:not([open])', { state: 'attached' });
      // sessions panel lists the finished session
      await page.waitForFunction(() => /pass/.test(document.getElementById('sessions').textContent));
      // search: file names and lines open the viewer at the line; past tasks are listed
      await page.fill('#search-q', 'sum_range');
      await page.click('#search-form button');
      await page.waitForSelector('#search-results .result-btn');
      assert.ok((await page.locator('#search-results .result-btn').count()) >= 2);
      await page.click('#search-results .result-btn:has-text("mathx.py:1")');
      await page.waitForSelector('dialog#viewer[open]');
      assert.ok((await page.locator('#viewer .cl.hit').count()) === 1, 'the matching line is highlighted');
      await page.click('#viewer-close');
      await page.waitForSelector('dialog#viewer:not([open])', { state: 'attached' });
      await page.selectOption('#search-kind', 'sessions');
      await page.fill('#search-q', 'sum_range');
      await page.click('#search-form button');
      await page.waitForFunction(() => /pass/.test(document.getElementById('search-results').textContent));
      await page.selectOption('#search-kind', 'files');
      // markdown coverage and markup safety, through the real module
      const md = await page.evaluate(async () => {
        const { renderMarkdown } = await import('/render.js');
        const n = renderMarkdown('# Title\n\n| a | b |\n|---|---|\n| 1 | **2** |\n\n1. one\n2. two\n\n[click](javascript:alert(1)) ![x](http://evil/x.png) <img src=x onerror=alert(1)> <script>alert(2)</script>\n\n> quoted');
        return { th: n.querySelectorAll('th').length, td: n.querySelectorAll('td').length, strongInCell: !!n.querySelector('td strong'), ol: n.querySelectorAll('ol li').length, h: !!n.querySelector('h3'), quote: !!n.querySelector('blockquote'), anchors: n.querySelectorAll('a,img,script,iframe').length, text: n.textContent };
      });
      assert.deepEqual([md.th, md.td, md.strongInCell, md.ol, md.h, md.quote, md.anchors], [2, 2, true, 2, true, true, 0]);
      assert.match(md.text, /click \(javascript:alert\(1\)\)/, 'links are shown as text, never followed');
      assert.match(md.text, /<script>alert\(2\)<\/script>/, 'raw HTML stays literal text');
      const tok = await page.evaluate(async () => {
        const { highlightLine } = await import('/render.js');
        const d = document.createElement('div'); d.append(highlightLine('py', 'def f(x): return "a#b" + 42  # note'));
        return [...d.querySelectorAll('span')].map((s) => `${s.className}:${s.textContent}`);
      });
      assert.deepEqual(tok, ['tok-kw:def', 'tok-kw:return', 'tok-str:"a#b"', 'tok-num:42', 'tok-com:# note']);
      assert.match(readFileSync(join(repo, 'mathx.py'), 'utf8'), /b \+ 1/);
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
  await browser.close(); ui.kill(); mock.kill();
}
