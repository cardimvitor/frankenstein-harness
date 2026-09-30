#!/usr/bin/env -S node --disable-warning=ExperimentalWarning
import { main } from '../src/ui/cli.ts';

main(process.argv.slice(2)).then(
  (code) => process.exit(code),
  (e) => { console.error(String(e?.stack ?? e)); process.exit(1); },
);
