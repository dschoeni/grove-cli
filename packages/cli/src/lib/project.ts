import * as fs from 'node:fs';
import * as path from 'node:path';
import { execFileSync } from 'node:child_process';
import type {
  ClaudeConfig,
  GroveConfig,
  ProjectContext,
  SandboxConfig,
  WorkspaceConfig,
  WorkspaceRepo,
} from '../types.js';

export class GroveError extends Error {}

export function findRepoRoot(start: string): string {
  try {
    const out = execFileSync('git', ['rev-parse', '--show-toplevel'], {
      cwd: start,
      encoding: 'utf-8',
      stdio: ['ignore', 'pipe', 'pipe'],
    }).trim();
    if (!out) throw new Error('empty output');
    return out;
  } catch {
    throw new GroveError(`Not inside a git repository (started from ${start})`);
  }
}

/** Walk up from `start` looking for a directory containing `.groverc`. */
export function findGrovercAncestor(start: string): string | null {
  let dir = path.resolve(start);
  while (true) {
    if (fs.existsSync(path.join(dir, '.groverc'))) return dir;
    const parent = path.dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

const DEFAULT_SANDBOX: SandboxConfig = {
  enabled: true,
  shareReadOnly: [],
  shareReadWrite: [],
};

const DEFAULT_CLAUDE: ClaudeConfig = {
  command: 'claude',
  extraArgs: ['--permission-mode', 'bypassPermissions'],
};

function defaultConfig(repoRoot: string): GroveConfig {
  return {
    baseBranch: currentBranch(repoRoot),
    sandbox: { ...DEFAULT_SANDBOX },
    postCreateCommands: [],
    claude: { ...DEFAULT_CLAUDE },
  };
}

export function currentBranch(repoRoot: string): string | null {
  try {
    const out = execFileSync('git', ['symbolic-ref', '--short', 'HEAD'], {
      cwd: repoRoot,
      encoding: 'utf-8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim();
    return out || null;
  } catch {
    return null;
  }
}

export function loadProject(start: string = process.cwd()): ProjectContext {
  const ancestor = findGrovercAncestor(start);
  if (ancestor) {
    const raw = fs.readFileSync(path.join(ancestor, '.groverc'), 'utf-8');
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch (err) {
      throw new GroveError(`.groverc is not valid JSON: ${(err as Error).message}`);
    }
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      throw new GroveError('.groverc must be a JSON object');
    }
    const obj = parsed as Record<string, unknown>;
    if (obj.type === 'workspace') {
      return {
        kind: 'workspace',
        workspaceRoot: ancestor,
        config: parseWorkspaceConfig(obj, ancestor),
        configSource: 'groverc',
      };
    }
    return {
      kind: 'single',
      repoRoot: findRepoRoot(ancestor),
      config: mergeSingleConfig(obj, ancestor),
      configSource: 'groverc',
    };
  }

  // No .groverc — fall back to git repo root with defaults.
  const repoRoot = findRepoRoot(start);
  return {
    kind: 'single',
    repoRoot,
    config: defaultConfig(repoRoot),
    configSource: 'defaults',
  };
}

function mergeSingleConfig(obj: Record<string, unknown>, repoRoot: string): GroveConfig {
  const base = defaultConfig(repoRoot);

  if (typeof obj.baseBranch === 'string' && obj.baseBranch.trim()) {
    base.baseBranch = obj.baseBranch.trim();
  }

  applySandbox(obj, base.sandbox);

  if (Array.isArray(obj.postCreateCommands)) {
    base.postCreateCommands = obj.postCreateCommands.filter(isString);
  }

  applyClaude(obj, base.claude);

  return base;
}

function parseWorkspaceConfig(obj: Record<string, unknown>, workspaceRoot: string): WorkspaceConfig {
  const reposRaw = obj.repos;
  if (!Array.isArray(reposRaw) || reposRaw.length === 0) {
    throw new GroveError('workspace .groverc must define a non-empty "repos" array');
  }

  const repos: WorkspaceRepo[] = reposRaw.map((r, idx) => {
    if (!r || typeof r !== 'object' || Array.isArray(r)) {
      throw new GroveError(`repos[${idx}] must be an object`);
    }
    const rec = r as Record<string, unknown>;
    if (typeof rec.path !== 'string' || !rec.path.trim()) {
      throw new GroveError(`repos[${idx}].path must be a non-empty string`);
    }
    const repoPath = rec.path.trim();
    const repoAbs = path.resolve(workspaceRoot, repoPath);
    if (!fs.existsSync(path.join(repoAbs, '.git'))) {
      throw new GroveError(
        `repos[${idx}] "${repoPath}" is not a git repo (no .git at ${repoAbs})`,
      );
    }
    const name =
      typeof rec.name === 'string' && rec.name.trim() ? rec.name.trim() : path.basename(repoPath);
    const baseBranch =
      typeof rec.baseBranch === 'string' && rec.baseBranch.trim() ? rec.baseBranch.trim() : null;
    return { name, path: repoPath, baseBranch };
  });

  const sandbox: SandboxConfig = { ...DEFAULT_SANDBOX };
  applySandbox(obj, sandbox);

  const claude: ClaudeConfig = { ...DEFAULT_CLAUDE };
  applyClaude(obj, claude);

  const postCreateCommands = Array.isArray(obj.postCreateCommands)
    ? obj.postCreateCommands.filter(isString)
    : [];

  return { repos, sandbox, postCreateCommands, claude };
}

function applySandbox(obj: Record<string, unknown>, sandbox: SandboxConfig): void {
  const sandboxRaw = obj.sandbox;
  if (sandboxRaw && typeof sandboxRaw === 'object' && !Array.isArray(sandboxRaw)) {
    const s = sandboxRaw as Record<string, unknown>;
    if (typeof s.enabled === 'boolean') sandbox.enabled = s.enabled;
    if (Array.isArray(s.shareReadOnly)) sandbox.shareReadOnly = s.shareReadOnly.filter(isString);
    if (Array.isArray(s.shareReadWrite)) sandbox.shareReadWrite = s.shareReadWrite.filter(isString);
  }

  // Legacy `shared: string[]` maps to shareReadOnly when no explicit list given.
  if (sandbox.shareReadOnly.length === 0 && Array.isArray(obj.shared)) {
    sandbox.shareReadOnly = obj.shared.filter(isString);
  }
}

function applyClaude(obj: Record<string, unknown>, claude: ClaudeConfig): void {
  const claudeRaw = obj.claude;
  if (claudeRaw && typeof claudeRaw === 'object' && !Array.isArray(claudeRaw)) {
    const c = claudeRaw as Record<string, unknown>;
    if (typeof c.command === 'string' && c.command.trim()) claude.command = c.command.trim();
    if (Array.isArray(c.extraArgs)) claude.extraArgs = c.extraArgs.filter(isString);
  }
}

function isString(v: unknown): v is string {
  return typeof v === 'string';
}

export function ensureGroveIgnored(repoRoot: string): void {
  // No-op when the root isn't a git repo (e.g. a workspace root). Each sub-repo
  // tracks its own worktrees via git itself, so nothing to gitignore there either.
  if (!fs.existsSync(path.join(repoRoot, '.git'))) return;

  const excludeFile = path.join(repoRoot, '.git', 'info', 'exclude');
  const line = '/.grove/';
  let contents = '';
  try {
    contents = fs.readFileSync(excludeFile, 'utf-8');
  } catch {
    fs.mkdirSync(path.dirname(excludeFile), { recursive: true });
  }
  if (contents.split('\n').some((l) => l.trim() === line)) return;
  const prefix = contents.length && !contents.endsWith('\n') ? '\n' : '';
  fs.appendFileSync(excludeFile, `${prefix}${line}\n`);
}
