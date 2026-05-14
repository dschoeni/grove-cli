import { spawn } from 'node:child_process';

export interface ExecInput {
  command: string;
  args: string[];
  env: NodeJS.ProcessEnv;
  cwd: string;
}

/**
 * Spawn `command` with stdio inherited from the parent. Forward common signals
 * to the child and resolve with its exit code so the CLI can mirror it.
 *
 * Note: Node has no native execve. The CLI process stays alive as a thin shim
 * over the child — functionally indistinguishable from exec for the user.
 */
export function execInteractive(input: ExecInput): Promise<number> {
  return new Promise((resolve, reject) => {
    const child = spawn(input.command, input.args, {
      cwd: input.cwd,
      env: input.env,
      stdio: 'inherit',
    });

    const forward = (sig: NodeJS.Signals) => {
      try {
        child.kill(sig);
      } catch {
        // child already exited
      }
    };
    const onInt = () => forward('SIGINT');
    const onTerm = () => forward('SIGTERM');
    const onHup = () => forward('SIGHUP');
    process.on('SIGINT', onInt);
    process.on('SIGTERM', onTerm);
    process.on('SIGHUP', onHup);

    child.on('error', (err) => {
      process.off('SIGINT', onInt);
      process.off('SIGTERM', onTerm);
      process.off('SIGHUP', onHup);
      reject(err);
    });
    child.on('exit', (code, signal) => {
      process.off('SIGINT', onInt);
      process.off('SIGTERM', onTerm);
      process.off('SIGHUP', onHup);
      if (signal) {
        // Mirror the child's signal in our own exit code (128 + sig).
        const sigNum = signalToNumber(signal);
        resolve(128 + sigNum);
      } else {
        resolve(code ?? 0);
      }
    });
  });
}

function signalToNumber(sig: NodeJS.Signals): number {
  // Cover the common cases; default to 15 (TERM) for anything else.
  switch (sig) {
    case 'SIGHUP':
      return 1;
    case 'SIGINT':
      return 2;
    case 'SIGQUIT':
      return 3;
    case 'SIGKILL':
      return 9;
    case 'SIGTERM':
      return 15;
    default:
      return 15;
  }
}
