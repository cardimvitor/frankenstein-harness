/// Small self-contained tasks (no network, no dependencies) so the harness and the model can be validated on a bare VPS.
/// They are smoke-level: the real comparison uses SWE-bench Verified / Terminal-Bench subsets and recorded tasks
/// loaded with --tasks <dir> (see docs/EVAL.md).
#[derive(Clone, Debug)]
pub struct EvalTask {
    pub id: String,
    pub prompt: String,
    /// shell command; exit 0 = task solved (the oracle is never shown to the agent)
    pub oracle: String,
    pub files: Vec<(String, String)>,
    /// external repo directory to copy instead of `files`
    pub repo_dir: Option<std::path::PathBuf>,
    pub timeout_s: u64,
    pub tags: Vec<String>,
    /// binary that must exist for the oracle to run (task is skipped otherwise)
    pub requires: Option<String>,
}

const PKG: &str = "{\n  \"name\": \"fixture\",\n  \"version\": \"1.0.0\",\n  \"type\": \"commonjs\",\n  \"scripts\": { \"test\": \"node --test\" }\n}\n";

fn f(p: &str, c: &str) -> (String, String) {
    (p.to_string(), c.to_string())
}

fn task(id: &str, tags: &[&str], prompt: &str, oracle: &str, files: Vec<(String, String)>) -> EvalTask {
    EvalTask { id: id.into(), prompt: prompt.into(), oracle: oracle.into(), files, repo_dir: None, timeout_s: 900, tags: tags.iter().map(|s| s.to_string()).collect(), requires: None }
}

fn js_tasks() -> Vec<EvalTask> {
    let mut v = vec![
        task(
            "js-off-by-one",
            &["bugfix", "js"],
            "The function sumRange(a, b) in lib/math.js should return the inclusive sum of integers from a to b, but the tests fail. Fix the bug.",
            "node --test",
            vec![
                f("package.json", PKG),
                f("lib/math.js", "function sumRange(a, b) {\n  let total = 0;\n  for (let i = a; i < b; i++) total += i;\n  return total;\n}\nmodule.exports = { sumRange };\n"),
                f("test/math.test.js", "const test = require('node:test');\nconst assert = require('node:assert');\nconst { sumRange } = require('../lib/math');\ntest('inclusive sum', () => { assert.strictEqual(sumRange(1, 4), 10); assert.strictEqual(sumRange(3, 3), 3); });\n"),
            ],
        ),
        task(
            "js-implement-slugify",
            &["feature", "js"],
            "Implement slugify(text) in lib/slug.js: lowercase, trim, replace runs of non-alphanumeric characters with a single hyphen, and strip leading/trailing hyphens. The tests in test/slug.test.js describe the behavior.",
            "node --test",
            vec![
                f("package.json", PKG),
                f("lib/slug.js", "function slugify(text) {\n  throw new Error('not implemented');\n}\nmodule.exports = { slugify };\n"),
                f("test/slug.test.js", "const test = require('node:test');\nconst assert = require('node:assert');\nconst { slugify } = require('../lib/slug');\ntest('basic', () => assert.strictEqual(slugify('Hello, World!'), 'hello-world'));\ntest('trim and collapse', () => assert.strictEqual(slugify('  --A  b__c--  '), 'a-b-c'));\ntest('empty', () => assert.strictEqual(slugify('***'), ''));\n"),
            ],
        ),
        task(
            "js-async-order",
            &["bugfix", "js", "async"],
            "fetchAll(ids, load) in lib/fetch.js must return results in the same order as ids, but callers sometimes get unresolved promises or the wrong order. Fix it without changing its signature.",
            "node --test",
            vec![
                f("package.json", PKG),
                f("lib/fetch.js", "async function fetchAll(ids, load) {\n  const out = [];\n  ids.forEach(async (id) => {\n    out.push(await load(id));\n  });\n  return out;\n}\nmodule.exports = { fetchAll };\n"),
                f("test/fetch.test.js", "const test = require('node:test');\nconst assert = require('node:assert');\nconst { fetchAll } = require('../lib/fetch');\nconst load = (id) => new Promise((r) => setTimeout(() => r(id * 10), 30 - id * 5));\ntest('keeps order', async () => assert.deepStrictEqual(await fetchAll([1, 2, 3], load), [10, 20, 30]));\ntest('empty', async () => assert.deepStrictEqual(await fetchAll([], load), []));\n"),
            ],
        ),
        task(
            "py-word-count",
            &["bugfix", "python"],
            "word_count in text.py should count words separated by any whitespace (spaces, tabs, newlines) and return 0 for empty or whitespace-only text. Tests fail; fix it.",
            "python3 -m unittest discover -q",
            vec![
                f("text.py", "def word_count(s):\n    return len(s.split(\" \"))\n"),
                f("test_text.py", "import unittest\nfrom text import word_count\n\nclass T(unittest.TestCase):\n    def test_basic(self):\n        self.assertEqual(word_count('a b  c'), 3)\n    def test_ws(self):\n        self.assertEqual(word_count('a\\tb\\nc'), 3)\n    def test_empty(self):\n        self.assertEqual(word_count('   '), 0)\n\nif __name__ == '__main__':\n    unittest.main()\n"),
            ],
        ),
        task(
            "js-two-modules",
            &["feature", "js", "multi-file"],
            "Two independent changes are needed. (1) In server/validate.js implement isEmail(s) (simple check: one @, non-empty local part, domain containing a dot, no spaces). (2) In client/format.js implement formatCents(n) that formats integer cents as dollars with two decimals and thousands separators, e.g. 123456 -> \"$1,234.56\". Tests exist for both.",
            "node --test",
            vec![
                f("package.json", PKG),
                f("server/validate.js", "function isEmail(s) {\n  throw new Error('todo');\n}\nmodule.exports = { isEmail };\n"),
                f("client/format.js", "function formatCents(n) {\n  throw new Error('todo');\n}\nmodule.exports = { formatCents };\n"),
                f("test/server.test.js", "const test = require('node:test');\nconst assert = require('node:assert');\nconst { isEmail } = require('../server/validate');\ntest('emails', () => { assert.ok(isEmail('a@b.co')); assert.ok(!isEmail('a@b')); assert.ok(!isEmail('@b.co')); assert.ok(!isEmail('a b@c.de')); assert.ok(!isEmail('a@@b.co')); });\n"),
                f("test/client.test.js", "const test = require('node:test');\nconst assert = require('node:assert');\nconst { formatCents } = require('../client/format');\ntest('money', () => { assert.strictEqual(formatCents(123456), '$1,234.56'); assert.strictEqual(formatCents(5), '$0.05'); assert.strictEqual(formatCents(0), '$0.00'); });\n"),
            ],
        ),
    ];
    for t in &mut v {
        t.requires = Some("node".into());
    }
    v
}

fn py_tasks() -> Vec<EvalTask> {
    let mut v = vec![
        task(
            "py-off-by-one",
            &["bugfix", "python"],
            "The function sum_range(a, b) in mathx.py should return the inclusive sum of integers from a to b, but the tests fail. Fix the bug.",
            "python3 -m unittest discover -q",
            vec![
                f("mathx.py", "def sum_range(a, b):\n    total = 0\n    for i in range(a, b):\n        total += i\n    return total\n"),
                f("test_mathx.py", "import unittest\nfrom mathx import sum_range\n\nclass T(unittest.TestCase):\n    def test_inclusive(self):\n        self.assertEqual(sum_range(1, 4), 10)\n        self.assertEqual(sum_range(3, 3), 3)\n"),
            ],
        ),
        task(
            "py-implement-slugify",
            &["feature", "python"],
            "Implement slugify(text) in slug.py: lowercase, trim, replace runs of non-alphanumeric characters with a single hyphen, and strip leading/trailing hyphens. The tests in test_slug.py describe the behavior.",
            "python3 -m unittest discover -q",
            vec![
                f("slug.py", "def slugify(text):\n    raise NotImplementedError\n"),
                f("test_slug.py", "import unittest\nfrom slug import slugify\n\nclass T(unittest.TestCase):\n    def test_basic(self):\n        self.assertEqual(slugify('Hello, World!'), 'hello-world')\n    def test_trim_collapse(self):\n        self.assertEqual(slugify('  --A  b__c--  '), 'a-b-c')\n    def test_empty(self):\n        self.assertEqual(slugify('***'), '')\n"),
            ],
        ),
        task(
            "py-dedupe-order",
            &["bugfix", "python"],
            "dedupe(items) in listutil.py must remove duplicates but keep the first occurrence of each item in its original order. It currently loses the order. Fix it.",
            "python3 -m unittest discover -q",
            vec![
                f("listutil.py", "def dedupe(items):\n    return list(set(items))\n"),
                f("test_listutil.py", "import unittest\nfrom listutil import dedupe\n\nclass T(unittest.TestCase):\n    def test_order(self):\n        self.assertEqual(dedupe([3, 1, 3, 2, 1]), [3, 1, 2])\n    def test_empty(self):\n        self.assertEqual(dedupe([]), [])\n    def test_strings(self):\n        self.assertEqual(dedupe(['b', 'a', 'b']), ['b', 'a'])\n"),
            ],
        ),
        task(
            "py-word-count",
            &["bugfix", "python"],
            "word_count in text.py should count words separated by any whitespace (spaces, tabs, newlines) and return 0 for empty or whitespace-only text. Tests fail; fix it.",
            "python3 -m unittest discover -q",
            vec![
                f("text.py", "def word_count(s):\n    return len(s.split(\" \"))\n"),
                f("test_text.py", "import unittest\nfrom text import word_count\n\nclass T(unittest.TestCase):\n    def test_basic(self):\n        self.assertEqual(word_count('a b  c'), 3)\n    def test_ws(self):\n        self.assertEqual(word_count('a\\tb\\nc'), 3)\n    def test_empty(self):\n        self.assertEqual(word_count('   '), 0)\n"),
            ],
        ),
        task(
            "py-two-modules",
            &["feature", "python", "multi-file"],
            "Two independent changes are needed. (1) In server/validate.py implement is_email(s) (simple check: exactly one @, non-empty local part, domain containing a dot, no spaces). (2) In client/format.py implement format_cents(n) that formats integer cents as dollars with two decimals and thousands separators, e.g. 123456 -> \"$1,234.56\". Tests exist for both.",
            "python3 -m unittest discover -q",
            vec![
                f("server/__init__.py", ""),
                f("client/__init__.py", ""),
                f("server/validate.py", "def is_email(s):\n    raise NotImplementedError\n"),
                f("client/format.py", "def format_cents(n):\n    raise NotImplementedError\n"),
                f("test_server.py", "import unittest\nfrom server.validate import is_email\n\nclass T(unittest.TestCase):\n    def test_emails(self):\n        self.assertTrue(is_email('a@b.co'))\n        self.assertFalse(is_email('a@b'))\n        self.assertFalse(is_email('@b.co'))\n        self.assertFalse(is_email('a b@c.de'))\n        self.assertFalse(is_email('a@@b.co'))\n"),
                f("test_client.py", "import unittest\nfrom client.format import format_cents\n\nclass T(unittest.TestCase):\n    def test_money(self):\n        self.assertEqual(format_cents(123456), '$1,234.56')\n        self.assertEqual(format_cents(5), '$0.05')\n        self.assertEqual(format_cents(0), '$0.00')\n"),
            ],
        ),
    ];
    for t in &mut v {
        t.requires = Some("python3".into());
    }
    v
}

fn have(bin: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    std::process::Command::new(finder).arg(bin).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// Python tasks first (python3 exists on nearly every VPS), JavaScript tasks when `node` is installed.
pub fn builtin_tasks() -> Vec<EvalTask> {
    py_tasks().into_iter().chain(js_tasks()).filter(|t| t.requires.as_deref().map(have).unwrap_or(true)).collect()
}
