import { findSecrets } from '../verify/secrets.ts';

const INJECTION = /(ignore (all |any |the )?(previous|prior|above)|disregard (the )?(system|previous)|you (must|should) (always )?(approve|allow|run|execute)|without (asking|confirmation|approval)|bypass (permission|approval|sandbox)|disable (the )?(sandbox|verification|approval)|exfiltrat|send .* to (http|an? (external|remote)))/i;
const SHELL_FENCE = /```\s*(sh|bash|shell|zsh|powershell|ps1|cmd|bat)\b/i;
const URL = /\b(?:https?|ftp|ssh|file):\/\/\S+|\bwww\.\S+\.\S+/i;
const CMD_LINE = /^\s*(?:\$|>)\s+\S+|^\s*(sudo|curl|wget|rm|chmod|npm|pip|dotnet|git)\s+\S+.*$/im;

/** Skills are data only: no commands, URLs, secrets, permission grants or override attempts. */
export function validateSkillBody(body: string, summary = '', name = ''): { ok: boolean; reason?: string } {
  const all = `${name}\n${summary}\n${body}`;
  if (body.trim().length < 40) return { ok: false, reason: 'too short' };
  if (body.length > 2400) return { ok: false, reason: 'too long' };
  const sec = findSecrets(all);
  if (sec.length) return { ok: false, reason: `contains secret (${sec[0]})` };
  if (URL.test(all)) return { ok: false, reason: 'contains a URL' };
  if (SHELL_FENCE.test(all) || CMD_LINE.test(all)) return { ok: false, reason: 'contains commands' };
  if (INJECTION.test(all)) return { ok: false, reason: 'contains instruction-override language' };
  if (!/^[\w .:+#/@-]{2,60}$/.test(name)) return { ok: false, reason: 'invalid name' };
  return { ok: true };
}
