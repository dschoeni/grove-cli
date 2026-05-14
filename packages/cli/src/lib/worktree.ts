import * as fs from 'node:fs';
import * as path from 'node:path';
import { BRANCH_TYPES, type BranchType, type WorkspaceRepo } from '../types.js';
import { GroveError } from './project.js';
import { branchExists, git, listWorktrees, type WorktreePorcelain } from './git.js';

const SLUG_NAME_RE = /^[A-Za-z0-9][A-Za-z0-9._\-/]*$/;
const SLUG_FORBIDDEN_RE = /(^|\/)\.\.($|\/)|[~^:?*\\[]/;

export interface ParsedSlug {
  /** Full slug as the user supplied it, e.g. "feat/auth-flow". Used as branch name. */
  full: string;
  type: BranchType;
  /** Tail after the type prefix, e.g. "auth-flow". */
  name: string;
}

export function parseSlug(input: string): ParsedSlug {
  const slug = input.trim();
  if (!slug) throw new GroveError('Slug cannot be empty');

  const slashIdx = slug.indexOf('/');
  if (slashIdx < 0) {
    throw new GroveError(`Slug must start with one of: ${BRANCH_TYPES.map((t) => `${t}/`).join(', ')}`);
  }
  const prefix = slug.slice(0, slashIdx);
  const rest = slug.slice(slashIdx + 1);
  if (!BRANCH_TYPES.includes(prefix as BranchType)) {
    throw new GroveError(`Slug prefix must be one of: ${BRANCH_TYPES.join(', ')} (got "${prefix}")`);
  }
  if (!rest) throw new GroveError(`Slug needs a name after "${prefix}/"`);
  if (!SLUG_NAME_RE.test(rest) || SLUG_FORBIDDEN_RE.test(rest)) {
    throw new GroveError(`Slug name "${rest}" contains invalid characters`);
  }

  return { full: slug, type: prefix as BranchType, name: rest };
}

export function worktreePathFor(repoRoot: string, slug: ParsedSlug): string {
  return path.join(repoRoot, '.grove', slug.type, slug.name);
}

export function isGroveWorktree(repoRoot: string, worktreePath: string): boolean {
  const grovePrefix = path.join(repoRoot, '.grove') + path.sep;
  return worktreePath.startsWith(grovePrefix);
}

export function findGroveWorktree(repoRoot: string, slug: ParsedSlug): WorktreePorcelain | null {
  const target = worktreePathFor(repoRoot, slug);
  return listWorktrees(repoRoot).find((w) => w.path === target) ?? null;
}

export function listGroveWorktrees(repoRoot: string): WorktreePorcelain[] {
  return listWorktrees(repoRoot).filter((w) => isGroveWorktree(repoRoot, w.path));
}

export interface AddWorktreeInput {
  repoRoot: string;
  slug: ParsedSlug;
  baseBranch: string | null;
}

export function addWorktree(input: AddWorktreeInput): string {
  const { repoRoot, slug, baseBranch } = input;
  if (branchExists(repoRoot, slug.full)) {
    throw new GroveError(`Branch "${slug.full}" already exists`);
  }
  const worktreePath = worktreePathFor(repoRoot, slug);
  if (fs.existsSync(worktreePath)) {
    throw new GroveError(`Worktree path already exists: ${worktreePath}`);
  }
  fs.mkdirSync(path.dirname(worktreePath), { recursive: true });

  const args = ['worktree', 'add', '-b', slug.full, worktreePath];
  if (baseBranch) args.push(baseBranch);
  git(args, { cwd: repoRoot });
  return worktreePath;
}

export interface RemoveWorktreeInput {
  repoRoot: string;
  slug: ParsedSlug;
  force: boolean;
}

export function removeWorktree(input: RemoveWorktreeInput): void {
  const { repoRoot, slug, force } = input;
  const worktreePath = worktreePathFor(repoRoot, slug);
  const args = ['worktree', 'remove'];
  if (force) args.push('--force');
  args.push(worktreePath);
  git(args, { cwd: repoRoot });

  // Best-effort branch delete. -D so it works even if not merged.
  if (branchExists(repoRoot, slug.full)) {
    git(['branch', '-D', slug.full], { cwd: repoRoot });
  }

  // Clean up empty parent type dir (.grove/feat/ if it's empty).
  const parent = path.dirname(worktreePath);
  try {
    if (fs.existsSync(parent) && fs.readdirSync(parent).length === 0) {
      fs.rmdirSync(parent);
    }
  } catch {
    // non-fatal
  }
}

export function rollbackWorktree(repoRoot: string, slug: ParsedSlug): void {
  try {
    removeWorktree({ repoRoot, slug, force: true });
  } catch {
    // Already gone or never created; nothing to do.
  }
}

// ============================================================
// Workspace helpers
// ============================================================

export function workspaceWorktreeDir(workspaceRoot: string, slug: ParsedSlug): string {
  return path.join(workspaceRoot, '.grove', slug.type, slug.name);
}

export function repoWorktreePath(
  workspaceRoot: string,
  slug: ParsedSlug,
  repo: WorkspaceRepo,
): string {
  return path.join(workspaceWorktreeDir(workspaceRoot, slug), repo.path);
}

export function repoAbsPath(workspaceRoot: string, repo: WorkspaceRepo): string {
  return path.resolve(workspaceRoot, repo.path);
}

export interface AddWorkspaceWorktreeInput {
  workspaceRoot: string;
  slug: ParsedSlug;
  repos: WorkspaceRepo[];
  /** When set, overrides every repo's baseBranch. */
  baseBranchOverride: string | null;
}

/**
 * Create one git worktree per repo under `<workspaceRoot>/.grove/<slug>/<repo.path>`.
 * Rolls back any partial state if any sub-repo fails.
 */
export function addWorkspaceWorktree(input: AddWorkspaceWorktreeInput): string {
  const { workspaceRoot, slug, repos, baseBranchOverride } = input;
  const workspaceDir = workspaceWorktreeDir(workspaceRoot, slug);

  if (fs.existsSync(workspaceDir)) {
    throw new GroveError(`Workspace worktree path already exists: ${workspaceDir}`);
  }
  for (const repo of repos) {
    const abs = repoAbsPath(workspaceRoot, repo);
    if (branchExists(abs, slug.full)) {
      throw new GroveError(`Branch "${slug.full}" already exists in repo "${repo.name}" (${repo.path})`);
    }
  }

  const created: WorkspaceRepo[] = [];
  try {
    for (const repo of repos) {
      const base = baseBranchOverride ?? repo.baseBranch;
      if (!base) {
        throw new GroveError(
          `No base branch for repo "${repo.name}". Set repos[].baseBranch in .groverc or pass --from.`,
        );
      }
      const target = repoWorktreePath(workspaceRoot, slug, repo);
      fs.mkdirSync(path.dirname(target), { recursive: true });
      git(['worktree', 'add', '-b', slug.full, target, base], {
        cwd: repoAbsPath(workspaceRoot, repo),
      });
      created.push(repo);
    }
  } catch (err) {
    rollbackWorkspaceWorktree(workspaceRoot, slug, created);
    throw err;
  }

  return workspaceDir;
}

export function rollbackWorkspaceWorktree(
  workspaceRoot: string,
  slug: ParsedSlug,
  repos: WorkspaceRepo[],
): void {
  for (const repo of repos) {
    const target = repoWorktreePath(workspaceRoot, slug, repo);
    try {
      git(['worktree', 'remove', '--force', target], { cwd: repoAbsPath(workspaceRoot, repo) });
    } catch {
      // best effort
    }
    try {
      if (branchExists(repoAbsPath(workspaceRoot, repo), slug.full)) {
        git(['branch', '-D', slug.full], { cwd: repoAbsPath(workspaceRoot, repo) });
      }
    } catch {
      // best effort
    }
  }
  // Remove the (likely empty) workspace dir tree.
  const workspaceDir = workspaceWorktreeDir(workspaceRoot, slug);
  try {
    fs.rmSync(workspaceDir, { recursive: true, force: true });
  } catch {
    // best effort
  }
  cleanupEmptyTypeDir(workspaceRoot, slug);
}

export interface RemoveWorkspaceWorktreeInput {
  workspaceRoot: string;
  slug: ParsedSlug;
  repos: WorkspaceRepo[];
  force: boolean;
}

export function removeWorkspaceWorktree(input: RemoveWorkspaceWorktreeInput): void {
  const { workspaceRoot, slug, repos, force } = input;
  for (const repo of repos) {
    const target = repoWorktreePath(workspaceRoot, slug, repo);
    if (!fs.existsSync(target)) continue;
    const args = ['worktree', 'remove'];
    if (force) args.push('--force');
    args.push(target);
    git(args, { cwd: repoAbsPath(workspaceRoot, repo) });
    if (branchExists(repoAbsPath(workspaceRoot, repo), slug.full)) {
      git(['branch', '-D', slug.full], { cwd: repoAbsPath(workspaceRoot, repo) });
    }
  }
  // Whatever's left in <ws>/.grove/<slug>/ is shared-symlinks or empty parent dirs.
  const workspaceDir = workspaceWorktreeDir(workspaceRoot, slug);
  try {
    fs.rmSync(workspaceDir, { recursive: true, force: true });
  } catch {
    // non-fatal
  }
  cleanupEmptyTypeDir(workspaceRoot, slug);
}

function cleanupEmptyTypeDir(workspaceRoot: string, slug: ParsedSlug): void {
  const typeDir = path.join(workspaceRoot, '.grove', slug.type);
  try {
    if (fs.existsSync(typeDir) && fs.readdirSync(typeDir).length === 0) {
      fs.rmdirSync(typeDir);
    }
  } catch {
    // non-fatal
  }
}

export interface WorkspaceWorktreeRepoRow {
  repo: WorkspaceRepo;
  branch: string | null;
  path: string;
  registered: boolean;
}

export interface WorkspaceWorktreeRow {
  slug: string;
  workspaceDir: string;
  perRepo: WorkspaceWorktreeRepoRow[];
}

export function listWorkspaceWorktrees(
  workspaceRoot: string,
  repos: WorkspaceRepo[],
): WorkspaceWorktreeRow[] {
  const groveDir = path.join(workspaceRoot, '.grove');
  if (!fs.existsSync(groveDir)) return [];

  const slugs = enumerateSlugDirs(groveDir);

  // Cache `git worktree list` per sub-repo to avoid N×M shell-outs.
  const perRepoIndex = new Map<string, Map<string, WorktreePorcelain>>();
  for (const repo of repos) {
    const repoAbs = repoAbsPath(workspaceRoot, repo);
    const idx = new Map<string, WorktreePorcelain>();
    try {
      for (const w of listWorktrees(repoAbs)) idx.set(w.path, w);
    } catch {
      // sub-repo unreachable — leave index empty
    }
    perRepoIndex.set(repo.path, idx);
  }

  const rows: WorkspaceWorktreeRow[] = [];
  for (const slug of slugs) {
    const workspaceDir = path.join(groveDir, slug);
    const perRepo: WorkspaceWorktreeRepoRow[] = repos.map((repo) => {
      const target = path.join(workspaceDir, repo.path);
      const entry = perRepoIndex.get(repo.path)?.get(target);
      return {
        repo,
        branch: entry?.branch ?? null,
        path: target,
        registered: Boolean(entry),
      };
    });
    if (perRepo.some((r) => r.registered)) {
      rows.push({ slug, workspaceDir, perRepo });
    }
  }
  return rows;
}

/** Find immediate `<type>/<name>` directory pairs under `.grove/`. */
function enumerateSlugDirs(groveDir: string): string[] {
  const out: string[] = [];
  let typeEntries: fs.Dirent[];
  try {
    typeEntries = fs.readdirSync(groveDir, { withFileTypes: true });
  } catch {
    return out;
  }
  for (const typeEnt of typeEntries) {
    if (!typeEnt.isDirectory()) continue;
    if (!BRANCH_TYPES.includes(typeEnt.name as BranchType)) continue;
    let nameEntries: fs.Dirent[];
    try {
      nameEntries = fs.readdirSync(path.join(groveDir, typeEnt.name), { withFileTypes: true });
    } catch {
      continue;
    }
    for (const nameEnt of nameEntries) {
      if (!nameEnt.isDirectory()) continue;
      out.push(`${typeEnt.name}/${nameEnt.name}`);
    }
  }
  return out.sort();
}

export function findWorkspaceWorktree(
  workspaceRoot: string,
  repos: WorkspaceRepo[],
  slug: ParsedSlug,
): WorkspaceWorktreeRow | null {
  const dir = workspaceWorktreeDir(workspaceRoot, slug);
  if (!fs.existsSync(dir)) return null;
  const matches = listWorkspaceWorktrees(workspaceRoot, repos).filter((r) => r.workspaceDir === dir);
  return matches[0] ?? null;
}
