import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import type { ClaudeConfig } from '../types.js';

export interface ClaudeLaunchInput {
  claude: ClaudeConfig;
  /** Pass-through args from `grove new foo -- ...`. */
  passthrough: string[];
  /** When true, append `--continue` to resume the most recent session. */
  resume: boolean;
}

export interface ClaudeArgv {
  program: string;
  args: string[];
}

export function buildClaudeArgv(input: ClaudeLaunchInput): ClaudeArgv {
  const args: string[] = [...input.claude.extraArgs];
  if (input.resume) args.push('--continue');
  args.push(...input.passthrough);
  return {
    program: input.claude.command,
    args,
  };
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
