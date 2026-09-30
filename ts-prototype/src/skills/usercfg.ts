import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { parseFrontmatter } from './frontmatter.ts';
import { clip } from '../util/proc.ts';

export interface UserSkill { name: string; description: string; body: string; path: string }
export interface UserConfig { rules: string; skills: UserSkill[] }

const SKILL_DIRS = ['.fh/skills', '.qwen/skills', '.claude/skills', '.agents/skills'];
const RULE_FILES = ['AGENTS.md', 'FRANKENSTEIN.md', '.fh/rules.md'];

/** Visible, editable user files: AGENTS.md rules and SKILL.md skills. These override internal skills. */
export function loadUserConfig(cwd: string): UserConfig {
  const rules: string[] = [];
  for (const f of RULE_FILES) {
    const p = join(cwd, f);
    if (existsSync(p)) rules.push(`# ${f}\n${readFileSync(p, 'utf8').trim()}`);
  }
  const skills: UserSkill[] = [];
  for (const d of SKILL_DIRS) {
    const base = join(cwd, d);
    if (!existsSync(base)) continue;
    for (const e of readdirSync(base, { withFileTypes: true })) {
      const p = join(base, e.name, 'SKILL.md');
      if (!e.isDirectory() || !existsSync(p)) continue;
      const { meta, body } = parseFrontmatter(readFileSync(p, 'utf8'));
      skills.push({ name: meta.name || e.name, description: meta.description || meta.summary || '', body, path: `${d}/${e.name}/SKILL.md` });
    }
  }
  return { rules: clip(rules.join('\n\n'), 8000), skills };
}
