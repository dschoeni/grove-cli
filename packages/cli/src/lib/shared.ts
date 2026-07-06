import * as fs from 'node:fs';
import * as path from 'node:path';

export interface SymlinkSharedInput {
  repoRoot: string;
  worktreePath: string;
  shareReadOnly: string[];
  shareReadWrite: string[];
}

export interface SymlinkResult {
  created: string[];
  skipped: { entry: string; reason: string }[];
}

export function symlinkShared(input: SymlinkSharedInput): SymlinkResult {
  const created: string[] = [];
  const skipped: { entry: string; reason: string }[] = [];
  const all = [...input.shareReadOnly, ...input.shareReadWrite];

  for (const entry of all) {
    const source = path.resolve(input.repoRoot, entry);
    const target = path.join(input.worktreePath, entry);

    if (!fs.existsSync(source)) {
      skipped.push({ entry, reason: 'source missing' });
      continue;
    }
    if (fs.existsSync(target) || isSymlink(target)) {
      skipped.push({ entry, reason: 'target exists' });
      continue;
    }
    const parent = path.dirname(target);
    fs.mkdirSync(parent, { recursive: true });
    fs.symlinkSync(source, target);
    created.push(entry);
  }

  return { created, skipped };
}

function isSymlink(p: string): boolean {
  try {
    return fs.lstatSync(p).isSymbolicLink();
  } catch {
    return false;
  }
}

export interface SharedBindSpec {
  source: string;
  dest: string;
  writable: boolean;
}

export interface ResolveSharedBindsInput {
  repoRoot: string;
  worktreePath: string;
  shareReadOnly: string[];
  shareReadWrite: string[];
}

/**
 * Resolve shared entries to bwrap bind specs that mount each source at its
 * *worktree-relative* path (e.g. `<worktree>/node_modules`), NOT the repo-root
 * path. Binding at the worktree dest keeps `realpath` inside the worktree, so a
 * tool that canonicalizes a shared path (node_modules, .env, …) doesn't escape
 * into the main tree and mis-detect the project root as the parent repo.
 *
 * Side effect: unlinks a stale grove symlink sitting at the dest (left by a
 * prior `grove new --no-sandbox`). bwrap resolves a symlink dest back to the
 * repo-root source before mounting, which would re-introduce the escape; a real
 * mountpoint is required. Entries whose worktree already has its own real
 * file/dir at that path are skipped so the branch's own copy wins.
 */
export function resolveSharedBinds(input: ResolveSharedBindsInput): SharedBindSpec[] {
  const specs: SharedBindSpec[] = [];
  const seen = new Set<string>();

  const consider = (entry: string, writable: boolean) => {
    const source = path.resolve(input.repoRoot, entry);
    const dest = path.join(input.worktreePath, entry);
    if (seen.has(dest)) return;
    if (!fs.existsSync(source)) return;
    if (isSymlink(dest)) {
      // Stale grove symlink from a no-sandbox run — remove so the bind lands on
      // a real mountpoint instead of resolving back through the link.
      try {
        fs.unlinkSync(dest);
      } catch {
        // best effort; bwrap will error loudly if the dest is unusable
      }
    } else if (fs.existsSync(dest)) {
      // Worktree tracks its own copy at this path — leave it be.
      return;
    }
    seen.add(dest);
    specs.push({ source, dest, writable });
  };

  for (const entry of input.shareReadOnly) consider(entry, false);
  for (const entry of input.shareReadWrite) consider(entry, true);
  return specs;
}
