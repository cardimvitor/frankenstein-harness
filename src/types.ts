export type Role = 'system' | 'user' | 'assistant' | 'tool';

export interface ToolCall {
  id: string;
  name: string;
  /** Raw argument string as received from the model. */
  rawArgs: string;
  /** Parsed arguments (after repair when needed). */
  args: Record<string, unknown>;
  /** 'ok' parsed as-is, 'repaired' needed repair, 'malformed' unusable. */
  parse: 'ok' | 'repaired' | 'malformed';
  /** True when the call was recovered from message content instead of tool_calls. */
  fromContent?: boolean;
}

export interface Message {
  role: Role;
  content: string | null;
  tool_calls?: { id: string; type: 'function'; function: { name: string; arguments: string } }[];
  tool_call_id?: string;
  name?: string;
}

export interface ToolSpec {
  name: string;
  description: string;
  parameters: Record<string, unknown>;
}

export interface Usage {
  prompt_tokens: number;
  completion_tokens: number;
  cached_tokens?: number;
}

export interface LlmResult {
  content: string;
  reasoning: string;
  toolCalls: ToolCall[];
  finish: string;
  usage: Usage;
  ttftMs: number;
  totalMs: number;
  /** tool-call parse stats for this response */
  repaired: number;
  malformed: number;
  thinkLeak: boolean;
}

export type ThinkingLevel = 'off' | 'low' | 'high';

export interface ChatOptions {
  messages: Message[];
  tools?: ToolSpec[];
  thinking?: ThinkingLevel;
  maxTokens?: number;
  temperature?: number;
  signal?: AbortSignal;
  /** JSON schema for structured output (response_format json_schema). */
  jsonSchema?: Record<string, unknown>;
  onContent?: (delta: string) => void;
  onReasoning?: (delta: string) => void;
  stop?: string[];
}

export type Mode = 'plan' | 'ask' | 'auto-edit' | 'yolo';
