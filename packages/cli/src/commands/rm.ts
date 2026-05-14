import * as path from 'node:path';
import { parseArgs } from 'node:util';
import { loadProject, GroveError } from '../lib/project.js';
import {
  parseSlug,
  findGroveWorktree,
  removeWorktree,
  findWorkspaceWorktree,
  removeWorkspaceWorktree,
} from '../lib/worktree.js';

const HELP = `\
grove rm — remove a Grove worktree and its branch

Usage:
  grove rm <slug> [--force]

Flags:
  --force                Discard local changes in the worktree (passes --force to git worktree remove).
`;

export function runRm(argv: string[]): void {
  const { values, positionals } = parseArgs({
    args: argv,
    options: {
      help: { type: 'boolean', short: 'h' },
      force: { type: 'boolean', short: 'f' },
    },
    strict: true,
    allowPositionals: true,
  });

  if (values.help) {
    process.stdout.write(HELP);
    return;
  }
  if (positionals.length !== 1) {
    throw new GroveError('Usage: grove rm <slug>');
  }

  const slug = parseSlug(positionals[0]!);
  const project = loadProject();
  const force = Boolean(values.force);

  if (project.kind === 'workspace') {
    const cwd = path.resolve(process.cwd());
    if (cwd !== path.resolve(project.workspaceRoot)) {
      throw new GroveError(
        `Run this command from the workspace root: ${project.workspaceRoot} (current: ${cwd})`,
      );
    }
    const row = findWorkspaceWorktree(project.workspaceRoot, project.config.repos, slug);
    if (!row) {
      throw new GroveError(`No Grove worktree found for slug "${slug.full}"`);
    }
    removeWorkspaceWorktree({
      workspaceRoot: project.workspaceRoot,
      slug,
      repos: project.config.repos,
      force,
    });
  } else {
    const wt = findGroveWorktree(project.repoRoot, slug);
    if (!wt) {
      throw new GroveError(`No Grove worktree found for slug "${slug.full}"`);
    }
    removeWorktree({ repoRoot: project.repoRoot, slug, force });
  }
  process.stdout.write(`Removed worktree ${slug.full}\n`);
}
