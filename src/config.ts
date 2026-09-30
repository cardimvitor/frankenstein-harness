import { readFileSync, existsSync } from 'node:fs';
import { homedir, platform } from 'node:os';
import { join } from 'node:path';

export interface Config {
  endpoint: string;
  model: string;
  /** Auth scheme: bearer (default), header (custom header name), none. */
  authScheme: 'bearer' | 'header' | 'none';
  authHeader: string;
  /** Name of the env var holding the secret. The secret itself is never stored. */
  apiKeyEnv: string;
  extraHeaders: Record<string, string>;
  clientId: string;
  /** vLLM /metrics URL (defaults to endpoint origin + /metrics). */
  metricsUrl: string;
  contextWindow: number;
  maxOutputTokens: number;
  maxSteps: number;
  requestTimeoutMs: number;
  idleTimeoutMs: number;
  retries: number;
  maxConcurrency: number;
  verifyRoundsNormal: number;
  verifyRoundsAuto: number;
  sampling: { thinking: Sampling; instant: Sampling };
}

export interface Sampling { temperature: number; top_p: number; top_k: number; presence_penalty?: number }

export function dataDir(): string {
  if (process.env.FH_HOME) return process.env.FH_HOME;
  const p = platform();
  if (p === 'win32') return join(process.env.LOCALAPPDATA ?? join(homedir(), 'AppData', 'Local'), 'frankenstein-harness');
  if (p === 'darwin') return join(homedir(), 'Library', 'Application Support', 'frankenstein-harness');
  return join(process.env.XDG_DATA_HOME ?? join(homedir(), '.local', 'share'), 'frankenstein-harness');
}

export function configDir(): string {
  if (process.env.FH_CONFIG_HOME) return process.env.FH_CONFIG_HOME;
  const p = platform();
  if (p === 'win32') return join(process.env.APPDATA ?? join(homedir(), 'AppData', 'Roaming'), 'frankenstein-harness');
  if (p === 'darwin') return join(homedir(), 'Library', 'Application Support', 'frankenstein-harness');
  return join(process.env.XDG_CONFIG_HOME ?? join(homedir(), '.config'), 'frankenstein-harness');
}

const DEFAULTS: Config = {
  endpoint: 'http://127.0.0.1:8000/v1',
  model: 'qwen',
  authScheme: 'bearer',
  authHeader: 'Authorization',
  apiKeyEnv: 'FH_API_KEY',
  extraHeaders: {},
  clientId: 'frankenstein-harness',
  metricsUrl: '',
  contextWindow: 131072,
  maxOutputTokens: 8192,
  maxSteps: 40,
  requestTimeoutMs: 600_000,
  idleTimeoutMs: 120_000,
  retries: 3,
  maxConcurrency: 3,
  verifyRoundsNormal: 2,
  verifyRoundsAuto: 5,
  // Qwen3-family recommended defaults; re-tune with `fh validate-vllm` on the real model.
  sampling: {
    thinking: { temperature: 0.6, top_p: 0.95, top_k: 20 },
    instant: { temperature: 0.7, top_p: 0.8, top_k: 20 },
  },
};

function readJson(path: string): Partial<Config> {
  try {
    return existsSync(path) ? (JSON.parse(readFileSync(path, 'utf8')) as Partial<Config>) : {};
  } catch (e) {
    throw new Error(`Invalid config ${path}: ${(e as Error).message}`);
  }
}

/** Load config: defaults < user config file < project .fh/config.json < env. */
export function loadConfig(cwd = process.cwd(), env = process.env): Config {
  const user = readJson(join(configDir(), 'config.json'));
  const proj = readJson(join(cwd, '.fh', 'config.json'));
  const cfg: Config = { ...DEFAULTS, ...user, ...proj };
  cfg.sampling = { ...DEFAULTS.sampling, ...(user.sampling ?? {}), ...(proj.sampling ?? {}) };
  if (env.FH_ENDPOINT) cfg.endpoint = env.FH_ENDPOINT;
  if (env.FH_MODEL) cfg.model = env.FH_MODEL;
  if (env.FH_AUTH_SCHEME) cfg.authScheme = env.FH_AUTH_SCHEME as Config['authScheme'];
  if (env.FH_API_KEY_ENV) cfg.apiKeyEnv = env.FH_API_KEY_ENV;
  if (env.FH_METRICS_URL) cfg.metricsUrl = env.FH_METRICS_URL;
  if (env.FH_CONTEXT_WINDOW) cfg.contextWindow = Number(env.FH_CONTEXT_WINDOW);
  cfg.endpoint = cfg.endpoint.replace(/\/+$/, '');
  if (!cfg.metricsUrl) cfg.metricsUrl = new URL(cfg.endpoint).origin + '/metrics';
  return cfg;
}

export function authHeaders(cfg: Config, env = process.env): Record<string, string> {
  const h: Record<string, string> = { 'x-client-id': cfg.clientId, ...cfg.extraHeaders };
  const secret = env[cfg.apiKeyEnv];
  if (cfg.authScheme === 'bearer' && secret) h['Authorization'] = `Bearer ${secret}`;
  else if (cfg.authScheme === 'header' && secret) h[cfg.authHeader] = secret;
  return h;
}

/** Redact secrets from any text destined for logs or the UI. */
export function redact(text: string, env = process.env): string {
  let out = text;
  for (const [k, v] of Object.entries(env)) {
    if (v && v.length >= 8 && /key|token|secret|password|auth/i.test(k)) out = out.split(v).join('«redacted»');
  }
  return out;
}
