use crate::args::{self, HELP, short_flag};
use crate::commands::require_cwd_at_workspace_root;
use crate::error::{Result, bail};
use crate::project::load_project;
use crate::term::stdout;
use crate::types::Project;
use crate::worktree::{
    find_grove_worktree, find_workspace_worktree, parse_slug, remove_workspace_worktree, remove_worktree,
};

const USAGE: &str = "\
grove rm — remove a Grove worktree and its branch

Usage:
  grove rm <slug> [--force]

Flags:
  --force                Discard local changes in the worktree (passes --force to git worktree remove).
";

pub fn run(argv: &[String]) -> Result<i32> {
    let p = args::parse(argv, &[HELP, short_flag("force", 'f')], true)?;
    if p.flag("help") {
        stdout!("{USAGE}");
        return Ok(0);
    }
    let [slug] = p.positionals.as_slice() else {
        bail!("Usage: grove rm <slug>");
    };

    let slug = parse_slug(slug)?;
    let force = p.flag("force");

    match load_project()? {
        Project::Workspace(ws) => {
            require_cwd_at_workspace_root(&ws.workspace_root)?;
            if find_workspace_worktree(&ws.workspace_root, &ws.config.repos, &slug).is_none() {
                bail!("No Grove worktree found for slug \"{}\"", slug.full);
            }
            remove_workspace_worktree(&ws.workspace_root, &slug, &ws.config.repos, force)?;
        }
        Project::Single(single) => {
            if find_grove_worktree(&single.repo_root, &slug)?.is_none() {
                bail!("No Grove worktree found for slug \"{}\"", slug.full);
            }
            remove_worktree(&single.repo_root, &slug, force, true)?;
        }
    }
    stdout!("Removed worktree {}\n", slug.full);
    Ok(0)
}
