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
