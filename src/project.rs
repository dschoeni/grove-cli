use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Map, Value};

use crate::error::{Result, bail};
use crate::paths;
use crate::types::{
    ClaudeConfig, ConfigSource, GroveConfig, Project, SandboxConfig, SingleProject, WorkspaceConfig, WorkspaceProject,
    WorkspaceRepo,
};

pub fn find_repo_root(start: &Path) -> Result<PathBuf> {
    let out =
        Command::new("git").args(["rev-parse", "--show-toplevel"]).current_dir(start).stdin(Stdio::null()).output();
    if let Ok(o) = out
        && o.status.success()
    {
        let root = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if !root.is_empty() {
            return Ok(PathBuf::from(root));
        }
    }
    bail!("Not inside a git repository (started from {})", start.display())
}

/// Walk up from `start` looking for a directory containing `.groverc`.
pub fn find_groverc_ancestor(start: &Path) -> Option<PathBuf> {
    let mut dir = paths::absolute(start);
    loop {
        if dir.join(".groverc").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

pub fn current_branch(repo_root: &Path) -> Option<String> {
    let o = Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !o.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (!branch.is_empty()).then_some(branch)
}

fn default_config(repo_root: &Path) -> GroveConfig {
    GroveConfig {
        base_branch: current_branch(repo_root),
        sandbox: SandboxConfig::default(),
        post_create_commands: Vec::new(),
        claude: ClaudeConfig::default(),
    }
}

/// Load the project the current directory belongs to.
pub fn load_project() -> Result<Project> {
    load_project_from(&paths::cwd())
}

pub fn load_project_from(start: &Path) -> Result<Project> {
    if let Some(ancestor) = find_groverc_ancestor(start) {
        let raw = fs::read_to_string(ancestor.join(".groverc"))?;
        let parsed: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => bail!(".groverc is not valid JSON: {e}"),
        };
        let Value::Object(obj) = parsed else {
            bail!(".groverc must be a JSON object");
        };
        if obj.get("type").and_then(Value::as_str) == Some("workspace") {
            let config = parse_workspace_config(&obj, &ancestor)?;
            return Ok(Project::Workspace(WorkspaceProject { workspace_root: ancestor, config }));
        }
        return Ok(Project::Single(SingleProject {
            repo_root: find_repo_root(&ancestor)?,
            config: merge_single_config(&obj, &ancestor),
            config_source: ConfigSource::Groverc,
        }));
    }

    // No .groverc — fall back to git repo root with defaults.
    let repo_root = find_repo_root(start)?;
    Ok(Project::Single(SingleProject {
        config: default_config(&repo_root),
        repo_root,
        config_source: ConfigSource::Defaults,
    }))
}

fn merge_single_config(obj: &Map<String, Value>, repo_root: &Path) -> GroveConfig {
    let mut base = default_config(repo_root);
    if let Some(b) = trimmed_str(obj.get("baseBranch")) {
        base.base_branch = Some(b);
    }
    apply_sandbox(obj, &mut base.sandbox);
    if let Some(cmds) = string_array(obj.get("postCreateCommands")) {
        base.post_create_commands = cmds;
    }
    apply_claude(obj, &mut base.claude);
    base
}

fn parse_workspace_config(obj: &Map<String, Value>, workspace_root: &Path) -> Result<WorkspaceConfig> {
    let repos_raw = match obj.get("repos") {
        Some(Value::Array(a)) if !a.is_empty() => a,
        _ => bail!("workspace .groverc must define a non-empty \"repos\" array"),
    };

    let mut repos = Vec::with_capacity(repos_raw.len());
    for (idx, r) in repos_raw.iter().enumerate() {
        let Value::Object(rec) = r else {
            bail!("repos[{idx}] must be an object");
        };
        let Some(repo_path) = trimmed_str(rec.get("path")) else {
            bail!("repos[{idx}].path must be a non-empty string");
        };
        let repo_abs = paths::resolve(workspace_root, &repo_path);
        if !repo_abs.join(".git").exists() {
            bail!("repos[{idx}] \"{repo_path}\" is not a git repo (no .git at {})", repo_abs.display());
        }
        let name = trimmed_str(rec.get("name")).unwrap_or_else(|| paths::basename(Path::new(&repo_path)));
        let base_branch = trimmed_str(rec.get("baseBranch"));
        repos.push(WorkspaceRepo { name, path: repo_path, base_branch });
    }

    let mut sandbox = SandboxConfig::default();
    apply_sandbox(obj, &mut sandbox);
    let mut claude = ClaudeConfig::default();
    apply_claude(obj, &mut claude);
    let post_create_commands = string_array(obj.get("postCreateCommands")).unwrap_or_default();

    Ok(WorkspaceConfig { repos, sandbox, post_create_commands, claude })
}

fn apply_sandbox(obj: &Map<String, Value>, sandbox: &mut SandboxConfig) {
    if let Some(Value::Object(s)) = obj.get("sandbox") {
        if let Some(enabled) = s.get("enabled").and_then(Value::as_bool) {
            sandbox.enabled = enabled;
        }
        if let Some(ro) = string_array(s.get("shareReadOnly")) {
            sandbox.share_read_only = ro;
        }
        if let Some(rw) = string_array(s.get("shareReadWrite")) {
            sandbox.share_read_write = rw;
        }
    }

    // Legacy `shared: string[]` maps to shareReadOnly when no explicit list given.
    if sandbox.share_read_only.is_empty()
        && let Some(shared) = string_array(obj.get("shared"))
    {
        sandbox.share_read_only = shared;
    }
}

fn apply_claude(obj: &Map<String, Value>, claude: &mut ClaudeConfig) {
    if let Some(Value::Object(c)) = obj.get("claude") {
        if let Some(command) = trimmed_str(c.get("command")) {
            claude.command = command;
        }
        if let Some(extra) = string_array(c.get("extraArgs")) {
            claude.extra_args = extra;
        }
        if let Some(inherit) = c.get("inheritLocalSettings").and_then(Value::as_bool) {
            claude.inherit_local_settings = inherit;
        }
    }
}

/// A non-blank string value, trimmed.
fn trimmed_str(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// The string elements of an array value (non-strings dropped), or None when not an array.
fn string_array(v: Option<&Value>) -> Option<Vec<String>> {
    let arr = v?.as_array()?;
    Some(arr.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
}

/// Add `/.grove/` to `<repo>/.git/info/exclude` if it isn't there yet.
pub fn ensure_grove_ignored(repo_root: &Path) -> Result<()> {
    // No-op when the root isn't a git repo (e.g. a workspace root). Each sub-repo
    // tracks its own worktrees via git itself, so nothing to gitignore there either.
    if !repo_root.join(".git").exists() {
        return Ok(());
    }

    let exclude_file = repo_root.join(".git").join("info").join("exclude");
    const LINE: &str = "/.grove/";
    let contents = match fs::read_to_string(&exclude_file) {
        Ok(c) => c,
        Err(_) => {
            if let Some(dir) = exclude_file.parent() {
                fs::create_dir_all(dir)?;
            }
            String::new()
        }
    };
    if contents.split('\n').any(|l| l.trim() == LINE) {
        return Ok(());
    }
    let prefix = if !contents.is_empty() && !contents.ends_with('\n') { "\n" } else { "" };
    let mut f = fs::OpenOptions::new().create(true).append(true).open(&exclude_file)?;
    writeln!(f, "{prefix}{LINE}")?;
    Ok(())
}
