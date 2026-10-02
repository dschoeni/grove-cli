use std::path::Path;

use crate::args::{self, HELP};
use crate::error::Result;
use crate::paths;
use crate::project::load_project;
use crate::term::stdout;
use crate::types::{Project, WorkspaceRepo};
use crate::worktree::{list_grove_worktrees, list_workspace_worktrees};

const USAGE: &str = "\
grove ls — list Grove-managed worktrees in the current repo
";

pub fn run(argv: &[String]) -> Result<i32> {
    let p = args::parse(argv, &[HELP], false)?;
    if p.flag("help") {
        stdout!("{USAGE}");
        return Ok(0);
    }

    match load_project()? {
        Project::Workspace(ws) => run_workspace(&ws.workspace_root, &ws.config.repos),
        Project::Single(single) => run_single(&single.repo_root)?,
    }
    Ok(0)
}

fn run_single(repo_root: &Path) -> Result<()> {
    let worktrees = list_grove_worktrees(repo_root)?;
    if worktrees.is_empty() {
        stdout!("No Grove worktrees in this repo.\n");
        return Ok(());
    }

    let grove_dir = repo_root.join(".grove");
    let rows: Vec<Vec<String>> = worktrees
        .iter()
        .map(|w| {
            vec![
                paths::relative(&grove_dir, &w.path).display().to_string(),
                w.branch.clone().unwrap_or_else(|| "(detached)".into()),
                w.path.display().to_string(),
            ]
        })
        .collect();
    print_table(&["SLUG", "BRANCH", "PATH"], &rows);
    Ok(())
}

fn run_workspace(workspace_root: &Path, repos: &[WorkspaceRepo]) {
    let rows = list_workspace_worktrees(workspace_root, repos);
    if rows.is_empty() {
        stdout!("No Grove worktrees in this workspace.\n");
        return;
    }

    let mut table = Vec::new();
    for row in &rows {
        let mut branches: Vec<&str> = Vec::new();
        for b in row.per_repo.iter().filter_map(|r| r.branch.as_deref()) {
            if !branches.contains(&b) {
                branches.push(b);
            }
        }
        let branch_label = match branches.as_slice() {
            [] => "(missing)".to_string(),
            [one] => one.to_string(),
            many => format!("mixed: {}", many.join(", ")),
        };
        let present: Vec<&str> = row.per_repo.iter().filter(|r| r.registered).map(|r| r.repo.name.as_str()).collect();
        let total = row.per_repo.len();
        let repos_label = if present.len() == total {
            format!("{}/{total}", present.len())
        } else {
            format!("{}/{total} ({})", present.len(), present.join(","))
        };
        table.push(vec![row.slug.clone(), branch_label, repos_label, row.workspace_dir.display().to_string()]);
    }
    print_table(&["SLUG", "BRANCH", "REPOS", "PATH"], &table);
}

/// Left-aligned columns separated by two spaces; the last column is not padded.
fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    // UTF-16 length, matching how the Node version measured column widths.
    let width = |s: &str| s.encode_utf16().count();
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| rows.iter().map(|r| r.get(i).map_or(0, |c| width(c))).fold(width(h), usize::max))
        .collect();
    let fmt = |cells: Vec<&str>| {
        let last = cells.len() - 1;
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| if i == last { c.to_string() } else { format!("{c}{}", " ".repeat(widths[i] - width(c))) })
            .collect::<Vec<_>>()
            .join("  ")
    };
    stdout!("{}\n", fmt(headers.to_vec()));
    for r in rows {
        stdout!("{}\n", fmt(r.iter().map(String::as_str).collect()));
    }
}
