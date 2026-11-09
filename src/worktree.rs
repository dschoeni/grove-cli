use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result, bail};
use crate::git::{
    WorktreePorcelain, branch_checked_out_at, branch_exists, default_remote, git, list_worktrees, prune_worktrees,
    remote_branch_exists, rev_exists,
};
use crate::paths;
use crate::types::{BRANCH_TYPES, Layout, WorkspaceRepo};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSlug {
    /// Full slug as the user supplied it, e.g. "feat/auth-flow". Used as branch name.
    pub full: String,
    pub kind: &'static str,
    /// Tail after the type prefix, e.g. "auth-flow".
    pub name: String,
}

pub fn parse_slug(input: &str) -> Result<ParsedSlug> {
    let slug = input.trim();
    if slug.is_empty() {
        bail!("Slug cannot be empty");
    }
    let Some((prefix, rest)) = slug.split_once('/') else {
        let types: Vec<String> = BRANCH_TYPES.iter().map(|t| format!("{t}/")).collect();
        bail!("Slug must start with one of: {}", types.join(", "));
    };
    let Some(kind) = BRANCH_TYPES.iter().copied().find(|t| *t == prefix) else {
        bail!("Slug prefix must be one of: {} (got \"{prefix}\")", BRANCH_TYPES.join(", "));
    };
    if rest.is_empty() {
        bail!("Slug needs a name after \"{prefix}/\"");
    }
    if !valid_slug_name(rest) {
        bail!("Slug name \"{rest}\" contains invalid characters");
    }
    Ok(ParsedSlug { full: slug.to_string(), kind, name: rest.to_string() })
}

/// `[A-Za-z0-9][A-Za-z0-9._-/]*`, with no `..` path segment and none of `~^:?*\[`.
fn valid_slug_name(name: &str) -> bool {
    let mut chars = name.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'));
    let forbidden = name.split('/').any(|seg| seg == "..") || name.contains(['~', '^', ':', '?', '*', '\\', '[']);
    first_ok && rest_ok && !forbidden
}

/// `<grove_dir>/<type>/<name>`.
pub fn worktree_path_for(grove_dir: &Path, slug: &ParsedSlug) -> PathBuf {
    paths::join(&grove_dir.join(slug.kind), &slug.name)
}

/// The grove dir of `layout` that `worktree_path` lives under, if any.
fn containing_dir<'a>(layout: &'a Layout, worktree_path: &Path) -> Option<&'a Path> {
    layout.dirs().find(|dir| {
        let prefix = format!("{}{}", dir.display(), std::path::MAIN_SEPARATOR);
        worktree_path.to_string_lossy().starts_with(&prefix)
    })
}

pub fn is_grove_worktree(layout: &Layout, worktree_path: &Path) -> bool {
    containing_dir(layout, worktree_path).is_some()
}

/// The `<type>/<name>` slug of a grove worktree path.
pub fn slug_of(layout: &Layout, worktree_path: &Path) -> String {
    let dir = containing_dir(layout, worktree_path).unwrap_or(&layout.dir);
    paths::relative(dir, worktree_path).display().to_string()
}

pub fn find_grove_worktree(repo_root: &Path, layout: &Layout, slug: &ParsedSlug) -> Result<Option<WorktreePorcelain>> {
    let worktrees = list_worktrees(repo_root)?;
    Ok(layout.dirs().map(|d| worktree_path_for(d, slug)).find_map(|t| worktrees.iter().find(|w| w.path == t).cloned()))
}

pub fn list_grove_worktrees(repo_root: &Path, layout: &Layout) -> Result<Vec<WorktreePorcelain>> {
    Ok(list_worktrees(repo_root)?.into_iter().filter(|w| is_grove_worktree(layout, &w.path)).collect())
}

/// Remove now-empty dirs from `path`'s parent up to and including its grove
/// dir, so `rm` leaves no empty `<type>/` (or per-project) dirs behind.
fn prune_empty_parents(layout: &Layout, path: &Path) {
    let Some(stop) = containing_dir(layout, path) else {
        return;
    };
    let mut dir = path.parent();
    while let Some(d) = dir {
        if !d.starts_with(stop) || !paths::is_empty_dir(d) || fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

/// How `grove new` obtained the branch for a worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddMode {
    Reuse,
    Adopt,
    Create,
}

struct BranchPlan {
    /// git args after `worktree add`.
    args: Vec<OsString>,
    /// True when grove created the branch (so rollback may safely delete it).
    created_branch: bool,
    mode: AddMode,
}

/// Decide how to check the slug's branch into `worktree_path`, without mutating:
///  - reuse  — a local branch already exists → check it out (base ignored).
///  - adopt  — only `<remote>/<slug>` exists → create a local tracking branch.
///  - create — neither exists → branch off `base_branch`.
///
/// Fails with a friendly error if the branch is already checked out elsewhere
/// or if a fresh branch is needed but the base is missing/unresolvable.
fn plan_branch(
    repo_cwd: &Path,
    slug: &ParsedSlug,
    worktree_path: &Path,
    base_branch: Option<&str>,
) -> Result<BranchPlan> {
    let branch = slug.full.as_str();

    if let Some(at) = branch_checked_out_at(repo_cwd, branch)? {
        bail!(
            "Branch \"{branch}\" is already checked out at {}. \
             Use `grove resume {branch}` to re-enter it, or `grove rm {branch}` first.",
            at.display()
        );
    }

    let wt: OsString = worktree_path.into();
    if branch_exists(repo_cwd, branch) {
        return Ok(BranchPlan { args: vec![wt, branch.into()], created_branch: false, mode: AddMode::Reuse });
    }

    if let Some(remote) = default_remote(repo_cwd)
        && remote_branch_exists(repo_cwd, &remote, branch)
    {
        return Ok(BranchPlan {
            args: vec!["-b".into(), branch.into(), wt, format!("{remote}/{branch}").into()],
            created_branch: true,
            mode: AddMode::Adopt,
        });
    }

    let Some(base) = base_branch else {
        bail!("No base branch to create \"{branch}\" from. Pass --from or set baseBranch in .groverc.");
    };
    if !rev_exists(repo_cwd, base) {
        bail!(
            "Base branch \"{base}\" does not resolve. \
             Fetch it first (`grove new {branch} --fetch`) or pass an existing --from <branch>."
        );
    }
    Ok(BranchPlan {
        args: vec!["-b".into(), branch.into(), wt, base.into()],
        created_branch: true,
        mode: AddMode::Create,
    })
}

fn worktree_add(repo_cwd: &Path, target: &Path, plan: &BranchPlan) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut args: Vec<OsString> = vec!["worktree".into(), "add".into()];
    args.extend(plan.args.iter().cloned());
    git(&args, repo_cwd)?;
    Ok(())
}

pub struct AddWorktreeResult {
    pub worktree_path: PathBuf,
    pub created_branch: bool,
    pub mode: AddMode,
}

pub fn add_worktree(
    repo_root: &Path,
    layout: &Layout,
    slug: &ParsedSlug,
    base_branch: Option<&str>,
) -> Result<AddWorktreeResult> {
    prune_worktrees(repo_root);

    let worktree_path = worktree_path_for(&layout.dir, slug);
    if let Some(existing) = find_grove_worktree(repo_root, layout, slug)? {
        bail!(
            "A worktree for \"{0}\" already exists at {1}. \
             Use `grove resume {0}` to re-enter it, or `grove rm {0}` to remove it.",
            slug.full,
            existing.path.display()
        );
    }
    if worktree_path.exists() {
        bail!(
            "Worktree path already exists: {}. \
             It is not a registered worktree — remove the leftover directory and retry.",
            worktree_path.display()
        );
    }

    let plan = plan_branch(repo_root, slug, &worktree_path, base_branch)?;
    worktree_add(repo_root, &worktree_path, &plan)?;
    Ok(AddWorktreeResult { worktree_path, created_branch: plan.created_branch, mode: plan.mode })
}

/// Remove the worktree and, when `delete_branch`, its branch (`-D`, so unmerged
/// work goes too). Prunes the now-empty `<type>/` dir.
pub fn remove_worktree(
    repo_root: &Path,
    layout: &Layout,
    worktree_path: &Path,
    slug: &ParsedSlug,
    force: bool,
    delete_branch: bool,
) -> Result<()> {
    let mut args: Vec<OsString> = vec!["worktree".into(), "remove".into()];
    if force {
        args.push("--force".into());
    }
    args.push(worktree_path.into());
    git(&args, repo_root)?;

    if delete_branch && branch_exists(repo_root, &slug.full) {
        git(["branch", "-D", &slug.full], repo_root)?;
    }

    prune_empty_parents(layout, worktree_path);
    Ok(())
}

pub fn rollback_worktree(
    repo_root: &Path,
    layout: &Layout,
    worktree_path: &Path,
    slug: &ParsedSlug,
    created_branch: bool,
) {
    // Already gone or never created; nothing to do.
    let _ = remove_worktree(repo_root, layout, worktree_path, slug, true, created_branch);
}

// ============================================================
// Workspace helpers
// ============================================================

/// `<workspace_dir>/<repo.path>`: one repo's worktree inside a workspace worktree.
pub fn repo_worktree_path(workspace_dir: &Path, repo: &WorkspaceRepo) -> PathBuf {
    paths::join(workspace_dir, &repo.path)
}

pub fn repo_abs_path(workspace_root: &Path, repo: &WorkspaceRepo) -> PathBuf {
    paths::resolve(workspace_root, &repo.path)
}

/// Per-repo outcome of a workspace worktree add.
#[derive(Clone, Debug)]
pub struct WorkspaceRepoAddResult {
    pub repo: WorkspaceRepo,
    /// True when grove created the branch (so rollback may safely delete it).
    pub created_branch: bool,
    pub mode: AddMode,
}

pub struct AddWorkspaceWorktreeResult {
    pub workspace_dir: PathBuf,
    pub per_repo: Vec<WorkspaceRepoAddResult>,
}

/// Create one git worktree per repo under `<grove_dir>/<slug>/<repo.path>`.
/// Each repo independently reuses / adopts / creates its branch, so a branch
/// that only exists in some repos is fine. Rolls back any partial state if a
/// repo fails, deleting only branches grove created.
pub fn add_workspace_worktree(
    workspace_root: &Path,
    layout: &Layout,
    slug: &ParsedSlug,
    repos: &[WorkspaceRepo],
    base_branch_override: Option<&str>,
) -> Result<AddWorkspaceWorktreeResult> {
    if let Some(existing) = layout.dirs().map(|d| worktree_path_for(d, slug)).find(|d| d.exists()) {
        bail!(
            "A worktree for \"{0}\" already exists at {1}. \
             Use `grove resume {0}` to re-enter it, or `grove rm {0}` to remove it.",
            slug.full,
            existing.display()
        );
    }
    let workspace_dir = worktree_path_for(&layout.dir, slug);

    let mut created: Vec<WorkspaceRepoAddResult> = Vec::new();
    let mut add_all = || -> Result<()> {
        for repo in repos {
            created.push(add_repo_worktree(workspace_root, &workspace_dir, slug, repo, base_branch_override)?);
        }
        Ok(())
    };

    if let Err(err) = add_all() {
        rollback_workspace_worktree(workspace_root, layout, &workspace_dir, slug, &created);
        return Err(err);
    }
    Ok(AddWorkspaceWorktreeResult { workspace_dir, per_repo: created })
}

/// Add one repo's worktree under an existing (or new) workspace worktree dir,
/// reusing / adopting / creating its branch exactly like `grove new` does.
pub fn add_repo_worktree(
    workspace_root: &Path,
    workspace_dir: &Path,
    slug: &ParsedSlug,
    repo: &WorkspaceRepo,
    base_branch_override: Option<&str>,
) -> Result<WorkspaceRepoAddResult> {
    let abs = repo_abs_path(workspace_root, repo);
    prune_worktrees(&abs);
    let target = repo_worktree_path(workspace_dir, repo);
    let base = base_branch_override.or(repo.base_branch.as_deref());
    let plan = plan_branch(&abs, slug, &target, base).map_err(|e| match e {
        Error::Grove(m) => Error::Grove(format!("repo \"{}\" ({}): {m}", repo.name, repo.path)),
        other => other,
    })?;
    worktree_add(&abs, &target, &plan)?;
    Ok(WorkspaceRepoAddResult { repo: repo.clone(), created_branch: plan.created_branch, mode: plan.mode })
}

pub fn rollback_workspace_worktree(
    workspace_root: &Path,
    layout: &Layout,
    workspace_dir: &Path,
    slug: &ParsedSlug,
    results: &[WorkspaceRepoAddResult],
) {
    for r in results {
        let abs = repo_abs_path(workspace_root, &r.repo);
        let target = repo_worktree_path(workspace_dir, &r.repo);
        let mut args: Vec<OsString> = vec!["worktree".into(), "remove".into(), "--force".into()];
        args.push(target.into());
        let _ = git(&args, &abs);
        if r.created_branch && branch_exists(&abs, &slug.full) {
            let _ = git(["branch", "-D", &slug.full], &abs);
        }
    }
    // Remove the (likely empty) workspace dir tree.
    let _ = paths::remove_all(workspace_dir);
    prune_empty_parents(layout, workspace_dir);
}

pub fn remove_workspace_worktree(
    workspace_root: &Path,
    layout: &Layout,
    workspace_dir: &Path,
    slug: &ParsedSlug,
    repos: &[WorkspaceRepo],
    force: bool,
) -> Result<()> {
    for repo in repos {
        let target = repo_worktree_path(workspace_dir, repo);
        if !target.exists() {
            continue;
        }
        let abs = repo_abs_path(workspace_root, repo);
        let mut args: Vec<OsString> = vec!["worktree".into(), "remove".into()];
        if force {
            args.push("--force".into());
        }
        args.push(target.into());
        git(&args, &abs)?;
        if branch_exists(&abs, &slug.full) {
            git(["branch", "-D", &slug.full], &abs)?;
        }
    }
    // Whatever's left in <grove_dir>/<slug>/ is shared-symlinks or empty parent dirs.
    let _ = paths::remove_all(workspace_dir);
    prune_empty_parents(layout, workspace_dir);
    Ok(())
}

#[derive(Clone, Debug)]
pub struct WorkspaceWorktreeRepoRow {
    pub repo: WorkspaceRepo,
    pub branch: Option<String>,
    pub path: PathBuf,
    pub registered: bool,
}

#[derive(Clone, Debug)]
pub struct WorkspaceWorktreeRow {
    pub slug: String,
    pub workspace_dir: PathBuf,
    pub per_repo: Vec<WorkspaceWorktreeRepoRow>,
}

pub fn list_workspace_worktrees(
    workspace_root: &Path,
    layout: &Layout,
    repos: &[WorkspaceRepo],
) -> Vec<WorkspaceWorktreeRow> {
    let grove_dirs: Vec<&Path> = layout.dirs().filter(|d| d.exists()).collect();
    if grove_dirs.is_empty() {
        return Vec::new();
    }

    // Cache `git worktree list` per sub-repo to avoid N×M shell-outs.
    let per_repo_index: Vec<HashMap<PathBuf, WorktreePorcelain>> = repos
        .iter()
        .map(|repo| {
            list_worktrees(&repo_abs_path(workspace_root, repo))
                .map(|ws| ws.into_iter().map(|w| (w.path.clone(), w)).collect())
                // sub-repo unreachable — leave index empty
                .unwrap_or_default()
        })
        .collect();

    let mut rows: Vec<WorkspaceWorktreeRow> = Vec::new();
    let slugs = grove_dirs.iter().flat_map(|dir| enumerate_slug_dirs(dir).into_iter().map(move |s| (*dir, s)));
    for (grove_dir, slug) in slugs {
        // The primary dir is enumerated first, so it wins over a legacy duplicate.
        if rows.iter().any(|r| r.slug == slug) {
            continue;
        }
        let workspace_dir = paths::join(grove_dir, &slug);
        let per_repo: Vec<WorkspaceWorktreeRepoRow> = repos
            .iter()
            .zip(&per_repo_index)
            .map(|(repo, index)| {
                let target = repo_worktree_path(&workspace_dir, repo);
                let entry = index.get(&target);
                WorkspaceWorktreeRepoRow {
                    repo: repo.clone(),
                    branch: entry.and_then(|e| e.branch.clone()),
                    registered: entry.is_some(),
                    path: target,
                }
            })
            .collect();
        if per_repo.iter().any(|r| r.registered) {
            rows.push(WorkspaceWorktreeRow { slug, workspace_dir, per_repo });
        }
    }
    rows.sort_by(|a, b| a.slug.cmp(&b.slug));
    rows
}

/// Find immediate `<type>/<name>` directory pairs under a grove dir.
fn enumerate_slug_dirs(grove_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(type_entries) = fs::read_dir(grove_dir) else {
        return out;
    };
    for type_ent in type_entries.flatten() {
        if !type_ent.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let type_name = type_ent.file_name().to_string_lossy().into_owned();
        if !BRANCH_TYPES.contains(&type_name.as_str()) {
            continue;
        }
        let Ok(name_entries) = fs::read_dir(type_ent.path()) else {
            continue;
        };
        for name_ent in name_entries.flatten() {
            if name_ent.file_type().is_ok_and(|t| t.is_dir()) {
                out.push(format!("{type_name}/{}", name_ent.file_name().to_string_lossy()));
            }
        }
    }
    out.sort();
    out
}

pub fn find_workspace_worktree(
    workspace_root: &Path,
    layout: &Layout,
    repos: &[WorkspaceRepo],
    slug: &ParsedSlug,
) -> Option<WorkspaceWorktreeRow> {
    if !layout.dirs().any(|d| worktree_path_for(d, slug).exists()) {
        return None;
    }
    list_workspace_worktrees(workspace_root, layout, repos).into_iter().find(|r| r.slug == slug.full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_slugs() {
        let s = parse_slug("  feat/auth-flow ").unwrap();
        assert_eq!(s.full, "feat/auth-flow");
        assert_eq!(s.kind, "feat");
        assert_eq!(s.name, "auth-flow");
        assert_eq!(parse_slug("chore/a/b.c_d").unwrap().name, "a/b.c_d");
    }

    #[test]
    fn rejects_invalid_slugs() {
        for bad in ["", "auth", "docs/x", "feat/", "feat/-x", "feat/a/../b", "feat/a b", "feat/a~1"] {
            assert!(parse_slug(bad).is_err(), "{bad} should be rejected");
        }
        // `a..` is not a `..` segment, so the name pattern allows it.
        assert!(parse_slug("feat/a..").is_ok());
    }

    #[test]
    fn worktree_paths() {
        let s = parse_slug("feat/a/b").unwrap();
        let local = Layout { dir: PathBuf::from("/r/.grove"), legacy: None };
        assert_eq!(worktree_path_for(&local.dir, &s), PathBuf::from("/r/.grove/feat/a/b"));
        assert!(is_grove_worktree(&local, Path::new("/r/.grove/feat/a")));
        assert!(!is_grove_worktree(&local, Path::new("/r/.grove")));
        assert!(!is_grove_worktree(&local, Path::new("/r/.groveish/x")));
    }

    #[test]
    fn moved_layout_still_sees_legacy_worktrees() {
        let moved =
            Layout { dir: PathBuf::from("/data/grove/worktrees/r-1"), legacy: Some(PathBuf::from("/r/.grove")) };
        assert!(is_grove_worktree(&moved, Path::new("/data/grove/worktrees/r-1/feat/x")));
        assert!(is_grove_worktree(&moved, Path::new("/r/.grove/fix/y")));
        assert!(!is_grove_worktree(&moved, Path::new("/data/grove/worktrees/r-2/feat/x")));
        assert_eq!(slug_of(&moved, Path::new("/data/grove/worktrees/r-1/feat/x")), "feat/x");
        assert_eq!(slug_of(&moved, Path::new("/r/.grove/fix/a/b")), "fix/a/b");
    }
}
