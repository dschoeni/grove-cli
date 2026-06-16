import * as path from 'node:path';
import { parseArgs } from 'node:util';
import { loadProject, GroveError } from '../lib/project.js';
import {
  gitInteractive,
  shortSha,
  remoteForBranch,
  listWorktrees,
} from '../lib/git.js';
import { repoAbsPath } from '../lib/worktree.js';
import type { ProjectContext } from '../types.js';

const HELP = `\
grove pull — fast-forward the base branch to its latest remote state

Usage:
  grove pull [branch] [--remote <name>]

Arguments:
  [branch]               Branch to update. Defaults to the .groverc baseBranch
                         (per-repo baseBranch in workspace mode).

Flags:
  --remote <name>        Remote to pull from. Defaults to the branch's upstream
                         remote, or "origin".

Updates are fast-forward only; a diverged branch is reported, not merged. When
the branch is checked out in a worktree it is fast-forwarded in place,
otherwise its ref is advanced directly.
`;

export function runPull(argv: string[]): void {
  const { values, positionals } = parseArgs({
    args: argv,
    options: {
      help: { type: 'boolean', short: 'h' },
      remote: { type: 'string' },
    },
    strict: true,
    allowPositionals: true,
  });

  if (values.help) {
    process.stdout.write(HELP);
    return;
  }
  if (positionals.length > 1) {
    throw new GroveError(`Too many positionals: ${positionals.join(' ')}`);
  }

  const branchOverride = positionals[0] ?? null;
  const remoteOverride = values.remote ?? null;
  const project = loadProject();

  if (project.kind === 'workspace') {
    runPullWorkspace(project, branchOverride, remoteOverride);
    return;
  }

  const branch = branchOverride ?? project.config.baseBranch;
  if (!branch) {
    throw new GroveError('Could not determine base branch. Pass a branch name or set baseBranch in .groverc.');
  }
  pullBranch(project.repoRoot, branch, remoteOverride);
}

function runPullWorkspace(
  project: Extract<ProjectContext, { kind: 'workspace' }>,
  branchOverride: string | null,
  remoteOverride: string | null,
): void {
  const cwd = path.resolve(process.cwd());
  if (cwd !== path.resolve(project.workspaceRoot)) {
    throw new GroveError(
      `Run this command from the workspace root: ${project.workspaceRoot} (current: ${cwd})`,
    );
  }
  for (const repo of project.config.repos) {
    const branch = branchOverride ?? repo.baseBranch;
    if (!branch) {
      process.stderr.write(
        `\x1b[33m[grove]\x1b[0m ${repo.name}: no baseBranch configured, skipping\n`,
      );
      continue;
    }
    process.stderr.write(`\x1b[36m[grove]\x1b[0m ${repo.name} (${repo.path})\n`);
    pullBranch(repoAbsPath(project.workspaceRoot, repo), branch, remoteOverride);
  }
}

/** Fetch and fast-forward `branch` in the repo at `repoRoot`. */
function pullBranch(repoRoot: string, branch: string, remoteOverride: string | null): void {
  const remote = remoteOverride ?? remoteForBranch(repoRoot, branch);
  if (!remote) {
    throw new GroveError(`No git remote configured to pull "${branch}" from.`);
  }

  const before = shortSha(repoRoot, branch);
  const checkout = listWorktrees(repoRoot).find((w) => w.branch === branch);

  if (checkout) {
    // Branch is checked out somewhere: fetch the remote-tracking ref, then
    // fast-forward the working tree in place.
    gitInteractive(['fetch', remote, branch], { cwd: repoRoot });
    gitInteractive(['merge', '--ff-only', `${remote}/${branch}`], { cwd: checkout.path });
  } else {
    // Not checked out: a refspec fetch advances the local ref (fast-forward only).
    gitInteractive(['fetch', remote, `${branch}:${branch}`], { cwd: repoRoot });
  }

  const after = shortSha(repoRoot, branch);
  if (before && after && before === after) {
    process.stdout.write(`\x1b[32m✓\x1b[0m ${branch} already up to date (${after})\n`);
  } else {
    process.stdout.write(`\x1b[32m✓\x1b[0m ${branch} ${before ?? '(new)'} → ${after}\n`);
  }
}
