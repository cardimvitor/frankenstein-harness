import type { Tool } from './types.ts';
import { bash, edit, grep, listFiles, readFile, writeNew } from './fs.ts';

export const ALL_TOOLS: Tool[] = [readFile, listFiles, grep, edit, writeNew, bash];
export const toolMap = (tools = ALL_TOOLS) => new Map(tools.map((t) => [t.spec.name, t]));
export * from './types.ts';
