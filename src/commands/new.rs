use std::path::{Path, PathBuf};

use crate::args::{self, HELP, flag, value};
use crate::claude_settings::{ensure_status_line, seed_local_settings};
use crate::commands::{
    Launch, collect_single_git_dirs, effective_sandbox, launch_claude, link_shared, require_cwd_at_workspace_root,
};
use crate::error::{Result, bail};
use crate::git::{default_remote, fetch_remote};
use crate::paths;
use crate::post_create::run_post_create_commands;
use crate::project::{ensure_grove_ignored, load_project};
use crate::sandbox::resolve_worktree_git_dir;
use crate::status_line::{single_status_line_text, workspace_status_line_text};
use crate::term::{self, stdout};
use crate::types::{ClaudeConfig, Project, SandboxConfig, SingleProject, WorkspaceProject, WorkspaceRepo};
use crate::worktree::{
    AddMode, ParsedSlug, add_workspace_worktree, add_worktree, parse_slug, repo_abs_path, rollback_workspace_worktree,
    rollback_worktree, workspace_worktree_dir,
};

const USAGE: &str = "\
grove new — create a worktree and launch a sandboxed Claude session

Usage:
  grove new <slug> [--from <branch>] [--fetch] [--no-sandbox] [--keep-on-failure] [--dry-run] [-- <claude-args>…]

Arguments:
  <slug>                 Branch name. Must start with feat/, fix/, or chore/.

If a branch matching <slug> already exists it is reused: a local branch is checked
out as-is, otherwise a local tracking branch is created from origin/<slug>. --from is
only consulted when a brand-new branch has to be created.

Flags:
  --from <branch>        Base branch to fork from. Defaults to .groverc baseBranch / current HEAD.
                         In workspace mode, overrides every repo's baseBranch.
                         Ignored when an existing branch is reused/adopted.
  --fetch                git fetch the default remote first, so base and origin/<slug> are current.
  --no-sandbox           Skip the sandbox. Launches claude in the worktree directly.
  --keep-on-failure      On postCreateCommand failure, leave the worktree in place.
  --dry-run              Print the planned actions and exit before any side effects.
  --                     Stop flag parsing; remaining args are passed to claude.
";

struct NewInput {
    slug: ParsedSlug,
    from_override: Option<String>,
    fetch: bool,
    sandbox: SandboxConfig,
    passthrough: Vec<String>,
    keep_on_failure: bool,
    dry_run: bool,
}

pub fn run(argv: &[String]) -> Result<i32> {
    let (argv, passthrough) = args::split_passthrough(argv);
    let p = args::parse(
        &argv,
        &[HELP, value("from"), flag("fetch"), flag("no-sandbox"), flag("keep-on-failure"), flag("dry-run")],
        true,
    )?;
    if p.flag("help") {
        stdout!("{USAGE}");
        return Ok(0);
    }

    match p.positionals.as_slice() {
        [] => bail!("Missing slug. Usage: grove new <slug>"),
        [_] => {}
        many => bail!("Too many positionals: {}", many.join(" ")),
    }

    let slug = parse_slug(&p.positionals[0])?;
    let project = load_project()?;
    let input = NewInput {
        slug,
        from_override: p.value("from"),
        fetch: p.flag("fetch"),
        sandbox: effective_sandbox(project.sandbox(), p.flag("no-sandbox")),
        passthrough,
        keep_on_failure: p.flag("keep-on-failure"),
        dry_run: p.flag("dry-run"),
    };

    match &project {
        Project::Workspace(ws) => {
            require_cwd_at_workspace_root(&ws.workspace_root)?;
            run_workspace(ws, input)
        }
        Project::Single(single) => run_single(single, input),
    }
}

fn run_single(project: &SingleProject, input: NewInput) -> Result<i32> {
    let NewInput { slug, from_override, sandbox, passthrough, keep_on_failure, .. } = &input;
    let repo_root = &project.repo_root;
    let base_branch = from_override.clone().or_else(|| project.config.base_branch.clone());

    if input.dry_run {
        print_dry_run_single(project, &slug.full, base_branch.as_deref(), sandbox, passthrough);
        return Ok(0);
    }

    ensure_grove_ignored(repo_root)?;

    if input.fetch
        && let Some(remote) = default_remote(repo_root)
    {
        term::info(format_args!("fetching {remote}..."));
        fetch_remote(repo_root, &remote);
    }

    term::info(format_args!("creating worktree {}", slug.full));
    let added = add_worktree(repo_root, slug, base_branch.as_deref())?;
    let worktree_path = added.worktree_path;
    report_add_mode(&slug.full, added.mode, base_branch.as_deref(), from_override.as_deref());

    let config = &project.config;
    let setup = || -> Result<()> {
        if config.claude.inherit_local_settings {
            seed_settings(repo_root, &worktree_path)?;
        }
        setup_shared(repo_root, &worktree_path, &config.sandbox)?;
        if !config.post_create_commands.is_empty() {
            run_post_create_commands(&worktree_path, &config.post_create_commands)?;
        }
        Ok(())
    };
    if let Err(err) = setup() {
        handle_setup_failure(*keep_on_failure, &worktree_path, || {
            rollback_worktree(repo_root, slug, added.created_branch)
        });
        return Err(err);
    }

    ensure_status_line(&worktree_path, &single_status_line_text(&slug.full))?;
    launch_claude(
        Launch {
            claude: &config.claude,
            passthrough,
            worktree_path: &worktree_path,
            root_dir: repo_root,
            branch: &slug.full,
            workspace: false,
            git_dirs: collect_single_git_dirs(repo_root, &worktree_path),
            sandbox,
        },
        false,
    )
}

fn run_workspace(project: &WorkspaceProject, input: NewInput) -> Result<i32> {
    let NewInput { slug, from_override, sandbox, passthrough, keep_on_failure, .. } = &input;
    let workspace_root = &project.workspace_root;
    let config = &project.config;

    if input.dry_run {
        print_dry_run_workspace(project, &slug.full, from_override.as_deref(), sandbox, passthrough);
        return Ok(0);
    }

    ensure_grove_ignored(workspace_root)?;
    for repo in &config.repos {
        ensure_grove_ignored(&repo_abs_path(workspace_root, repo))?;
    }

    if input.fetch {
        for repo in &config.repos {
            let abs = repo_abs_path(workspace_root, repo);
            if let Some(remote) = default_remote(&abs) {
                term::info(format_args!("fetching {remote} in {}...", repo.name));
                fetch_remote(&abs, &remote);
            }
        }
    }

    term::info(format_args!("creating workspace worktree {} across {} repo(s)", slug.full, config.repos.len()));

    let added = add_workspace_worktree(workspace_root, slug, &config.repos, from_override.as_deref())?;
    let worktree_path = added.workspace_dir;
    for r in &added.per_repo {
        let base = from_override.as_deref().or(r.repo.base_branch.as_deref());
        report_add_mode(&format!("{} · {}", slug.full, r.repo.name), r.mode, base, from_override.as_deref());
    }

    let setup = || -> Result<()> {
        if config.claude.inherit_local_settings {
            seed_settings(workspace_root, &worktree_path)?;
        }
        setup_shared(workspace_root, &worktree_path, &config.sandbox)?;
        if !config.post_create_commands.is_empty() {
            run_post_create_commands(&worktree_path, &config.post_create_commands)?;
        }
        Ok(())
    };
    if let Err(err) = setup() {
        handle_setup_failure(*keep_on_failure, &worktree_path, || {
            rollback_workspace_worktree(workspace_root, slug, &added.per_repo)
        });
        return Err(err);
    }

    ensure_status_line(&worktree_path, &workspace_status_line_text(workspace_root, &slug.full))?;
    launch_claude(
        Launch {
            claude: &config.claude,
            passthrough,
            worktree_path: &worktree_path,
            root_dir: workspace_root,
            branch: &slug.full,
            workspace: true,
            git_dirs: collect_workspace_git_dirs(workspace_root, &config.repos, slug),
            sandbox,
        },
        false,
    )
}

fn handle_setup_failure(keep_on_failure: bool, worktree_path: &Path, rollback: impl FnOnce()) {
    if keep_on_failure {
        term::warn(format_args!(
            "setup failed but --keep-on-failure set; worktree left at {}",
            worktree_path.display()
        ));
    } else {
        term::fail("setup failed, rolling back...");
        rollback();
    }
}

fn report_add_mode(label: &str, mode: AddMode, base: Option<&str>, from_override: Option<&str>) {
    match mode {
        AddMode::Reuse => term::info(format_args!("reusing existing branch {label}")),
        AddMode::Adopt => term::info(format_args!("adopting remote branch for {label} (tracking origin)")),
        AddMode::Create => match base {
            Some(b) => term::info(format_args!("branched {label} from {b}")),
            None => term::info(format_args!("branched {label}")),
        },
    }
    if mode != AddMode::Create
        && let Some(from) = from_override
    {
        term::warn(format_args!("--from {from} ignored; {label} already exists"));
    }
}

fn seed_settings(root_dir: &Path, worktree_path: &Path) -> Result<()> {
    if seed_local_settings(root_dir, worktree_path)? {
        term::info("copied .claude/settings.local.json");
    }
    Ok(())
}

fn setup_shared(root_dir: &Path, worktree_path: &Path, sandbox: &SandboxConfig) -> Result<()> {
    let result = link_shared(root_dir, worktree_path, sandbox)?;
    for entry in &result.linked {
        term::info(format_args!("linked {entry}"));
    }
    for entry in &result.repaired {
        term::info(format_args!("repaired {entry} (removed stale empty mountpoint)"));
    }
    for (entry, reason) in &result.skipped {
        term::warn(format_args!("skipped {entry} ({reason})"));
    }
    Ok(())
}

fn collect_workspace_git_dirs(workspace_root: &Path, repos: &[WorkspaceRepo], slug: &ParsedSlug) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for repo in repos {
        dirs.push(repo_abs_path(workspace_root, repo).join(".git"));
        let sub_path = paths::join(&workspace_worktree_dir(workspace_root, slug), &repo.path);
        dirs.extend(resolve_worktree_git_dir(&sub_path));
    }
    dirs
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() { "(none)".into() } else { items.join(", ") }
}

fn claude_line(claude: &ClaudeConfig, passthrough: &[String]) -> String {
    let pass = if passthrough.is_empty() { String::new() } else { format!(" -- {}", passthrough.join(" ")) };
    format!("{} {}{pass}", claude.command, claude.extra_args.join(" "))
}

fn enabled(sandbox: &SandboxConfig) -> &'static str {
    if sandbox.enabled { "enabled" } else { "disabled" }
}

fn print_dry_run_single(
    project: &SingleProject,
    slug: &str,
    base_branch: Option<&str>,
    sandbox: &SandboxConfig,
    passthrough: &[String],
) {
    let root = project.repo_root.display();
    let lines = [
        "Mode:         single-repo".to_string(),
        format!("Repo root:    {root}"),
        format!("Config:       {}", project.config_source.as_str()),
        format!("Branch:       {slug}"),
        format!("From:         {}", base_branch.unwrap_or("(existing branch if present, else current HEAD)")),
        format!("Worktree:     {root}/.grove/{slug}"),
        format!("Sandbox:      {}", enabled(sandbox)),
        format!("Shared (ro):  {}", list_or_none(&sandbox.share_read_only)),
        format!("Shared (rw):  {}", list_or_none(&sandbox.share_read_write)),
        format!("Post-create:  {} command(s)", project.config.post_create_commands.len()),
        format!("Claude:       {}", claude_line(&project.config.claude, passthrough)),
    ];
    stdout!("{}\n", lines.join("\n"));
}

fn print_dry_run_workspace(
    project: &WorkspaceProject,
    slug: &str,
    from_override: Option<&str>,
    sandbox: &SandboxConfig,
    passthrough: &[String],
) {
    let root = project.workspace_root.display();
    let config = &project.config;
    let mut lines = vec![
        "Mode:         workspace".to_string(),
        format!("Workspace:    {root}"),
        format!("Branch:       {slug}"),
        format!("Worktree:     {root}/.grove/{slug}"),
        "Repos:".to_string(),
    ];
    for repo in &config.repos {
        let base = from_override.or(repo.base_branch.as_deref()).unwrap_or("(unset)");
        lines.push(format!("  • {} ({}) from {base}", repo.name, repo.path));
    }
    lines.push(format!("Sandbox:      {}", enabled(sandbox)));
    lines.push(format!("Shared (ro):  {}", list_or_none(&sandbox.share_read_only)));
    lines.push(format!("Shared (rw):  {}", list_or_none(&sandbox.share_read_write)));
    lines.push(format!("Post-create:  {} command(s)", config.post_create_commands.len()));
    lines.push(format!("Claude:       {}", claude_line(&config.claude, passthrough)));
    stdout!("{}\n", lines.join("\n"));
}
