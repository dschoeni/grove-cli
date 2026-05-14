import { spawnSync } from 'node:child_process';
import { GroveError } from './project.js';

export interface RunPostCreateInput {
  worktreePath: string;
  commands: string[];
}

export function runPostCreateCommands(input: RunPostCreateInput): void {
  for (const cmd of input.commands) {
    process.stderr.write(`\x1b[36m[grove]\x1b[0m running: ${cmd}\n`);
    const result = spawnSync(cmd, {
      cwd: input.worktreePath,
      shell: true,
      stdio: 'inherit',
    });
    if (result.error) {
      throw new GroveError(`postCreateCommand failed to launch: ${result.error.message}`);
    }
    if (typeof result.status === 'number' && result.status !== 0) {
      throw new GroveError(`postCreateCommand exited with status ${result.status}: ${cmd}`);
    }
    if (result.signal) {
      throw new GroveError(`postCreateCommand killed by signal ${result.signal}: ${cmd}`);
    }
  }
}
