use std::path::{Path, PathBuf};

use crate::args::{self, HELP, flag};
use crate::claude::has_claude_session;
use crate::claude_settings::ensure_status_line;
use crate::commands::{Launch, collect_single_git_dirs, effective_sandbox, launch_claude, link_shared};
use crate::error::{Result, bail};
use crate::project::load_project;
use crate::sandbox::resolve_worktree_git_dir;
use crate::status_line::{single_status_line_text, workspace_status_line_text};
use crate::term::{self, stderr, stdout};
use crate::types::{Project, WorkspaceRepo};
use crate::worktree::{
    WorkspaceWorktreeRow, find_grove_worktree, find_workspace_worktree, parse_slug, repo_abs_path,
    workspace_worktree_dir,
};

const USAGE: &str = "\
grove resume — re-enter an existing worktree and continue the last Claude session

Usage:
  grove resume <slug> [--no-sandbox] [-- <claude-args>…]

Flags:
  --no-sandbox           Skip the sandbox.
  --                     Stop flag parsing; remaining args are passed to claude.
";

pub fn run(argv: &[String]) -> Result<i32> {
    let (argv, passthrough) = args::split_passthrough(argv);
    let p = args::parse(&argv, &[HELP, flag("no-sandbox")], true)?;
    if p.flag("help") {
        stdout!("{USAGE}");
        return Ok(0);
    }
    let [slug] = p.positionals.as_slice() else {
        bail!("Usage: grove resume <slug>");
    };

    let slug = parse_slug(slug)?;
    let project = load_project()?;
    let sandbox = effective_sandbox(project.sandbox(), p.flag("no-sandbox"));

    let (root_dir, worktree_path, git_dirs) = match &project {
        Project::Workspace(ws) => {
            let Some(row) = find_workspace_worktree(&ws.workspace_root, &ws.config.repos, &slug) else {
                bail!("No Grove worktree found for slug \"{}\"", slug.full);
            };
            let worktree_path = workspace_worktree_dir(&ws.workspace_root, &slug);
            let git_dirs = collect_workspace_git_dirs(&ws.workspace_root, &ws.config.repos, &row);
            ensure_status_line(&worktree_path, &workspace_status_line_text(&ws.workspace_root, &slug.full))?;
            (ws.workspace_root.clone(), worktree_path, git_dirs)
        }
        Project::Single(single) => {
            let Some(wt) = find_grove_worktree(&single.repo_root, &slug)? else {
                bail!("No Grove worktree found for slug \"{}\"", slug.full);
            };
            let git_dirs = collect_single_git_dirs(&single.repo_root, &wt.path);
            ensure_status_line(&wt.path, &single_status_line_text(&slug.full))?;
            (single.repo_root.clone(), wt.path, git_dirs)
        }
    };

    // Re-assert the shared symlink chains before launching: this also heals
    // worktrees left with empty bind mountpoints by earlier grove versions.
    let shared = link_shared(&root_dir, &worktree_path, project.sandbox())?;
    for entry in shared.linked.iter().chain(&shared.repaired) {
        term::info(format_args!("relinked {entry}"));
    }

    let resume = has_claude_session(&worktree_path);
    if !resume {
        stderr!("No previous Claude session in {} — starting a new one.\n", worktree_path.display());
    }

    launch_claude(
        Launch {
            claude: project.claude(),
            passthrough: &passthrough,
            worktree_path: &worktree_path,
            root_dir: &root_dir,
            branch: &slug.full,
            workspace: matches!(project, Project::Workspace(_)),
            git_dirs,
            sandbox: &sandbox,
        },
        resume,
    )
}

fn collect_workspace_git_dirs(
    workspace_root: &Path,
    repos: &[WorkspaceRepo],
    row: &WorkspaceWorktreeRow,
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = repos.iter().map(|r| repo_abs_path(workspace_root, r).join(".git")).collect();
    for r in row.per_repo.iter().filter(|r| r.registered) {
        dirs.extend(resolve_worktree_git_dir(&r.path));
    }
    dirs
}
