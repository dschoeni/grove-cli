import { GroveError } from './lib/project.js';
import { runInit } from './commands/init.js';
import { runNew } from './commands/new.js';
import { runResume } from './commands/resume.js';
import { runLs } from './commands/ls.js';
import { runRm } from './commands/rm.js';
import { runCompletion } from './commands/completion.js';
import { runCompleteSlugs } from './commands/complete-slugs.js';

const TOP_HELP = `\
grove — git worktree + bwrap sandbox harness for Claude Code

Usage:
  grove <command> [args]

Commands:
  init                 Write a starter .groverc at the repo root.
  new <slug>           Create a worktree and launch Claude inside a sandbox.
  resume <slug>        Re-enter an existing worktree and continue the last session.
  ls                   List Grove-managed worktrees.
  rm <slug>            Remove a worktree and its branch.
  completion <shell>   Output a bash or zsh completion script.

Use "grove <command> --help" for per-command flags.
`;

async function main(): Promise<number> {
  const [, , subcommand, ...rest] = process.argv;
  if (!subcommand || subcommand === '-h' || subcommand === '--help' || subcommand === 'help') {
    process.stdout.write(TOP_HELP);
    return 0;
  }

  switch (subcommand) {
    case 'init':
      runInit(rest);
      return 0;
    case 'new':
      return await runNew(rest);
    case 'resume':
      return await runResume(rest);
    case 'ls':
      runLs(rest);
      return 0;
    case 'rm':
      runRm(rest);
      return 0;
    case 'completion':
      return runCompletion(rest);
    case '__complete-slugs':
      runCompleteSlugs();
      return 0;
    default:
      process.stderr.write(`Unknown command: ${subcommand}\n\n${TOP_HELP}`);
      return 64;
  }
}

main()
  .then((code) => {
    process.exit(code);
  })
  .catch((err) => {
    if (err instanceof GroveError) {
      process.stderr.write(`\x1b[31merror:\x1b[0m ${err.message}\n`);
      process.exit(1);
    }
    process.stderr.write(`\x1b[31munexpected error:\x1b[0m ${(err as Error).stack ?? err}\n`);
    process.exit(1);
  });
