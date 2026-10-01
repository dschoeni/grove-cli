use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

fn collect<I, S>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter().map(|a| a.as_ref().to_os_string()).collect()
}

/// "exited with status N", or the signal description when there is no code.
pub fn describe_exit(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exited with status {code}"),
        None => status.to_string(),
    }
}

fn display(args: &[OsString]) -> String {
    args.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ")
}

/// Run git in `cwd` and return its stdout. Failures become a `GroveError`
/// carrying git's stderr.
pub fn git<I, S>(args: I, cwd: &Path) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = collect(args);
    let out = Command::new("git")
        .args(&args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Error::Grove(format!("git {} failed: {e}", display(&args))))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let message = if stderr.trim().is_empty() { describe_exit(out.status) } else { stderr.trim().to_string() };
        return Err(Error::Grove(format!("git {} failed: {message}", display(&args))));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Like [`git`], but any failure yields `None`.
pub fn git_try<I, S>(args: I, cwd: &Path) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git(args, cwd).ok()
}

/// Run git with inherited stdio so the user sees progress (fetch, merge, …).
pub fn git_interactive<I, S>(args: I, cwd: &Path) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = collect(args);
    let status = Command::new("git")
        .args(&args)
        .current_dir(cwd)
        .status()
        .map_err(|e| Error::Grove(format!("git {} failed: {e}", display(&args))))?;
    if !status.success() {
        return Err(Error::Grove(format!("git {} failed: {}", display(&args), describe_exit(status))));
    }
    Ok(())
}

/// Resolve a ref to its short hash, or None when it doesn't exist.
pub fn short_sha(repo_root: &Path, rev: &str) -> Option<String> {
    git_try(["rev-parse", "--short", rev], repo_root).map(|s| s.trim().to_string())
}

/// Pick the remote to pull `branch` from: its configured upstream remote when
/// set, else `origin` when present, else the first remote, else None.
pub fn remote_for_branch(repo_root: &Path, branch: &str) -> Option<String> {
    branch_remote(repo_root, branch).or_else(|| default_remote(repo_root))
}

pub fn branch_exists(repo_root: &Path, branch: &str) -> bool {
    git_try(["rev-parse", "--verify", &format!("refs/heads/{branch}")], repo_root).is_some()
}

pub fn remote_branch_exists(cwd: &Path, remote: &str, branch: &str) -> bool {
    git_try(["rev-parse", "--verify", &format!("refs/remotes/{remote}/{branch}")], cwd).is_some()
}

/// True if `rev` resolves to a commit (branch, tag, remote ref, or SHA).
pub fn rev_exists(cwd: &Path, rev: &str) -> bool {
    git_try(["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")], cwd).is_some()
}

/// Preferred remote for a repo: `origin` when present, otherwise the first
/// listed. None when the repo has no remotes.
pub fn default_remote(cwd: &Path) -> Option<String> {
    let out = git_try(["remote"], cwd)?;
    let remotes: Vec<&str> = out.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if remotes.contains(&"origin") {
        return Some("origin".into());
    }
    remotes.first().map(|r| r.to_string())
}

/// Configured remote for a branch (`branch.<name>.remote`), or None.
pub fn branch_remote(cwd: &Path, branch: &str) -> Option<String> {
    let out = git_try(["config", "--get", &format!("branch.{branch}.remote")], cwd)?;
    let remote = out.trim();
    (!remote.is_empty()).then(|| remote.to_string())
}

/// Fully-qualified upstream (e.g. "origin/main") for a branch, or None when unset.
pub fn upstream_of(cwd: &Path, branch: &str) -> Option<String> {
    let out = git_try(["rev-parse", "--abbrev-ref", "--symbolic-full-name", &format!("{branch}@{{upstream}}")], cwd)?;
    let upstream = out.trim();
    (!upstream.is_empty()).then(|| upstream.to_string())
}

/// Count commits `branch` is (ahead of, behind) `target`.
pub fn ahead_behind(cwd: &Path, branch: &str, target: &str) -> (u64, u64) {
    let Some(out) = git_try(["rev-list", "--left-right", "--count", &format!("{branch}...{target}")], cwd) else {
        return (0, 0);
    };
    let mut nums = out.split_whitespace().map(|n| n.parse().unwrap_or(0));
    (nums.next().unwrap_or(0), nums.next().unwrap_or(0))
}

/// True when the working tree at `cwd` has staged or unstaged changes.
pub fn working_tree_dirty(cwd: &Path) -> bool {
    git_try(["status", "--porcelain"], cwd).is_some_and(|o| !o.trim().is_empty())
}

/// Path of the worktree that currently has `branch` checked out, or None.
pub fn branch_checked_out_at(repo_root: &Path, branch: &str) -> Result<Option<PathBuf>> {
    Ok(list_worktrees(repo_root)?.into_iter().find(|w| w.branch.as_deref() == Some(branch)).map(|w| w.path))
}

/// Best-effort fetch of a single branch. Returns false on failure.
pub fn fetch_branch(cwd: &Path, remote: &str, branch: &str) -> bool {
    git_try(["fetch", remote, branch], cwd).is_some()
}

/// Best-effort fetch of a whole remote. Returns false on failure.
pub fn fetch_remote(cwd: &Path, remote: &str) -> bool {
    git_try(["fetch", remote], cwd).is_some()
}

/// Clear stale worktree registrations (dirs deleted out from under git).
pub fn prune_worktrees(cwd: &Path) {
    let _ = git_try(["worktree", "prune"], cwd);
}

#[derive(Clone, Debug)]
pub struct WorktreePorcelain {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
}

pub fn list_worktrees(repo_root: &Path) -> Result<Vec<WorktreePorcelain>> {
    let raw = git(["worktree", "list", "--porcelain"], repo_root)?;
    Ok(parse_worktree_porcelain(&raw))
}

fn parse_worktree_porcelain(raw: &str) -> Vec<WorktreePorcelain> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreePorcelain> = None;
    for line in raw.split('\n') {
        if let Some(p) = line.strip_prefix("worktree ") {
            entries.extend(current.take());
            current = Some(WorktreePorcelain { path: PathBuf::from(p.trim()), head: String::new(), branch: None });
        } else if let Some(cur) = current.as_mut() {
            if let Some(h) = line.strip_prefix("HEAD ") {
                cur.head = h.trim().to_string();
            } else if let Some(r) = line.strip_prefix("branch ") {
                let r = r.trim();
                cur.branch = Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string());
            }
        }
    }
    entries.extend(current);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain() {
        let raw = "worktree /r\nHEAD abc\nbranch refs/heads/main\n\nworktree /r/.grove/feat/x\nHEAD def\ndetached\n\n";
        let w = parse_worktree_porcelain(raw);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].branch.as_deref(), Some("main"));
        assert_eq!(w[0].head, "abc");
        assert_eq!(w[1].path, PathBuf::from("/r/.grove/feat/x"));
        assert_eq!(w[1].branch, None);
    }
}
