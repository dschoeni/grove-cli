import * as fs from 'node:fs';
import * as path from 'node:path';
import { parseArgs } from 'node:util';
import { findRepoRoot, GroveError, ensureGroveIgnored, currentBranch } from '../lib/project.js';

const HELP = `\
grove init — write a starter .groverc at the repo root

Usage:
  grove init [--base-branch <name>] [--force]

Flags:
  --base-branch <name>   Default base branch for new worktrees. Defaults to current HEAD.
  --force                Overwrite an existing .groverc.
`;

export function runInit(argv: string[]): void {
  const { values } = parseArgs({
    args: argv,
    options: {
      help: { type: 'boolean', short: 'h' },
      'base-branch': { type: 'string' },
      force: { type: 'boolean' },
    },
    strict: true,
    allowPositionals: false,
  });

  if (values.help) {
    process.stdout.write(HELP);
    return;
  }

  const repoRoot = findRepoRoot(process.cwd());
  const grovercPath = path.join(repoRoot, '.groverc');
  if (fs.existsSync(grovercPath) && !values.force) {
    throw new GroveError(`.groverc already exists at ${grovercPath} (use --force to overwrite)`);
  }

  const baseBranch = values['base-branch'] ?? currentBranch(repoRoot) ?? 'main';
  const starter = {
    baseBranch,
    postCreateCommands: [] as string[],
    sandbox: {
      enabled: true,
      shareReadOnly: [] as string[],
      shareReadWrite: [] as string[],
    },
  };
  fs.writeFileSync(grovercPath, JSON.stringify(starter, null, 2) + '\n', 'utf-8');
  ensureGroveIgnored(repoRoot);
  process.stdout.write(`Wrote ${grovercPath}\n`);
}
