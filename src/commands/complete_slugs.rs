use crate::project::load_project;
use crate::term::stdout;
use crate::types::Project;
use crate::worktree::{list_grove_worktrees, list_workspace_worktrees, slug_of};

/// Hidden helper invoked by shell completion. Prints one slug per line of every
/// Grove worktree known in the current project. Stays silent on errors so a
/// transient failure doesn't break tab-completion.
pub fn run() {
    let Ok(project) = load_project() else {
        return;
    };
    match project {
        Project::Workspace(ws) => {
            for row in list_workspace_worktrees(&ws.workspace_root, &ws.layout, &ws.config.repos) {
                stdout!("{}\n", row.slug);
            }
        }
        Project::Single(single) => {
            for w in list_grove_worktrees(&single.repo_root, &single.layout).unwrap_or_default() {
                stdout!("{}\n", slug_of(&single.layout, &w.path));
            }
        }
    }
}
