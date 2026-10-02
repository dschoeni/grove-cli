use std::path::PathBuf;

pub const BRANCH_TYPES: [&str; 3] = ["feat", "fix", "chore"];

#[derive(Clone, Debug)]
pub struct SandboxConfig {
    pub enabled: bool,
    pub share_read_only: Vec<String>,
    pub share_read_write: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self { enabled: true, share_read_only: Vec::new(), share_read_write: Vec::new() }
    }
}

#[derive(Clone, Debug)]
pub struct ClaudeConfig {
    pub command: String,
    pub extra_args: Vec<String>,
    /// Seed a new worktree's `.claude/settings.local.json` from the root's copy.
    pub inherit_local_settings: bool,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            command: "claude".into(),
            extra_args: vec!["--permission-mode".into(), "auto".into()],
            inherit_local_settings: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GroveConfig {
    pub base_branch: Option<String>,
    pub sandbox: SandboxConfig,
    pub post_create_commands: Vec<String>,
    pub claude: ClaudeConfig,
}

#[derive(Clone, Debug)]
pub struct WorkspaceRepo {
    /// Human label, used in CLI output.
    pub name: String,
    /// Path relative to the workspace root, e.g. "apps/web".
    pub path: String,
    /// Default base branch for new worktrees of this repo.
    pub base_branch: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WorkspaceConfig {
    pub repos: Vec<WorkspaceRepo>,
    pub sandbox: SandboxConfig,
    pub post_create_commands: Vec<String>,
    pub claude: ClaudeConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    Groverc,
    Defaults,
}

impl ConfigSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ConfigSource::Groverc => "groverc",
            ConfigSource::Defaults => "defaults",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SingleProject {
    pub repo_root: PathBuf,
    pub config: GroveConfig,
    pub config_source: ConfigSource,
}

#[derive(Clone, Debug)]
pub struct WorkspaceProject {
    pub workspace_root: PathBuf,
    pub config: WorkspaceConfig,
}

#[derive(Clone, Debug)]
pub enum Project {
    Single(SingleProject),
    Workspace(WorkspaceProject),
}

impl Project {
    pub fn sandbox(&self) -> &SandboxConfig {
        match self {
            Project::Single(p) => &p.config.sandbox,
            Project::Workspace(p) => &p.config.sandbox,
        }
    }

    pub fn claude(&self) -> &ClaudeConfig {
        match self {
            Project::Single(p) => &p.config.claude,
            Project::Workspace(p) => &p.config.claude,
        }
    }
}
