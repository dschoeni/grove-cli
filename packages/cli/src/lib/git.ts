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
