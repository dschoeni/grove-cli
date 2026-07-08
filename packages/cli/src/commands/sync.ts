import * as path from 'node:path';
import { parseArgs } from 'node:util';
import { loadProject, GroveError } from '../lib/project.js';
import {
  parseSlug,
  findGroveWorktree,
  findWorkspaceWorktree,
} from '../lib/worktree.js';
import {
  aheadBehind,
  branchRemote,
  defaultRemote,
  fetchBranch,
  git,
  revExists,
  upstreamOf,
  workingTreeDirty,
} from '../lib/git.js';

const HELP = `\
grove sync — bring a worktree's branch(es) up to date with their remote

Usage:
  grove sync <slug> [--hard] [--remote <name>] [--dry-run]

Fetches the remote and, per branch:
  • fast-forwards when the remote is strictly ahead;
  • leaves diverged branches untouched and asks for --hard (the force-push case);
  • with --hard, resets the branch to its upstream, discarding local divergence.

In a workspace every repo of the worktree is synced independently.

Flags:
  --hard                 git reset --hard to the upstream even when diverged/ahead. Discards local commits.
  --remote <name>        Remote to sync against. Defaults to the branch's upstream remote, else origin.
  --dry-run              Report what each branch would do without touching refs or the working tree.
`;

type SyncStatus =
  | 'up-to-date'
  | 'fast-forwarded'
  | 'reset'
  | 'ahead'
  | 'diverged'
  | 'no-upstream'
  | 'no-remote'
  | 'error';

interface SyncResult {
  status: SyncStatus;
  detail: string;
}

interface SyncOpts {
  hard: boolean;
  dryRun: boolean;
  remoteOverride: string | null;
}

export function runSync(argv: string[]): number {
  const { values, positionals } = parseArgs({
    args: argv,
    options: {
      help: { type: 'boolean', short: 'h' },
      hard: { type: 'boolean' },
      remote: { type: 'string' },
      'dry-run': { type: 'boolean' },
    },
    strict: true,
    allowPositionals: true,
  });

  if (values.help) {
    process.stdout.write(HELP);
    return 0;
  }
  if (positionals.length !== 1) {
    throw new GroveError('Usage: grove sync <slug>');
  }

  const slug = parseSlug(positionals[0]!);
  const project = loadProject();
  const opts: SyncOpts = {
    hard: Boolean(values.hard),
    dryRun: Boolean(values['dry-run']),
    remoteOverride: values.remote ?? null,
  };

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
    let failed = false;
    for (const r of row.perRepo) {
      if (!r.registered) continue;
      const branch = r.branch ?? slug.full;
      const result = safeSync(r.path, branch, opts);
      report(r.repo.name, result);
      if (result.status === 'diverged' || result.status === 'error') failed = true;
    }
    return failed ? 1 : 0;
  }

  const wt = findGroveWorktree(project.repoRoot, slug);
  if (!wt) {
    throw new GroveError(`No Grove worktree found for slug "${slug.full}"`);
  }
  const branch = wt.branch ?? slug.full;
  const result = safeSync(wt.path, branch, opts);
  report(slug.full, result);
  return result.status === 'diverged' || result.status === 'error' ? 1 : 0;
}

function safeSync(cwd: string, branch: string, opts: SyncOpts): SyncResult {
  try {
    return syncWorktree(cwd, branch, opts);
  } catch (err) {
    const message = err instanceof GroveError ? err.message : String((err as Error).message ?? err);
    return { status: 'error', detail: message };
  }
}

function syncWorktree(cwd: string, branch: string, opts: SyncOpts): SyncResult {
  const remote = opts.remoteOverride ?? branchRemote(cwd, branch) ?? defaultRemote(cwd);
  if (!remote) {
    return { status: 'no-remote', detail: 'repo has no remote' };
  }

  const target = upstreamOf(cwd, branch) ?? `${remote}/${branch}`;

  fetchBranch(cwd, remote, branch); // best-effort; a stale target is still handled below
  if (!revExists(cwd, target)) {
    return { status: 'no-upstream', detail: `${target} not found on ${remote}` };
  }

  const { ahead, behind } = aheadBehind(cwd, branch, target);

  if (behind === 0 && ahead === 0) {
    return { status: 'up-to-date', detail: `even with ${target}` };
  }

  // Fast-forward: remote strictly ahead, nothing local to lose.
  if (behind > 0 && ahead === 0) {
    if (opts.dryRun) return { status: 'fast-forwarded', detail: `would fast-forward ${behind} commit(s) from ${target}` };
    git(['merge', '--ff-only', target], { cwd });
    return { status: 'fast-forwarded', detail: `fast-forwarded ${behind} commit(s) from ${target}` };
  }

  // Diverged (force-push) or purely ahead: only touch it under --hard.
  const divergedDetail =
    behind > 0
      ? `diverged (ahead ${ahead}, behind ${behind})`
      : `${ahead} local commit(s) not on ${remote}`;

  if (!opts.hard) {
    return {
      status: behind > 0 ? 'diverged' : 'ahead',
      detail: `${divergedDetail}; re-run with --hard to reset to ${target}`,
    };
  }

  if (opts.dryRun) {
    return { status: 'reset', detail: `would reset --hard to ${target} (${divergedDetail})` };
  }
  const dirty = workingTreeDirty(cwd);
  git(['reset', '--hard', target], { cwd });
  const discarded = dirty ? ', discarded uncommitted changes' : '';
  return { status: 'reset', detail: `reset to ${target} (${divergedDetail}${discarded})` };
}

function report(label: string, result: SyncResult): void {
  const tags: Record<SyncStatus, string> = {
    'up-to-date': '\x1b[32mup-to-date\x1b[0m',
    'fast-forwarded': '\x1b[32mfast-forwarded\x1b[0m',
    reset: '\x1b[33mreset\x1b[0m',
    ahead: '\x1b[33mahead\x1b[0m',
    diverged: '\x1b[31mdiverged\x1b[0m',
    'no-upstream': '\x1b[90mno-upstream\x1b[0m',
    'no-remote': '\x1b[90mno-remote\x1b[0m',
    error: '\x1b[31merror\x1b[0m',
  };
  process.stdout.write(`${label}: ${tags[result.status]} — ${result.detail}\n`);
}
