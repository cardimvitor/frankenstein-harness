import { platform } from 'node:os';
import { spawnSync } from 'node:child_process';

export type ShellWrap = (c: { file: string; args: string[] }) => { file: string; args: string[] };

const have = (bin: string) => spawnSync(platform() === 'win32' ? 'where' : 'which', [bin], { stdio: 'ignore' }).status === 0;

export function bwrapWrap(cwd: string, network: boolean): ShellWrap {
  return (c) => ({
    file: 'bwrap',
    args: ['--ro-bind', '/', '/', '--bind', cwd, cwd, '--dev', '/dev', '--proc', '/proc', '--tmpfs', '/tmp', '--unshare-pid', '--die-with-parent', ...(network ? [] : ['--unshare-net']), '--chdir', cwd, c.file, ...c.args],
  });
}

export function seatbeltProfile(cwd: string, network: boolean): string {
  return `(version 1)(allow default)(deny file-write*)(allow file-write* (subpath ${JSON.stringify(cwd)}) (subpath "/private/tmp") (subpath "/private/var/folders") (literal "/dev/null"))${network ? '' : '(deny network*)'}`;
}

/** OS-level write confinement for shell commands. Linux: bubblewrap; macOS: Seatbelt. Windows: not implemented yet (see docs). */
export function sandboxFor(cwd: string, opts: { network?: boolean } = {}): { wrap?: ShellWrap; backend: string } {
  const network = opts.network ?? true;
  const p = platform();
  if (p === 'linux' && have('bwrap')) return { wrap: bwrapWrap(cwd, network), backend: 'bwrap' };
  if (p === 'darwin' && have('sandbox-exec')) {
    const prof = seatbeltProfile(cwd, network);
    return { wrap: (c) => ({ file: 'sandbox-exec', args: ['-p', prof, c.file, ...c.args] }), backend: 'seatbelt' };
  }
  return { backend: 'none' };
}
