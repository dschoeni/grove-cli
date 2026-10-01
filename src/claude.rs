use std::fs;
use std::path::{Path, PathBuf};

use crate::paths;
use crate::types::ClaudeConfig;

pub struct WorktreeContext<'a> {
    /// The worktree Claude is launched in.
    pub worktree_path: &'a Path,
    /// The repo root (single) or workspace root (workspace).
    pub root_dir: &'a Path,
    /// Branch slug, e.g. feat/auth-flow.
    pub branch: &'a str,
    /// True in workspace mode (one nested worktree per repo).
    pub workspace: bool,
}

pub struct ClaudeLaunchInput<'a> {
    pub claude: &'a ClaudeConfig,
    /// Pass-through args from `grove new foo -- ...`.
    pub passthrough: &'a [String],
    /// When true, append `--continue` to resume the most recent session.
    pub resume: bool,
    /// When set, append a system prompt pinning Claude to the worktree.
    pub worktree: Option<WorktreeContext<'a>>,
}

pub struct ClaudeArgv {
    pub program: String,
    pub args: Vec<String>,
}

pub fn build_claude_argv(input: &ClaudeLaunchInput) -> ClaudeArgv {
    let mut args = input.claude.extra_args.clone();
    if input.resume {
        args.push("--continue".into());
    }
    // The worktree lives *inside* the main checkout, so Claude picks up parent
    // CLAUDE.md files and symlink targets that point at the root — without an
    // explicit pin it regularly wanders out of the worktree. Skipped when the
    // user supplies their own --append-system-prompt (the CLI only honors one).
    if let Some(ctx) = &input.worktree
        && !has_append_system_prompt(input)
    {
        args.push("--append-system-prompt".into());
        args.push(worktree_system_prompt(ctx));
    }
    args.extend(input.passthrough.iter().cloned());
    ClaudeArgv { program: input.claude.command.clone(), args }
}

fn has_append_system_prompt(input: &ClaudeLaunchInput) -> bool {
    input.claude.extra_args.iter().chain(input.passthrough).any(|a| a == "--append-system-prompt")
}

fn worktree_system_prompt(ctx: &WorktreeContext) -> String {
    let what = if ctx.workspace {
        format!(
            "a grove-managed workspace checkout for branch \"{}\" — each subdirectory is a git worktree of one of the workspace's repositories",
            ctx.branch
        )
    } else {
        format!("a grove-managed git worktree for branch \"{}\"", ctx.branch)
    };
    let wt = ctx.worktree_path.display();
    let root = ctx.root_dir.display();
    format!(
        "Your working directory {wt} is {what}. \
         Treat it as the project root: create, edit, commit, and run files only inside it. \
         The main checkout at {root} is NOT your workspace — never modify anything there. \
         If a path, symlink target, or command output points outside {wt}, \
         switch to the corresponding path inside the worktree before acting."
    )
}

/// True when Claude has a stored transcript for `cwd`, i.e. `claude --continue`
/// would find something. Claude keeps one directory per project under
/// `<config>/projects/`, named after the cwd with every non-alphanumeric
/// character replaced by a dash.
pub fn has_claude_session(cwd: &Path) -> bool {
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| paths::home_dir().join(".claude"));
    let project_dir = config_dir.join("projects").join(encode_project_dir(cwd));
    let Ok(entries) = fs::read_dir(project_dir) else {
        return false;
    };
    entries.flatten().any(|e| e.file_name().to_string_lossy().ends_with(".jsonl"))
}

/// Mirrors Claude's own cwd → project-dir mapping: realpath, then dash out
/// non-alphanumerics. Claude runs on JS strings, so a character outside the
/// BMP (two UTF-16 code units) becomes two dashes.
fn encode_project_dir(cwd: &Path) -> String {
    let resolved = paths::canonical(cwd);
    resolved
        .to_string_lossy()
        .chars()
        .flat_map(|c| {
            let n = if c.is_ascii_alphanumeric() { 0 } else { c.len_utf16() };
            let keep = (n == 0).then_some(c);
            keep.into_iter().chain(std::iter::repeat_n('-', n))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(extra: &[&str]) -> ClaudeConfig {
        ClaudeConfig { extra_args: extra.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn appends_pin_prompt_unless_user_supplies_one() {
        let c = cfg(&["--permission-mode", "auto"]);
        let ctx = || WorktreeContext {
            worktree_path: Path::new("/r/.grove/feat/x"),
            root_dir: Path::new("/r"),
            branch: "feat/x",
            workspace: false,
        };
        let pass = vec!["-p".to_string()];
        let argv = build_claude_argv(&ClaudeLaunchInput {
            claude: &c,
            passthrough: &pass,
            resume: true,
            worktree: Some(ctx()),
        });
        assert_eq!(argv.program, "claude");
        assert_eq!(&argv.args[..4], ["--permission-mode", "auto", "--continue", "--append-system-prompt"]);
        assert!(argv.args[4].starts_with("Your working directory /r/.grove/feat/x is a grove-managed git worktree"));
        assert_eq!(argv.args.last().unwrap(), "-p");

        let pass = vec!["--append-system-prompt".to_string(), "mine".to_string()];
        let argv = build_claude_argv(&ClaudeLaunchInput {
            claude: &c,
            passthrough: &pass,
            resume: false,
            worktree: Some(ctx()),
        });
        assert_eq!(argv.args, ["--permission-mode", "auto", "--append-system-prompt", "mine"]);
    }

    #[test]
    fn encodes_project_dir_like_claude() {
        assert_eq!(encode_project_dir(Path::new("/no/such/dir_x.y")), "-no-such-dir-x-y");
        assert_eq!(encode_project_dir(Path::new("/no/é😀")), "-no----");
    }
}
