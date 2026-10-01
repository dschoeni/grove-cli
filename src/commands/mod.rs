pub mod complete_slugs;
pub mod completion;
pub mod init;
pub mod ls;
pub mod new;
pub mod pull;
pub mod resume;
pub mod rm;
pub mod sync;

use std::path::{Path, PathBuf};

use crate::claude::{ClaudeLaunchInput, WorktreeContext, build_claude_argv};
use crate::claude_settings::merge_local_settings;
use crate::error::{Result, bail};
use crate::exec::exec_interactive;
use crate::paths;
use crate::sandbox::{BuildSandboxInput, build_sandbox, resolve_worktree_git_dir};
use crate::shared::{EnsureSharedLinksResult, SharedInput, ensure_shared_links};
use crate::types::{ClaudeConfig, SandboxConfig};

/// Workspace-mode commands that touch every repo must run from the workspace root.
pub fn require_cwd_at_workspace_root(workspace_root: &Path) -> Result<()> {
    let cwd = paths::cwd();
    if paths::absolute(&cwd) != paths::absolute(workspace_root) {
        bail!("Run this command from the workspace root: {} (current: {})", workspace_root.display(), cwd.display());
    }
    Ok(())
}

/// The repo's main `.git` dir plus the worktree's own gitdir, bound rw in the sandbox.
pub fn collect_single_git_dirs(repo_root: &Path, worktree_path: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![repo_root.join(".git")];
    dirs.extend(resolve_worktree_git_dir(worktree_path));
    dirs
}

/// Wire the `.groverc` share entries into the worktree.
pub fn link_shared(root_dir: &Path, worktree_path: &Path, sandbox: &SandboxConfig) -> Result<EnsureSharedLinksResult> {
    ensure_shared_links(&SharedInput {
        root_dir,
        worktree_path,
        share_read_only: &sandbox.share_read_only,
        share_read_write: &sandbox.share_read_write,
    })
}

/// A config's sandbox settings with `--no-sandbox` applied.
pub fn effective_sandbox(config: &SandboxConfig, no_sandbox: bool) -> SandboxConfig {
    SandboxConfig { enabled: config.enabled && !no_sandbox, ..config.clone() }
}

/// Everything needed to start Claude in a prepared worktree.
pub struct Launch<'a> {
    pub claude: &'a ClaudeConfig,
    pub passthrough: &'a [String],
    pub worktree_path: &'a Path,
    pub root_dir: &'a Path,
    pub branch: &'a str,
    pub workspace: bool,
    pub git_dirs: Vec<PathBuf>,
    pub sandbox: &'a SandboxConfig,
}

/// Build the Claude argv, wrap it in the sandbox, apply any settings patch, and exec.
pub fn launch_claude(l: Launch, resume: bool) -> Result<i32> {
    let claude = build_claude_argv(&ClaudeLaunchInput {
        claude: l.claude,
        passthrough: l.passthrough,
        resume,
        worktree: Some(WorktreeContext {
            worktree_path: l.worktree_path,
            root_dir: l.root_dir,
            branch: l.branch,
            workspace: l.workspace,
        }),
    });
    let sandboxed = build_sandbox(&BuildSandboxInput {
        root_dir: l.root_dir,
        worktree_path: l.worktree_path,
        git_dirs: &l.git_dirs,
        sandbox: l.sandbox,
        program: &claude.program,
        program_args: &claude.args,
    })?;
    if let Some(patch) = &sandboxed.local_settings {
        merge_local_settings(l.worktree_path, patch)?;
    }
    exec_interactive(&sandboxed, l.worktree_path)
}
