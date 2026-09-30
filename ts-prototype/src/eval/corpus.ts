export interface EvalTask {
  id: string;
  prompt: string;
  /** shell command; exit 0 = task solved (the oracle is never shown to the agent) */
  oracle: string;
  files: Record<string, string>;
  timeoutS?: number;
  tags?: string[];
}

const PKG = JSON.stringify({ name: 'fixture', version: '1.0.0', type: 'commonjs', scripts: { test: 'node --test' } }, null, 2);

/**
 * Small self-contained tasks (no network, no dependencies) so the harness and the model can be validated on a bare VPS.
 * They are smoke-level: the real comparison uses SWE-bench Verified / Terminal-Bench subsets and recorded tasks
 * loaded with --tasks <dir> (see docs/EVAL.md).
 */
export const BUILTIN_TASKS: EvalTask[] = [
  {
    id: 'js-off-by-one', tags: ['bugfix', 'js'],
    prompt: 'The function sumRange(a, b) in lib/math.js should return the inclusive sum of integers from a to b, but the tests fail. Fix the bug.',
    oracle: 'node --test',
    files: {
      'package.json': PKG,
      'lib/math.js': 'function sumRange(a, b) {\n  let total = 0;\n  for (let i = a; i < b; i++) total += i;\n  return total;\n}\nmodule.exports = { sumRange };\n',
      'test/math.test.js': "const test = require('node:test');\nconst assert = require('node:assert');\nconst { sumRange } = require('../lib/math');\ntest('inclusive sum', () => { assert.strictEqual(sumRange(1, 4), 10); assert.strictEqual(sumRange(3, 3), 3); });\n",
    },
  },
  {
    id: 'js-implement-slugify', tags: ['feature', 'js'],
    prompt: 'Implement slugify(text) in lib/slug.js: lowercase, trim, replace runs of non-alphanumeric characters with a single hyphen, and strip leading/trailing hyphens. The tests in test/slug.test.js describe the behavior.',
    oracle: 'node --test',
    files: {
      'package.json': PKG,
      'lib/slug.js': "function slugify(text) {\n  throw new Error('not implemented');\n}\nmodule.exports = { slugify };\n",
      'test/slug.test.js': "const test = require('node:test');\nconst assert = require('node:assert');\nconst { slugify } = require('../lib/slug');\ntest('basic', () => assert.strictEqual(slugify('Hello, World!'), 'hello-world'));\ntest('trim and collapse', () => assert.strictEqual(slugify('  --A  b__c--  '), 'a-b-c'));\ntest('empty', () => assert.strictEqual(slugify('***'), ''));\n",
    },
  },
  {
    id: 'js-async-order', tags: ['bugfix', 'js', 'async'],
    prompt: 'fetchAll(ids, load) in lib/fetch.js must return results in the same order as ids, but callers sometimes get unresolved promises or the wrong order. Fix it without changing its signature.',
    oracle: 'node --test',
    files: {
      'package.json': PKG,
      'lib/fetch.js': 'async function fetchAll(ids, load) {\n  const out = [];\n  ids.forEach(async (id) => {\n    out.push(await load(id));\n  });\n  return out;\n}\nmodule.exports = { fetchAll };\n',
      'test/fetch.test.js': "const test = require('node:test');\nconst assert = require('node:assert');\nconst { fetchAll } = require('../lib/fetch');\nconst load = (id) => new Promise((r) => setTimeout(() => r(id * 10), 30 - id * 5));\ntest('keeps order', async () => assert.deepStrictEqual(await fetchAll([1, 2, 3], load), [10, 20, 30]));\ntest('empty', async () => assert.deepStrictEqual(await fetchAll([], load), []));\n",
    },
  },
  {
    id: 'py-word-count', tags: ['bugfix', 'python'],
    prompt: 'word_count in text.py should count words separated by any whitespace (spaces, tabs, newlines) and return 0 for empty or whitespace-only text. Tests fail; fix it.',
    oracle: 'python3 -m unittest discover -q',
    files: {
      'text.py': 'def word_count(s):\n    return len(s.split(" "))\n',
      'test_text.py': "import unittest\nfrom text import word_count\n\nclass T(unittest.TestCase):\n    def test_basic(self):\n        self.assertEqual(word_count('a b  c'), 3)\n    def test_ws(self):\n        self.assertEqual(word_count('a\\tb\\nc'), 3)\n    def test_empty(self):\n        self.assertEqual(word_count('   '), 0)\n\nif __name__ == '__main__':\n    unittest.main()\n",
    },
  },
  {
    id: 'js-two-modules', tags: ['feature', 'js', 'multi-file'],
    prompt: 'Two independent changes are needed. (1) In server/validate.js implement isEmail(s) (simple check: one @, non-empty local part, domain containing a dot, no spaces). (2) In client/format.js implement formatCents(n) that formats integer cents as dollars with two decimals and thousands separators, e.g. 123456 -> "$1,234.56". Tests exist for both.',
    oracle: 'node --test',
    files: {
      'package.json': PKG,
      'server/validate.js': "function isEmail(s) {\n  throw new Error('todo');\n}\nmodule.exports = { isEmail };\n",
      'client/format.js': "function formatCents(n) {\n  throw new Error('todo');\n}\nmodule.exports = { formatCents };\n",
      'test/server.test.js': "const test = require('node:test');\nconst assert = require('node:assert');\nconst { isEmail } = require('../server/validate');\ntest('emails', () => { assert.ok(isEmail('a@b.co')); assert.ok(!isEmail('a@b')); assert.ok(!isEmail('@b.co')); assert.ok(!isEmail('a b@c.de')); assert.ok(!isEmail('a@@b.co')); });\n",
      'test/client.test.js': "const test = require('node:test');\nconst assert = require('node:assert');\nconst { formatCents } = require('../client/format');\ntest('money', () => { assert.strictEqual(formatCents(123456), '$1,234.56'); assert.strictEqual(formatCents(5), '$0.05'); assert.strictEqual(formatCents(0), '$0.00'); });\n",
    },
  },
];
