use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::args::{self, HELP, flag};
use crate::claude::claude_project_dir;
use crate::commands::require_cwd_at_workspace_root;
use crate::error::{Result, bail};
use crate::git::{git, prune_worktrees};
use crate::paths;
use crate::project::load_project;
use crate::term::{self, GREEN, RED, RESET, stdout};
use crate::types::{Layout, Project};
use crate::worktree::{
    list_grove_worktrees, list_workspace_worktrees, parse_slug, prune_empty_parents, repo_abs_path, slug_of,
    worktree_path_for,
};

const USAGE: &str = "\
grove migrate — move worktrees from <repo>/.grove to the worktreeRoot location

Usage:
  grove migrate <slug|all> [--dry-run]

Moves worktrees still under the repo-local .grove/ to where worktreeRoot
(.groverc) or $GROVE_WORKTREE_ROOT now puts them, e.g. ~/.grove/<project>/.
For each worktree it:
  • moves the directory, then runs `git worktree repair` so git follows it;
  • moves Claude's session history along, so `grove resume` still continues;
  • removes the repo-local .grove/ dirs it leaves empty.

Make sure no Claude session (or anything else) is running in a worktree
while it is moved. Tools that baked the old absolute path in (e.g. a Python
virtualenv) may need their postCreateCommands re-run.

Flags:
  --dry-run              Print the planned moves without changing anything.
";

/// One worktree dir to move, plus the git worktrees inside it.
struct Move {
    slug: String,
    src: PathBuf,
    dst: PathBuf,
    /// `(repo to run git in, worktree path after the move)` per git worktree.
    repairs: Vec<(PathBuf, PathBuf)>,
}

pub fn run(argv: &[String]) -> Result<i32> {
    let p = args::parse(argv, &[HELP, flag("dry-run")], true)?;
    if p.flag("help") {
        stdout!("{USAGE}");
        return Ok(0);
    }
    let [target] = p.positionals.as_slice() else {
        bail!("Usage: grove migrate <slug|all>");
    };
    let only = if target == "all" { None } else { Some(parse_slug(target)?.full) };

    let project = load_project()?;
    let layout = match &project {
        Project::Single(s) => &s.layout,
        Project::Workspace(ws) => &ws.layout,
    };
    let Some(legacy) = &layout.legacy else {
        bail!(
            "Worktrees already live in {} (worktreeRoot is unset or \"repo\"). \
             Set \"worktreeRoot\": \"system\" in .groverc first.",
            layout.dir.display()
        );
    };

    let mut moves = match &project {
        Project::Single(single) => {
            prune_worktrees(&single.repo_root);
            list_grove_worktrees(&single.repo_root, layout)?
                .into_iter()
                .filter(|w| w.path.starts_with(legacy))
                .map(|w| {
                    let slug = slug_of(layout, &w.path);
                    let dst = paths_for(layout, &slug)?;
                    Ok(Move { repairs: vec![(single.repo_root.clone(), dst.clone())], slug, src: w.path, dst })
                })
                .collect::<Result<Vec<_>>>()?
        }
        Project::Workspace(ws) => {
            require_cwd_at_workspace_root(&ws.workspace_root)?;
            for repo in &ws.config.repos {
                prune_worktrees(&repo_abs_path(&ws.workspace_root, repo));
            }
            list_workspace_worktrees(&ws.workspace_root, layout, &ws.config.repos)
                .into_iter()
                .filter(|row| row.workspace_dir.starts_with(legacy))
                .map(|row| {
                    let dst = paths_for(layout, &row.slug)?;
                    let repairs = row
                        .per_repo
                        .iter()
                        .filter(|r| r.registered)
                        .map(|r| (repo_abs_path(&ws.workspace_root, &r.repo), paths::join(&dst, &r.repo.path)))
                        .collect();
                    Ok(Move { slug: row.slug, src: row.workspace_dir, dst, repairs })
                })
                .collect::<Result<Vec<_>>>()?
        }
    };

    if let Some(slug) = &only {
        moves.retain(|m| &m.slug == slug);
        if moves.is_empty() {
            bail!("No worktree for \"{slug}\" under {}", legacy.display());
        }
    }
    if moves.is_empty() {
        stdout!("Nothing to migrate: no worktrees under {}.\n", legacy.display());
        return Ok(0);
    }

    if p.flag("dry-run") {
        for m in &moves {
            stdout!("{}: would move {} → {}\n", m.slug, m.src.display(), m.dst.display());
        }
        return Ok(0);
    }

    let mut failed = false;
    for m in &moves {
        match migrate_one(layout, m) {
            Ok(()) => stdout!("{}: {GREEN}moved{RESET} → {}\n", m.slug, m.dst.display()),
            Err(e) => {
                stdout!("{}: {RED}error{RESET} — {}\n", m.slug, e.message());
                failed = true;
            }
        }
    }
    term::info(
        "if a postCreateCommand baked in absolute paths (e.g. a Python virtualenv), re-run it in the moved worktree",
    );
    Ok(i32::from(failed))
}

fn paths_for(layout: &Layout, slug: &str) -> Result<PathBuf> {
    Ok(worktree_path_for(&layout.dir, &parse_slug(slug)?))
}

fn migrate_one(layout: &Layout, m: &Move) -> Result<()> {
    if m.dst.exists() {
        bail!("{} already exists; remove it or migrate this worktree by hand", m.dst.display());
    }
    // Resolve before moving: the encoding realpaths the directory.
    let old_session = claude_project_dir(&m.src);

    if let Some(parent) = m.dst.parent() {
        fs::create_dir_all(parent)?;
    }
    move_dir(&m.src, &m.dst)?;

    for (repo, wt) in &m.repairs {
        if let Err(e) = git(["worktree".as_ref(), "repair".as_ref(), wt.as_os_str()], repo) {
            bail!(
                "moved, but `git worktree repair` failed: {} — run `git -C {} worktree repair {}`",
                e.message(),
                repo.display(),
                wt.display()
            );
        }
    }

    let new_session = claude_project_dir(&m.dst);
    if old_session.is_dir() {
        if new_session.exists() {
            term::warn(format_args!(
                "{}: Claude history already exists for the new path; left the old one at {}",
                m.slug,
                old_session.display()
            ));
        } else {
            fs::rename(&old_session, &new_session)?;
        }
    }

    prune_empty_parents(layout, &m.src);
    Ok(())
}

/// `rename`, falling back to `mv` when the target is on another filesystem
/// (e.g. the repo lives on a different disk than `$HOME`).
fn move_dir(src: &Path, dst: &Path) -> Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices && cfg!(unix) => {
            let status = std::process::Command::new("mv").arg(src).arg(dst).status()?;
            if !status.success() {
                bail!("mv {} {} failed", src.display(), dst.display());
            }
            Ok(())
        }
        Err(e) => bail!("could not move {} to {}: {e}", src.display(), dst.display()),
    }
}
