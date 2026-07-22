import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import type { ClaudeConfig } from '../types.js';

export interface WorktreeContext {
  /** The worktree Claude is launched in. */
  worktreePath: string;
  /** The repo root (single) or workspace root (workspace). */
  rootDir: string;
  /** Branch slug, e.g. feat/auth-flow. */
  branch: string;
  /** True in workspace mode (one nested worktree per repo). */
  workspace: boolean;
}

export interface ClaudeLaunchInput {
  claude: ClaudeConfig;
  /** Pass-through args from `grove new foo -- ...`. */
  passthrough: string[];
  /** When true, append `--continue` to resume the most recent session. */
  resume: boolean;
  /** When set, append a system prompt pinning Claude to the worktree. */
  worktree?: WorktreeContext;
}

export interface ClaudeArgv {
  program: string;
  args: string[];
}

export function buildClaudeArgv(input: ClaudeLaunchInput): ClaudeArgv {
  const args: string[] = [...input.claude.extraArgs];
  if (input.resume) args.push('--continue');
  // The worktree lives *inside* the main checkout, so Claude picks up parent
  // CLAUDE.md files and symlink targets that point at the root — without an
  // explicit pin it regularly wanders out of the worktree. Skipped when the
  // user supplies their own --append-system-prompt (the CLI only honors one).
  if (input.worktree && !hasAppendSystemPrompt(input)) {
    args.push('--append-system-prompt', worktreeSystemPrompt(input.worktree));
  }
  args.push(...input.passthrough);
  return {
    program: input.claude.command,
    args,
  };
}

function hasAppendSystemPrompt(input: ClaudeLaunchInput): boolean {
  return [...input.claude.extraArgs, ...input.passthrough].includes('--append-system-prompt');
}

function worktreeSystemPrompt(ctx: WorktreeContext): string {
  const what = ctx.workspace
    ? `a grove-managed workspace checkout for branch "${ctx.branch}" — each subdirectory is a git worktree of one of the workspace's repositories`
    : `a grove-managed git worktree for branch "${ctx.branch}"`;
  return (
    `Your working directory ${ctx.worktreePath} is ${what}. ` +
    `Treat it as the project root: create, edit, commit, and run files only inside it. ` +
    `The main checkout at ${ctx.rootDir} is NOT your workspace — never modify anything there. ` +
    `If a path, symlink target, or command output points outside ${ctx.worktreePath}, ` +
    `switch to the corresponding path inside the worktree before acting.`
  );
}

/**
 * True when Claude has a stored transcript for `cwd`, i.e. `claude --continue`
 * would find something. Claude keeps one directory per project under
 * `<config>/projects/`, named after the cwd with every non-alphanumeric
 * character replaced by a dash.
 */
export function hasClaudeSession(cwd: string): boolean {
  const configDir = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
  const projectDir = path.join(configDir, 'projects', encodeProjectDir(cwd));
  try {
    return fs.readdirSync(projectDir).some((entry) => entry.endsWith('.jsonl'));
  } catch {
    return false;
  }
}

/** Mirrors Claude's own cwd → project-dir mapping: realpath, then dash out non-alphanumerics. */
function encodeProjectDir(cwd: string): string {
  let resolved: string;
  try {
    resolved = fs.realpathSync(cwd);
  } catch {
    resolved = path.resolve(cwd);
  }
  return resolved.replace(/[^a-zA-Z0-9]/g, '-');
}
