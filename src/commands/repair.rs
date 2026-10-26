use std::collections::HashSet;
use std::path::Path;

use crate::args::{self, HELP, flag, value};
use crate::commands::{link_shared, report_add_mode, require_cwd_at_workspace_root};
use crate::error::{Result, bail};
use crate::git::{WorktreePorcelain, default_remote, fetch_remote, prune_worktrees};
use crate::paths;
use crate::project::{ensure_grove_ignored, load_project};
use crate::term::{self, GREEN, RED, RESET, YELLOW, stdout};
use crate::types::{Project, SandboxConfig, SingleProject, WorkspaceProject};
use crate::worktree::{
    WorkspaceWorktreeRow, add_repo_worktree, find_grove_worktree, list_grove_worktrees, list_workspace_worktrees,
    parse_slug, repo_abs_path,
};

const USAGE: &str = "\
grove repair — bring existing worktrees back in line with .groverc

Usage:
  grove repair [slug] [--from <branch>] [--fetch] [--dry-run]

Without a slug every Grove worktree in the project is repaired.

Per worktree:
  • workspace mode: adds a worktree for every repo listed in .groverc that the
    worktree is missing (e.g. a repo added to the workspace after `grove new`),
    reusing / adopting / creating the branch exactly like `grove new`;
  • re-links shareReadOnly / shareReadWrite entries (picks up new ones);
  • ensures /.grove/ is in each repo's .git/info/exclude and prunes stale
    worktree registrations.

postCreateCommands are not re-run; run them yourself in the worktree if the
added repo needs it.

Flags:
  --from <branch>        Base branch for repos that need a brand-new branch.
                         Overrides each repo's baseBranch.
  --fetch                git fetch each affected repo's default remote first.
  --dry-run              Report the missing repos without changing anything.
";

struct RepairOpts {
    from_override: Option<String>,
    fetch: bool,
    dry_run: bool,
}

pub fn run(argv: &[String]) -> Result<i32> {
    let p = args::parse(argv, &[HELP, value("from"), flag("fetch"), flag("dry-run")], true)?;
    if p.flag("help") {
        stdout!("{USAGE}");
        return Ok(0);
    }
    let slug = match p.positionals.as_slice() {
        [] => None,
        [slug] => Some(parse_slug(slug)?),
        many => bail!("Too many positionals: {}", many.join(" ")),
    };
    let opts = RepairOpts { from_override: p.value("from"), fetch: p.flag("fetch"), dry_run: p.flag("dry-run") };

    match load_project()? {
        Project::Workspace(ws) => {
            require_cwd_at_workspace_root(&ws.workspace_root)?;
            // Prune first so a worktree dir deleted by hand shows up as missing (and gets re-added).
            for repo in &ws.config.repos {
                prune_worktrees(&repo_abs_path(&ws.workspace_root, repo));
            }
            let mut rows = list_workspace_worktrees(&ws.workspace_root, &ws.config.repos);
            if let Some(slug) = &slug {
                rows.retain(|r| r.slug == slug.full);
                if rows.is_empty() {
                    bail!("No Grove worktree found for slug \"{}\"", slug.full);
                }
            }
            repair_workspace(&ws, &rows, &opts)
        }
        Project::Single(single) => {
            if opts.from_override.is_some() || opts.fetch {
                term::warn("--from / --fetch only apply in workspace mode; ignoring");
            }
            prune_worktrees(&single.repo_root);
            let worktrees = match &slug {
                Some(slug) => {
                    let Some(wt) = find_grove_worktree(&single.repo_root, slug)? else {
                        bail!("No Grove worktree found for slug \"{}\"", slug.full);
                    };
                    vec![wt]
                }
                None => list_grove_worktrees(&single.repo_root)?,
            };
            repair_single(&single, &worktrees, &opts)
        }
    }
}

fn repair_workspace(project: &WorkspaceProject, rows: &[WorkspaceWorktreeRow], opts: &RepairOpts) -> Result<i32> {
    let workspace_root = &project.workspace_root;
    let config = &project.config;
    if rows.is_empty() {
        stdout!("No Grove worktrees to repair.\n");
        return Ok(0);
    }

    if opts.dry_run {
        for row in rows {
            let missing: Vec<_> = row.per_repo.iter().filter(|r| !r.registered).collect();
            if missing.is_empty() {
                stdout!("{}: {GREEN}complete{RESET}\n", row.slug);
            }
            for r in missing {
                let base = opts.from_override.as_deref().or(r.repo.base_branch.as_deref()).unwrap_or("(unset)");
                stdout!(
                    "{} · {}: {YELLOW}missing{RESET} — would add {} (new branches from {base})\n",
                    row.slug,
                    r.repo.name,
                    r.path.display()
                );
            }
        }
        return Ok(0);
    }

    ensure_grove_ignored(workspace_root)?;
    for repo in &config.repos {
        ensure_grove_ignored(&repo_abs_path(workspace_root, repo))?;
    }

    if opts.fetch {
        let affected: HashSet<&str> = rows
            .iter()
            .flat_map(|row| row.per_repo.iter().filter(|r| !r.registered).map(|r| r.repo.name.as_str()))
            .collect();
        for repo in config.repos.iter().filter(|r| affected.contains(r.name.as_str())) {
            let abs = repo_abs_path(workspace_root, repo);
            if let Some(remote) = default_remote(&abs) {
                term::info(format_args!("fetching {remote} in {}...", repo.name));
                fetch_remote(&abs, &remote);
            }
        }
    }

    let mut failed = false;
    for row in rows {
        // Only reached via list_workspace_worktrees, which yields valid `<type>/<name>` dirs.
        let slug = parse_slug(&row.slug)?;
        let missing: Vec<_> = row.per_repo.iter().filter(|r| !r.registered).collect();
        if missing.is_empty() {
            stdout!("{}: {GREEN}complete{RESET}\n", row.slug);
        }
        for r in missing {
            let label = format!("{} · {}", row.slug, r.repo.name);
            if r.path.exists() && !paths::is_empty_dir(&r.path) {
                report_error(
                    &label,
                    format!("{} exists but is not a registered worktree; remove it and re-run", r.path.display()),
                );
                failed = true;
                continue;
            }
            match add_repo_worktree(workspace_root, &slug, &r.repo, opts.from_override.as_deref()) {
                Ok(res) => {
                    let base = opts.from_override.as_deref().or(r.repo.base_branch.as_deref());
                    report_add_mode(&label, res.mode, base, opts.from_override.as_deref());
                    stdout!("{label}: {GREEN}added{RESET} — {}\n", r.path.display());
                }
                Err(e) => {
                    report_error(&label, e.message());
                    failed = true;
                }
            }
        }
        relink(workspace_root, &row.workspace_dir, &config.sandbox)?;
    }
    Ok(i32::from(failed))
}

fn repair_single(project: &SingleProject, worktrees: &[WorktreePorcelain], opts: &RepairOpts) -> Result<i32> {
    let repo_root = &project.repo_root;
    if worktrees.is_empty() {
        stdout!("No Grove worktrees to repair.\n");
        return Ok(0);
    }
    if opts.dry_run {
        for wt in worktrees {
            stdout!("{}: would re-link shared entries\n", wt.path.display());
        }
        return Ok(0);
    }

    ensure_grove_ignored(repo_root)?;
    for wt in worktrees {
        relink(repo_root, &wt.path, &project.config.sandbox)?;
        stdout!("{}: {GREEN}ok{RESET}\n", wt.path.display());
    }
    Ok(0)
}

fn relink(root_dir: &Path, worktree_path: &Path, sandbox: &SandboxConfig) -> Result<()> {
    let shared = link_shared(root_dir, worktree_path, sandbox)?;
    for entry in shared.linked.iter().chain(&shared.repaired) {
        term::info(format_args!("relinked {entry} in {}", worktree_path.display()));
    }
    Ok(())
}

fn report_error(label: &str, detail: impl std::fmt::Display) {
    stdout!("{label}: {RED}error{RESET} — {detail}\n");
}
