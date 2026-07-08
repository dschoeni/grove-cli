import * as fs from 'node:fs';
import * as path from 'node:path';

/**
 * Worktree-relative directory holding the second hop of each shared entry's
 * symlink chain. Lives inside the worktree so it is covered by the existing
 * `/.grove/` entry in `.git/info/exclude` (info/exclude is shared across
 * worktrees, and its anchored patterns match against each checkout's own root).
 */
export const SHARED_LINK_DIR = '.grove/shared';

/**
 * Shared entries are wired as a two-hop symlink chain:
 *
 *   <worktree>/<entry>                    → .grove/shared/<entry>   (relative)
 *   <worktree>/.grove/shared/<entry>      → <root>/<entry>          (absolute)
 *
 * Outside the sandbox (host shells, post-create commands, --no-sandbox) the
 * chain resolves through to the real content at the repo/workspace root.
 * Inside the bwrap sandbox, a tmpfs is mounted over `.grove/shared` and each
 * source is bound at its chain path (see resolveSharedOverlay), so
 * `realpath(<worktree>/<entry>)` stays inside the worktree instead of escaping
 * into the main tree — and every mountpoint bwrap creates lands in the tmpfs,
 * never on the host filesystem.
 */
export interface EnsureSharedLinksInput {
  rootDir: string;
  worktreePath: string;
  shareReadOnly: string[];
  shareReadWrite: string[];
}

export interface EnsureSharedLinksResult {
  /** Entries whose worktree link was newly created (or rewired). */
  linked: string[];
  /** Entries where a leftover empty bind mountpoint was replaced by the link. */
  repaired: string[];
  skipped: { entry: string; reason: string }[];
}

export function ensureSharedLinks(input: EnsureSharedLinksInput): EnsureSharedLinksResult {
  const linked: string[] = [];
  const repaired: string[] = [];
  const skipped: { entry: string; reason: string }[] = [];
  const seen = new Set<string>();

  for (const entry of [...input.shareReadOnly, ...input.shareReadWrite]) {
    const source = path.resolve(input.rootDir, entry);
    const linkPath = path.join(input.worktreePath, entry);
    if (seen.has(linkPath)) continue;
    seen.add(linkPath);

    if (!fs.existsSync(source)) {
      skipped.push({ entry, reason: 'source missing' });
      continue;
    }

    const chainPath = path.join(input.worktreePath, SHARED_LINK_DIR, entry);
    const linkTarget = path.relative(path.dirname(linkPath), chainPath);

    const st = lstatOrNull(linkPath);
    if (st?.isSymbolicLink()) {
      // Ours (or a pre-chain grove symlink pointing straight at the root) —
      // rewire onto the chain if needed.
      if (fs.readlinkSync(linkPath) !== linkTarget) {
        fs.unlinkSync(linkPath);
        fs.symlinkSync(linkTarget, linkPath);
        linked.push(entry);
      }
    } else if (st) {
      if (isEmptyMountpointJunk(linkPath, st)) {
        // Leftover mountpoint from a bwrap bind landing on the real fs (or a
        // post-create run that executed without the shares in place).
        fs.rmSync(linkPath, { recursive: true, force: true });
        fs.symlinkSync(linkTarget, linkPath);
        repaired.push(entry);
      } else {
        skipped.push({ entry, reason: 'worktree has its own copy' });
        continue;
      }
    } else {
      fs.mkdirSync(path.dirname(linkPath), { recursive: true });
      fs.symlinkSync(linkTarget, linkPath);
      linked.push(entry);
    }

    ensureChainLink(chainPath, source);
  }

  return { linked, repaired, skipped };
}

/** Second hop: <worktree>/.grove/shared/<entry> → <root>/<entry>. */
function ensureChainLink(chainPath: string, source: string): void {
  const st = lstatOrNull(chainPath);
  if (st?.isSymbolicLink()) {
    if (fs.readlinkSync(chainPath) === source) return;
    fs.unlinkSync(chainPath);
  } else if (st) {
    // Grove owns .grove/shared; anything solid here is stale junk.
    fs.rmSync(chainPath, { recursive: true, force: true });
  } else {
    fs.mkdirSync(path.dirname(chainPath), { recursive: true });
  }
  fs.symlinkSync(source, chainPath);
}

/**
 * A zero-byte file or empty directory at a *declared shared path* is junk left
 * behind by a bind mountpoint, not a copy the branch tracks (git cannot track
 * an empty directory, and a genuinely shared file has content at the root).
 */
function isEmptyMountpointJunk(p: string, st: fs.Stats): boolean {
  if (st.isFile()) return st.size === 0;
  if (st.isDirectory()) {
    try {
      return fs.readdirSync(p).length === 0;
    } catch {
      return false;
    }
  }
  return false;
}

function lstatOrNull(p: string): fs.Stats | null {
  try {
    return fs.lstatSync(p);
  } catch {
    return null;
  }
}

export interface SharedBindSpec {
  source: string;
  /** The chain path inside the tmpfs overlay, i.e. <worktree>/.grove/shared/<entry>. */
  dest: string;
  writable: boolean;
}

export interface SharedOverlay {
  /** Mount a tmpfs here so bwrap's mountpoints never touch the host fs. */
  tmpfsDir: string;
  binds: SharedBindSpec[];
}

/**
 * Resolve the bwrap overlay for shared entries: a tmpfs over
 * `<worktree>/.grove/shared` plus one bind per active entry at its chain path.
 * An entry is active when its source exists and the worktree-level link is a
 * symlink (grove's); a real file/dir there means the branch carries its own
 * copy and wins. Returns null when nothing needs mounting.
 */
export function resolveSharedOverlay(input: EnsureSharedLinksInput): SharedOverlay | null {
  const binds: SharedBindSpec[] = [];
  const seen = new Set<string>();

  const consider = (entry: string, writable: boolean) => {
    const source = path.resolve(input.rootDir, entry);
    const linkPath = path.join(input.worktreePath, entry);
    if (seen.has(linkPath)) return;
    seen.add(linkPath);
    if (!fs.existsSync(source)) return;
    if (!lstatOrNull(linkPath)?.isSymbolicLink()) return;
    binds.push({
      source,
      dest: path.join(input.worktreePath, SHARED_LINK_DIR, entry),
      writable,
    });
  };

  for (const entry of input.shareReadOnly) consider(entry, false);
  for (const entry of input.shareReadWrite) consider(entry, true);

  if (binds.length === 0) return null;
  return { tmpfsDir: path.join(input.worktreePath, SHARED_LINK_DIR), binds };
}
