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
