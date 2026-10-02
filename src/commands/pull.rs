use std::path::Path;

use crate::args::{self, HELP, value};
use crate::commands::require_cwd_at_workspace_root;
use crate::error::{Result, bail};
use crate::git::{git_interactive, list_worktrees, remote_for_branch, short_sha};
use crate::project::load_project;
use crate::term::{self, GREEN, RESET, stdout};
use crate::types::{Project, WorkspaceProject};
use crate::worktree::repo_abs_path;

const USAGE: &str = "\
grove pull — fast-forward the base branch to its latest remote state

Usage:
  grove pull [branch] [--remote <name>]

Arguments:
  [branch]               Branch to update. Defaults to the .groverc baseBranch
                         (per-repo baseBranch in workspace mode).

Flags:
  --remote <name>        Remote to pull from. Defaults to the branch's upstream
                         remote, or \"origin\".

Updates are fast-forward only; a diverged branch is reported, not merged. When
the branch is checked out in a worktree it is fast-forwarded in place,
otherwise its ref is advanced directly.
";

pub fn run(argv: &[String]) -> Result<i32> {
    let p = args::parse(argv, &[HELP, value("remote")], true)?;
    if p.flag("help") {
        stdout!("{USAGE}");
        return Ok(0);
    }
    if p.positionals.len() > 1 {
        bail!("Too many positionals: {}", p.positionals.join(" "));
    }

    let branch_override = p.positionals.first().cloned();
    let remote_override = p.value("remote");

    match load_project()? {
        Project::Workspace(ws) => run_workspace(&ws, branch_override.as_deref(), remote_override.as_deref())?,
        Project::Single(single) => {
            let Some(branch) = branch_override.or(single.config.base_branch) else {
                bail!("Could not determine base branch. Pass a branch name or set baseBranch in .groverc.");
            };
            pull_branch(&single.repo_root, &branch, remote_override.as_deref())?;
        }
    }
    Ok(0)
}

fn run_workspace(ws: &WorkspaceProject, branch_override: Option<&str>, remote_override: Option<&str>) -> Result<()> {
    require_cwd_at_workspace_root(&ws.workspace_root)?;
    for repo in &ws.config.repos {
        let Some(branch) = branch_override.or(repo.base_branch.as_deref()) else {
            term::warn(format_args!("{}: no baseBranch configured, skipping", repo.name));
            continue;
        };
        term::info(format_args!("{} ({})", repo.name, repo.path));
        pull_branch(&repo_abs_path(&ws.workspace_root, repo), branch, remote_override)?;
    }
    Ok(())
}

/// Fetch and fast-forward `branch` in the repo at `repo_root`.
fn pull_branch(repo_root: &Path, branch: &str, remote_override: Option<&str>) -> Result<()> {
    let Some(remote) = remote_override.map(str::to_string).or_else(|| remote_for_branch(repo_root, branch)) else {
        bail!("No git remote configured to pull \"{branch}\" from.");
    };

    let before = short_sha(repo_root, branch);
    let checkout = list_worktrees(repo_root)?.into_iter().find(|w| w.branch.as_deref() == Some(branch));

    if let Some(checkout) = checkout {
        // Branch is checked out somewhere: fetch the remote-tracking ref, then
        // fast-forward the working tree in place.
        git_interactive(["fetch", &remote, branch], repo_root)?;
        git_interactive(["merge", "--ff-only", &format!("{remote}/{branch}")], &checkout.path)?;
    } else {
        // Not checked out: a refspec fetch advances the local ref (fast-forward only).
        git_interactive(["fetch", &remote, &format!("{branch}:{branch}")], repo_root)?;
    }

    let after = short_sha(repo_root, branch);
    match (before, after) {
        (Some(b), Some(a)) if b == a => stdout!("{GREEN}✓{RESET} {branch} already up to date ({a})\n"),
        (before, after) => stdout!(
            "{GREEN}✓{RESET} {branch} {} → {}\n",
            before.as_deref().unwrap_or("(new)"),
            after.as_deref().unwrap_or("(missing)")
        ),
    }
    Ok(())
}
