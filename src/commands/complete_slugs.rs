use crate::paths;
use crate::project::load_project;
use crate::term::stdout;
use crate::types::Project;
use crate::worktree::{list_grove_worktrees, list_workspace_worktrees};

/// Hidden helper invoked by shell completion. Prints one slug per line of every
/// Grove worktree known in the current project. Stays silent on errors so a
/// transient failure doesn't break tab-completion.
pub fn run() {
    let Ok(project) = load_project() else {
        return;
    };
    match project {
        Project::Workspace(ws) => {
            for row in list_workspace_worktrees(&ws.workspace_root, &ws.config.repos) {
                stdout!("{}\n", row.slug);
            }
        }
        Project::Single(single) => {
            let grove_dir = single.repo_root.join(".grove");
            for w in list_grove_worktrees(&single.repo_root).unwrap_or_default() {
                stdout!("{}\n", paths::relative(&grove_dir, &w.path).display());
            }
        }
    }
}
