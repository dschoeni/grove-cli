import { execFileSync } from 'node:child_process';
import { GroveError } from './project.js';

export interface GitRunOptions {
  cwd: string;
  /** When true, errors return null instead of throwing. */
  ignoreErrors?: boolean;
}

export function git(args: string[], opts: GitRunOptions): string {
  try {
    return execFileSync('git', args, {
      cwd: opts.cwd,
      encoding: 'utf-8',
      stdio: ['ignore', 'pipe', 'pipe'],
    });
  } catch (err) {
    const stderr = (err as { stderr?: Buffer | string }).stderr;
    const message = typeof stderr === 'string' ? stderr : stderr?.toString() || (err as Error).message;
    throw new GroveError(`git ${args.join(' ')} failed: ${message.trim()}`);
  }
}

export function gitTry(args: string[], opts: GitRunOptions): string | null {
  try {
    return git(args, opts);
  } catch {
    return null;
  }
}

/** Run git with inherited stdio so the user sees progress (fetch, merge, …). */
export function gitInteractive(args: string[], opts: GitRunOptions): void {
  try {
    execFileSync('git', args, { cwd: opts.cwd, stdio: 'inherit' });
  } catch (err) {
    throw new GroveError(`git ${args.join(' ')} failed: ${(err as Error).message}`);
  }
}

/** Resolve a ref to its short hash, or null when it doesn't exist. */
export function shortSha(repoRoot: string, ref: string): string | null {
  const out = gitTry(['rev-parse', '--short', ref], { cwd: repoRoot });
  return out ? out.trim() : null;
}

/**
 * Pick the remote to pull `branch` from: its configured upstream remote when
 * set, else `origin` when present, else the first remote, else null.
 */
export function remoteForBranch(repoRoot: string, branch: string): string | null {
  const configured = gitTry(['config', '--get', `branch.${branch}.remote`], { cwd: repoRoot });
  if (configured && configured.trim()) return configured.trim();
  const remotes = (gitTry(['remote'], { cwd: repoRoot }) ?? '')
    .split('\n')
    .map((r) => r.trim())
    .filter(Boolean);
  if (remotes.includes('origin')) return 'origin';
  return remotes[0] ?? null;
}

export function branchExists(repoRoot: string, branch: string): boolean {
  const out = gitTry(['rev-parse', '--verify', `refs/heads/${branch}`], { cwd: repoRoot });
  return out !== null;
}

export function remoteBranchExists(cwd: string, remote: string, branch: string): boolean {
  const out = gitTry(['rev-parse', '--verify', `refs/remotes/${remote}/${branch}`], { cwd });
  return out !== null;
}

/** Return true if `ref` resolves to a commit (branch, tag, remote ref, or SHA). */
export function revExists(cwd: string, ref: string): boolean {
  return gitTry(['rev-parse', '--verify', '--quiet', `${ref}^{commit}`], { cwd }) !== null;
}

/**
 * Preferred remote for a repo: `origin` when present, otherwise the sole remote,
 * otherwise the first listed. Returns null when the repo has no remotes.
 */
export function defaultRemote(cwd: string): string | null {
  const out = gitTry(['remote'], { cwd });
  if (out === null) return null;
  const remotes = out.split('\n').map((l) => l.trim()).filter(Boolean);
  if (remotes.length === 0) return null;
  if (remotes.includes('origin')) return 'origin';
  return remotes[0]!;
}

/** Configured remote for a branch (branch.<name>.remote), or null. */
export function branchRemote(cwd: string, branch: string): string | null {
  const out = gitTry(['config', '--get', `branch.${branch}.remote`], { cwd });
  return out === null ? null : out.trim() || null;
}

/** Fully-qualified upstream (e.g. "origin/main") for a branch, or null when unset. */
export function upstreamOf(cwd: string, branch: string): string | null {
  const out = gitTry(
    ['rev-parse', '--abbrev-ref', '--symbolic-full-name', `${branch}@{upstream}`],
    { cwd },
  );
  return out === null ? null : out.trim() || null;
}

/** Count commits `branch` is ahead of / behind `target`. */
export function aheadBehind(
  cwd: string,
  branch: string,
  target: string,
): { ahead: number; behind: number } {
  const out = gitTry(['rev-list', '--left-right', '--count', `${branch}...${target}`], { cwd });
  if (out === null) return { ahead: 0, behind: 0 };
  const [ahead, behind] = out.trim().split(/\s+/).map((n) => Number.parseInt(n, 10));
  return { ahead: ahead || 0, behind: behind || 0 };
}

/** True when the working tree at `cwd` has staged or unstaged changes. */
export function workingTreeDirty(cwd: string): boolean {
  const out = gitTry(['status', '--porcelain'], { cwd });
  return out !== null && out.trim().length > 0;
}

/** Path of the worktree that currently has `branch` checked out, or null. */
export function branchCheckedOutAt(repoRoot: string, branch: string): string | null {
  for (const w of listWorktrees(repoRoot)) {
    if (w.branch === branch) return w.path;
  }
  return null;
}

/** Best-effort fetch of a single branch. Returns false on failure. */
export function fetchBranch(cwd: string, remote: string, branch: string): boolean {
  return gitTry(['fetch', remote, branch], { cwd }) !== null;
}

/** Best-effort fetch of a whole remote. Returns false on failure. */
export function fetchRemote(cwd: string, remote: string): boolean {
  return gitTry(['fetch', remote], { cwd }) !== null;
}

/** Clear stale worktree registrations (dirs deleted out from under git). */
export function pruneWorktrees(cwd: string): void {
  gitTry(['worktree', 'prune'], { cwd });
}

export interface WorktreePorcelain {
  path: string;
  head: string;
  branch: string | null;
}

export function listWorktrees(repoRoot: string): WorktreePorcelain[] {
  const raw = git(['worktree', 'list', '--porcelain'], { cwd: repoRoot });
  const entries: WorktreePorcelain[] = [];
  let current: Partial<WorktreePorcelain> = {};
  for (const line of raw.split('\n')) {
    if (line.startsWith('worktree ')) {
      if (current.path) entries.push({ path: current.path, head: current.head ?? '', branch: current.branch ?? null });
      current = { path: line.slice('worktree '.length).trim() };
    } else if (line.startsWith('HEAD ')) {
      current.head = line.slice('HEAD '.length).trim();
    } else if (line.startsWith('branch ')) {
      const ref = line.slice('branch '.length).trim();
      current.branch = ref.replace(/^refs\/heads\//, '');
    }
  }
  if (current.path) entries.push({ path: current.path, head: current.head ?? '', branch: current.branch ?? null });
  return entries;
}
