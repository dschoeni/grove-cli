use std::path::Path;

use crate::args::{self, HELP, flag, value};
use crate::commands::require_cwd_at_workspace_root;
use crate::error::{Result, bail};
use crate::git::{
    ahead_behind, branch_remote, default_remote, fetch_branch, git, rev_exists, upstream_of, working_tree_dirty,
};
use crate::project::load_project;
use crate::term::{GRAY, GREEN, RED, RESET, YELLOW, stdout};
use crate::types::Project;
use crate::worktree::{find_grove_worktree, find_workspace_worktree, parse_slug};

const USAGE: &str = "\
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
";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    UpToDate,
    FastForwarded,
    Reset,
    Ahead,
    Diverged,
    NoUpstream,
    NoRemote,
    Error,
}

impl Status {
    fn tag(self) -> String {
        let (color, label) = match self {
            Status::UpToDate => (GREEN, "up-to-date"),
            Status::FastForwarded => (GREEN, "fast-forwarded"),
            Status::Reset => (YELLOW, "reset"),
            Status::Ahead => (YELLOW, "ahead"),
            Status::Diverged => (RED, "diverged"),
            Status::NoUpstream => (GRAY, "no-upstream"),
            Status::NoRemote => (GRAY, "no-remote"),
            Status::Error => (RED, "error"),
        };
        format!("{color}{label}{RESET}")
    }

    fn is_failure(self) -> bool {
        matches!(self, Status::Diverged | Status::Error)
    }
}

struct SyncResult {
    status: Status,
    detail: String,
}

fn result(status: Status, detail: impl Into<String>) -> SyncResult {
    SyncResult { status, detail: detail.into() }
}

struct SyncOpts {
    hard: bool,
    dry_run: bool,
    remote_override: Option<String>,
}

pub fn run(argv: &[String]) -> Result<i32> {
    let p = args::parse(argv, &[HELP, flag("hard"), value("remote"), flag("dry-run")], true)?;
    if p.flag("help") {
        stdout!("{USAGE}");
        return Ok(0);
    }
    let [slug] = p.positionals.as_slice() else {
        bail!("Usage: grove sync <slug>");
    };

    let slug = parse_slug(slug)?;
    let project = load_project()?;
    let opts = SyncOpts { hard: p.flag("hard"), dry_run: p.flag("dry-run"), remote_override: p.value("remote") };

    match &project {
        Project::Workspace(ws) => {
            require_cwd_at_workspace_root(&ws.workspace_root)?;
            let Some(row) = find_workspace_worktree(&ws.workspace_root, &ws.config.repos, &slug) else {
                bail!("No Grove worktree found for slug \"{}\"", slug.full);
            };
            let mut failed = false;
            for r in row.per_repo.iter().filter(|r| r.registered) {
                let branch = r.branch.as_deref().unwrap_or(&slug.full);
                let res = safe_sync(&r.path, branch, &opts);
                report(&r.repo.name, &res);
                failed |= res.status.is_failure();
            }
            Ok(i32::from(failed))
        }
        Project::Single(single) => {
            let Some(wt) = find_grove_worktree(&single.repo_root, &slug)? else {
                bail!("No Grove worktree found for slug \"{}\"", slug.full);
            };
            let branch = wt.branch.as_deref().unwrap_or(&slug.full);
            let res = safe_sync(&wt.path, branch, &opts);
            report(&slug.full, &res);
            Ok(i32::from(res.status.is_failure()))
        }
    }
}

fn safe_sync(cwd: &Path, branch: &str, opts: &SyncOpts) -> SyncResult {
    sync_worktree(cwd, branch, opts).unwrap_or_else(|e| result(Status::Error, e.message()))
}

fn sync_worktree(cwd: &Path, branch: &str, opts: &SyncOpts) -> Result<SyncResult> {
    let remote = opts.remote_override.clone().or_else(|| branch_remote(cwd, branch)).or_else(|| default_remote(cwd));
    let Some(remote) = remote else {
        return Ok(result(Status::NoRemote, "repo has no remote"));
    };

    let target = upstream_of(cwd, branch).unwrap_or_else(|| format!("{remote}/{branch}"));

    fetch_branch(cwd, &remote, branch); // best-effort; a stale target is still handled below
    if !rev_exists(cwd, &target) {
        return Ok(result(Status::NoUpstream, format!("{target} not found on {remote}")));
    }

    let (ahead, behind) = ahead_behind(cwd, branch, &target);

    if behind == 0 && ahead == 0 {
        return Ok(result(Status::UpToDate, format!("even with {target}")));
    }

    // Fast-forward: remote strictly ahead, nothing local to lose.
    if behind > 0 && ahead == 0 {
        if opts.dry_run {
            return Ok(result(Status::FastForwarded, format!("would fast-forward {behind} commit(s) from {target}")));
        }
        git(["merge", "--ff-only", &target], cwd)?;
        return Ok(result(Status::FastForwarded, format!("fast-forwarded {behind} commit(s) from {target}")));
    }

    // Diverged (force-push) or purely ahead: only touch it under --hard.
    let diverged_detail = if behind > 0 {
        format!("diverged (ahead {ahead}, behind {behind})")
    } else {
        format!("{ahead} local commit(s) not on {remote}")
    };

    if !opts.hard {
        let status = if behind > 0 { Status::Diverged } else { Status::Ahead };
        return Ok(result(status, format!("{diverged_detail}; re-run with --hard to reset to {target}")));
    }

    if opts.dry_run {
        return Ok(result(Status::Reset, format!("would reset --hard to {target} ({diverged_detail})")));
    }
    let dirty = working_tree_dirty(cwd);
    git(["reset", "--hard", &target], cwd)?;
    let discarded = if dirty { ", discarded uncommitted changes" } else { "" };
    Ok(result(Status::Reset, format!("reset to {target} ({diverged_detail}{discarded})")))
}

fn report(label: &str, res: &SyncResult) {
    stdout!("{label}: {} — {}\n", res.status.tag(), res.detail);
}
