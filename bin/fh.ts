#!/usr/bin/env -S node --disable-warning=ExperimentalWarning
// node:sqlite is still flagged experimental; silence only that warning so CLI output stays clean.
process.removeAllListeners('warning');
process.on('warning', (w) => { if (w.name !== 'ExperimentalWarning') console.error(String(w)); });

const { main } = await import('../src/ui/cli.ts');
main(process.argv.slice(2)).then(
  (code) => process.exit(code),
  (e) => { console.error(String(e?.stack ?? e)); process.exit(1); },
);
