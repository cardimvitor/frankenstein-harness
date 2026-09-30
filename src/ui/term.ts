import { createInterface } from 'node:readline';
import type { IO } from '../engine.ts';
import { redact } from '../config.ts';

const tty = process.stdout.isTTY && !process.env.NO_COLOR;
export const c = {
  dim: (s: string) => (tty ? `\x1b[2m${s}\x1b[0m` : s),
  bold: (s: string) => (tty ? `\x1b[1m${s}\x1b[0m` : s),
  green: (s: string) => (tty ? `\x1b[32m${s}\x1b[0m` : s),
  red: (s: string) => (tty ? `\x1b[31m${s}\x1b[0m` : s),
  yellow: (s: string) => (tty ? `\x1b[33m${s}\x1b[0m` : s),
  cyan: (s: string) => (tty ? `\x1b[36m${s}\x1b[0m` : s),
};

export function ask(prompt: string): Promise<string> {
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  return new Promise((res) => rl.question(prompt, (a) => { rl.close(); res(a); }));
}

/** Single keypress when attached to a TTY; falls back to a line prompt. */
export function askKey(prompt: string, keys: string): Promise<string> {
  if (!process.stdin.isTTY) return ask(prompt).then((a) => a.trim().toLowerCase()[0] ?? '');
  process.stdout.write(prompt);
  return new Promise((res) => {
    process.stdin.setRawMode(true);
    process.stdin.resume();
    const on = (b: Buffer) => {
      const k = b.toString();
      if (k === '\x03') { process.stdin.setRawMode(false); process.exit(130); }
      const ch = k === '\r' || k === '\n' ? 'enter' : k.toLowerCase();
      if (ch === 'enter' || keys.includes(ch)) {
        process.stdin.setRawMode(false); process.stdin.pause(); process.stdin.off('data', on);
        process.stdout.write(`${ch === 'enter' ? '' : ch}\n`);
        res(ch);
      }
    };
    process.stdin.on('data', on);
  });
}

const short = (o: Record<string, unknown>) => redact(JSON.stringify(o)).slice(0, 110);

export function terminalIO(opts: { showReasoning?: boolean; assumeYes?: boolean } = {}): IO {
  let inText = false;
  const endText = () => { if (inText) { process.stdout.write('\n'); inText = false; } };
  return {
    notice(kind, msg) {
      endText();
      const tag = { phase: c.cyan('▸'), skill: c.yellow('✦'), verify: c.green('✓'), warn: c.red('!'), info: c.dim('·') }[kind];
      console.log(`${tag} ${kind === 'info' ? c.dim(msg) : msg}`);
    },
    progress(d) { process.stdout.write(c.dim(d)); inText = true; },
    reasoning(d) { if (opts.showReasoning) { process.stdout.write(c.dim(d)); inText = true; } },
    toolStart(name, args) { endText(); process.stdout.write(c.dim(`  ${name} ${short(args)}`)); },
    toolEnd(_n, ok, _o, ms) { process.stdout.write(` ${ok ? c.green('ok') : c.red('fail')} ${c.dim(`${ms}ms`)}\n`); },
    async askQuestions(qs) {
      endText();
      const out: string[] = [];
      console.log(c.bold('A few questions before I start (Enter to let me decide):'));
      for (const [i, q] of qs.entries()) out.push((await ask(`${i + 1}. ${q}\n   > `)).trim() || '(use your best judgement)');
      return out;
    },
    async approvePlan(plan, trivial) {
      endText();
      if (opts.assumeYes) { console.log(c.bold('Plan:') + '\n' + plan); return { ok: true }; }
      console.log(c.bold(trivial ? 'Plan (small change):' : 'Plan:') + '\n' + plan + '\n');
      const k = await askKey(trivial ? 'Enter = go, n = cancel: ' : 'Enter = go, n = cancel, e = give feedback: ', 'ne');
      if (k === 'n') return { ok: false };
      if (k === 'e') return { ok: false, feedback: (await ask('Feedback: ')).trim() || undefined };
      return { ok: true };
    },
    async confirm(tool, args) {
      endText();
      if (opts.assumeYes) return true;
      console.log(c.yellow(`\n${tool.spec.name} wants to run: ${short(args)}`));
      const k = await askKey('Allow? y = once, n = deny: ', 'yn');
      return k === 'y';
    },
    async offerReuse(offers) {
      endText();
      if (opts.assumeYes) return [];
      const picks: { fromProject: string; ids: string[] }[] = [];
      for (const o of offers) {
        console.log(c.bold(`\n"${o.fromLabel}" has ${o.skills.length} skill(s) for a similar stack. Reuse them here?`));
        o.skills.forEach((s, i) => console.log(`  ${i + 1}. ${s.name} — ${s.summary}`));
        const a = (await ask('  a = reuse all, numbers (e.g. 1,3) = pick, Enter = no: ')).trim().toLowerCase();
        if (a === 'a') picks.push({ fromProject: o.fromProject, ids: o.skills.map((s) => s.id) });
        else if (a) picks.push({ fromProject: o.fromProject, ids: a.split(/[ ,]+/).map((n) => o.skills[Number(n) - 1]?.id).filter(Boolean) });
      }
      return picks;
    },
  };
}
