//! Shared entries are wired as a two-hop symlink chain:
//!
//! ```text
//! <worktree>/<entry>                → .grove/shared/<entry>   (relative)
//! <worktree>/.grove/shared/<entry>  → <root>/<entry>          (absolute)
//! ```
//!
//! Outside the sandbox (host shells, post-create commands, --no-sandbox) the
//! chain resolves through to the real content at the repo/workspace root.
//! Inside the bwrap sandbox, a tmpfs is mounted over `.grove/shared` and each
//! source is bound at its chain path (see [`resolve_shared_overlay`]), so
//! `realpath(<worktree>/<entry>)` stays inside the worktree instead of escaping
//! into the main tree — and every mountpoint bwrap creates lands in the tmpfs,
//! never on the host filesystem.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::paths;

/// Worktree-relative directory holding the second hop of each shared entry's
/// symlink chain. Lives inside the worktree so it is covered by the existing
/// `/.grove/` entry in `.git/info/exclude` (info/exclude is shared across
/// worktrees, and its anchored patterns match against each checkout's own root).
pub const SHARED_LINK_DIR: &str = ".grove/shared";

pub struct SharedInput<'a> {
    pub root_dir: &'a Path,
    pub worktree_path: &'a Path,
    pub share_read_only: &'a [String],
    pub share_read_write: &'a [String],
}

#[derive(Default, Debug)]
pub struct EnsureSharedLinksResult {
    /// Entries whose worktree link was newly created (or rewired).
    pub linked: Vec<String>,
    /// Entries where a leftover empty bind mountpoint was replaced by the link.
    pub repaired: Vec<String>,
    /// (entry, reason) pairs that were left alone.
    pub skipped: Vec<(String, &'static str)>,
}

pub fn ensure_shared_links(input: &SharedInput) -> Result<EnsureSharedLinksResult> {
    let mut result = EnsureSharedLinksResult::default();
    let mut seen = HashSet::new();

    for entry in input.share_read_only.iter().chain(input.share_read_write) {
        let source = paths::resolve(input.root_dir, entry);
        let link_path = paths::join(input.worktree_path, entry);
        if !seen.insert(link_path.clone()) {
            continue;
        }

        if !source.exists() {
            result.skipped.push((entry.clone(), "source missing"));
            continue;
        }

        let chain_path = paths::join(&input.worktree_path.join(SHARED_LINK_DIR), entry);
        let link_target = paths::relative(link_path.parent().unwrap_or(Path::new("/")), &chain_path);

        match paths::lstat(&link_path) {
            Some(st) if st.file_type().is_symlink() => {
                // Ours (or a pre-chain grove symlink pointing straight at the
                // root) — rewire onto the chain if needed.
                if fs::read_link(&link_path)? != link_target {
                    fs::remove_file(&link_path)?;
                    paths::symlink(&link_target, &link_path)?;
                    result.linked.push(entry.clone());
                }
            }
            Some(st) => {
                if is_empty_mountpoint_junk(&link_path, &st) {
                    // Leftover mountpoint from a bwrap bind landing on the real
                    // fs (or a post-create run that executed without the shares).
                    paths::remove_all(&link_path)?;
                    paths::symlink(&link_target, &link_path)?;
                    result.repaired.push(entry.clone());
                } else {
                    result.skipped.push((entry.clone(), "worktree has its own copy"));
                    continue;
                }
            }
            None => {
                if let Some(parent) = link_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                paths::symlink(&link_target, &link_path)?;
                result.linked.push(entry.clone());
            }
        }

        ensure_chain_link(&chain_path, &source)?;
    }

    Ok(result)
}

/// Second hop: `<worktree>/.grove/shared/<entry>` → `<root>/<entry>`.
fn ensure_chain_link(chain_path: &Path, source: &Path) -> Result<()> {
    match paths::lstat(chain_path) {
        Some(st) if st.file_type().is_symlink() => {
            if fs::read_link(chain_path)? == source {
                return Ok(());
            }
            fs::remove_file(chain_path)?;
        }
        // Grove owns .grove/shared; anything solid here is stale junk.
        Some(_) => paths::remove_all(chain_path)?,
        None => {
            if let Some(parent) = chain_path.parent() {
                fs::create_dir_all(parent)?;
            }
        }
    }
    paths::symlink(source, chain_path)?;
    Ok(())
}

/// A zero-byte file or empty directory at a *declared shared path* is junk left
/// behind by a bind mountpoint, not a copy the branch tracks (git cannot track
/// an empty directory, and a genuinely shared file has content at the root).
fn is_empty_mountpoint_junk(p: &Path, st: &fs::Metadata) -> bool {
    if st.is_file() {
        return st.len() == 0;
    }
    st.is_dir() && paths::is_empty_dir(p)
}

#[derive(Debug, PartialEq, Eq)]
pub struct SharedBindSpec {
    pub source: PathBuf,
    /// The chain path inside the tmpfs overlay, i.e. `<worktree>/.grove/shared/<entry>`.
    pub dest: PathBuf,
    pub writable: bool,
}

#[derive(Debug)]
pub struct SharedOverlay {
    /// Mount a tmpfs here so bwrap's mountpoints never touch the host fs.
    pub tmpfs_dir: PathBuf,
    pub binds: Vec<SharedBindSpec>,
}

/// Resolve the bwrap overlay for shared entries: a tmpfs over
/// `<worktree>/.grove/shared` plus one bind per active entry at its chain path.
/// An entry is active when its source exists and the worktree-level link is a
/// symlink (grove's); a real file/dir there means the branch carries its own
/// copy and wins. None when nothing needs mounting.
pub fn resolve_shared_overlay(input: &SharedInput) -> Option<SharedOverlay> {
    let mut binds = Vec::new();
    let mut seen = HashSet::new();
    let entries =
        input.share_read_only.iter().map(|e| (e, false)).chain(input.share_read_write.iter().map(|e| (e, true)));

    for (entry, writable) in entries {
        let source = paths::resolve(input.root_dir, entry);
        let link_path = paths::join(input.worktree_path, entry);
        if !seen.insert(link_path.clone()) {
            continue;
        }
        if !source.exists() || !paths::is_symlink(&link_path) {
            continue;
        }
        binds.push(SharedBindSpec {
            source,
            dest: paths::join(&input.worktree_path.join(SHARED_LINK_DIR), entry),
            writable,
        });
    }

    if binds.is_empty() {
        return None;
    }
    Some(SharedOverlay { tmpfs_dir: input.worktree_path.join(SHARED_LINK_DIR), binds })
}
