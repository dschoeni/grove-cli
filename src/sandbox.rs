use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::error::{Result, bail};
use crate::paths;
use crate::shared::{SharedInput, resolve_shared_overlay};
use crate::types::SandboxConfig;

pub type Env = Vec<(OsString, OsString)>;

pub struct BuildSandboxInput<'a> {
    /// Bound read-only so shared symlinks pointing back into the project resolve.
    pub root_dir: &'a Path,
    /// Bound read-write — the working directory for the agent.
    pub worktree_path: &'a Path,
    /// Additional rw bind paths (typically per-worktree gitdirs). Caller resolves.
    pub git_dirs: &'a [PathBuf],
    pub sandbox: &'a SandboxConfig,
    pub program: &'a str,
    pub program_args: &'a [String],
}

pub struct SandboxArgv {
    pub command: OsString,
    pub args: Vec<OsString>,
    /// The child's complete environment (the parent's is not inherited).
    pub env: Env,
    /// When set, merge into `<worktree>/.claude/settings.local.json` before
    /// launch (macOS: enables/disables Claude Code's built-in sandbox).
    pub local_settings: Option<Value>,
}

/// Build the argv to launch `program` inside a sandbox.
///
/// Linux wraps the process in bwrap (a mount namespace exposing only what is
/// bound). macOS launches Claude directly and drives Claude Code's *built-in*
/// sandbox instead, via a settings.local.json patch the caller applies.
/// When the sandbox is disabled, the program is launched as-is.
pub fn build_sandbox(input: &BuildSandboxInput) -> Result<SandboxArgv> {
    // macOS always takes the native path: even --no-sandbox must write
    // sandbox.enabled=false so settings from a previous sandboxed launch
    // don't keep applying.
    if cfg!(target_os = "macos") {
        return Ok(build_native_sandbox(input));
    }

    if !input.sandbox.enabled {
        return Ok(SandboxArgv {
            command: input.program.into(),
            args: input.program_args.iter().map(OsString::from).collect(),
            env: passthrough_env(input.worktree_path),
            local_settings: None,
        });
    }

    if cfg!(target_os = "linux") {
        return build_bwrap(input);
    }
    bail!("Sandboxing is not supported on {}. Pass --no-sandbox to run without it.", std::env::consts::OS)
}

/// Linux: construct a bwrap invocation that mounts the worktree rw, the repo
/// root ro (so shared symlinks resolve), essential system + home dirs ro/rw,
/// and execs `program` directly with no intermediate shell.
fn build_bwrap(input: &BuildSandboxInput) -> Result<SandboxArgv> {
    let bwrap = resolve_bwrap()?;
    let tools = resolve_tools();
    let home = paths::home_dir();

    let mut a = Args::default();

    // Namespace isolation. Network stays so Claude can call the API.
    for flag in ["--unshare-user", "--unshare-pid", "--unshare-uts", "--unshare-cgroup", "--die-with-parent"] {
        a.push(flag);
    }

    // System (read-only)
    for dir in ["/usr", "/bin", "/lib", "/sbin", "/etc"] {
        a.ro_bind(Path::new(dir));
    }
    a.push("--proc").push("/proc");
    a.push("--dev").push("/dev");
    a.push("--tmpfs").push("/tmp");

    for opt in ["/lib64", "/run", "/mnt/wsl", "/home/linuxbrew"] {
        if Path::new(opt).exists() {
            a.ro_bind(Path::new(opt));
        }
    }

    // Empty home, then selectively re-share.
    a.push("--tmpfs").push(&home);
    share_home_entries(&mut a, &home);
    share_tool_paths(&mut a, &home, &tools);

    // Root ro so shared symlinks (pointing back into the project) resolve.
    // Worktree binding below overlays this for the rw region.
    a.ro_bind(input.root_dir);

    // Worktree itself (rw)
    a.bind(input.worktree_path);

    // Gitdirs (rw) — main .git dir(s) plus per-worktree gitdir(s).
    for dir in input.git_dirs {
        if dir.exists() {
            a.bind(dir);
        }
    }

    // Optional extra shares from .groverc. On disk each share is a symlink chain
    // <worktree>/<entry> → .grove/shared/<entry> → <root>/<entry>; here a tmpfs
    // overlays .grove/shared and each source is bound at its chain path, so
    // `realpath` on a shared entry stays inside the worktree AND the mountpoints
    // bwrap creates land in the tmpfs instead of persisting on the host fs.
    let overlay = resolve_shared_overlay(&SharedInput {
        root_dir: input.root_dir,
        worktree_path: input.worktree_path,
        share_read_only: &input.sandbox.share_read_only,
        share_read_write: &input.sandbox.share_read_write,
    });
    if let Some(overlay) = overlay {
        a.push("--tmpfs").push(&overlay.tmpfs_dir);
        for spec in overlay.binds {
            a.push(if spec.writable { "--bind" } else { "--ro-bind" }).push(&spec.source).push(&spec.dest);
        }
    }

    a.push("--chdir").push(input.worktree_path);
    a.push("--clearenv");
    for (k, v) in build_sandbox_env(&home, &tools, input.worktree_path) {
        a.push("--setenv").push(k).push(v);
    }

    a.push("--").push(input.program);
    for arg in input.program_args {
        a.push(arg);
    }

    Ok(SandboxArgv { command: bwrap.into(), args: a.0, env: host_env_for_bwrap(), local_settings: None })
}

/// macOS: launch `program` directly and enable Claude Code's *built-in*
/// sandbox (Seatbelt-based, nothing to install) instead of wrapping the
/// process. A hand-rolled sandbox-exec profile keeps fighting Claude's own
/// needs — tty raw mode, keychain reads, atomic config writes — while the
/// native sandbox is maintained against them. It confines Bash commands and
/// their child processes to the worktree plus the session temp dir at the OS
/// level, and gates network access per domain. Read/Edit/Write file tools
/// follow the permission system rather than the sandbox; the worktree-pinning
/// system prompt (claude.rs) covers those.
///
/// The returned `local_settings` patch is merged into the worktree's
/// .claude/settings.local.json by the caller before launch. gitdirs and
/// shareReadWrite sources live outside the worktree, so they are granted via
/// sandbox.filesystem.allowWrite; shareReadOnly needs nothing (native default
/// read policy is broad).
fn build_native_sandbox(input: &BuildSandboxInput) -> SandboxArgv {
    let mut allow_write: Vec<String> = Vec::new();
    let mut add = |p: &Path| {
        let c = paths::canonical(p).to_string_lossy().into_owned();
        if !allow_write.contains(&c) {
            allow_write.push(c);
        }
    };
    for dir in input.git_dirs {
        if dir.exists() {
            add(dir);
        }
    }
    for share in &input.sandbox.share_read_write {
        let abs = paths::resolve(input.root_dir, share);
        if abs.exists() {
            add(&abs);
        }
    }

    let local_settings = if input.sandbox.enabled {
        json!({
            "sandbox": {
                "enabled": true,
                "autoAllowBashIfSandboxed": true,
                "filesystem": { "allowWrite": allow_write },
            }
        })
    } else {
        // Explicit false: settings.local.json persists across launches, so a
        // --no-sandbox run must overwrite what a sandboxed run wrote.
        json!({ "sandbox": { "enabled": false } })
    };

    SandboxArgv {
        command: input.program.into(),
        args: input.program_args.iter().map(OsString::from).collect(),
        env: passthrough_env(input.worktree_path),
        local_settings: Some(local_settings),
    }
}

/// Accumulates bwrap arguments.
#[derive(Default)]
struct Args(Vec<OsString>);

impl Args {
    fn push(&mut self, arg: impl Into<OsString>) -> &mut Self {
        self.0.push(arg.into());
        self
    }

    fn ro_bind(&mut self, p: &Path) {
        self.push("--ro-bind").push(p).push(p);
    }

    fn bind(&mut self, p: &Path) {
        self.push("--bind").push(p).push(p);
    }
}

fn resolve_bwrap() -> Result<PathBuf> {
    let candidate = std::env::var("GROVE_BWRAP_PATH").ok().filter(|s| !s.is_empty()).unwrap_or("bwrap".into());
    let resolved = if Path::new(&candidate).is_absolute() {
        paths::is_executable(Path::new(&candidate)).then(|| PathBuf::from(&candidate))
    } else {
        paths::which(&candidate)
    };
    let Some(resolved) = resolved else {
        bail!("bwrap not found (looked for \"{candidate}\"). Install bwrap or pass --no-sandbox.");
    };

    if let Err(why) = smoke_test(&resolved) {
        bail!("bwrap smoke test failed (user namespaces may be disabled): {why}");
    }
    Ok(resolved)
}

/// Run `bwrap --ro-bind / / -- /bin/true` with a 5s timeout.
fn smoke_test(bwrap: &Path) -> std::result::Result<(), String> {
    let mut child = Command::new(bwrap)
        .args(["--ro-bind", "/", "/", "--", "/bin/true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => return Err(crate::git::describe_exit(status)),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timed out after 5s".into());
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// Real paths of tools the sandbox should be able to find, in PATH order.
struct ResolvedTools {
    claude: Option<PathBuf>,
    node: Option<PathBuf>,
    git: Option<PathBuf>,
    pnpm: Option<PathBuf>,
}

impl ResolvedTools {
    fn all(&self) -> [&Option<PathBuf>; 4] {
        [&self.claude, &self.node, &self.git, &self.pnpm]
    }
}

fn resolve_tools() -> ResolvedTools {
    let find = |bin: &str| paths::which(bin).and_then(|p| fs::canonicalize(p).ok());
    ResolvedTools { claude: find("claude"), node: find("node"), git: find("git"), pnpm: find("pnpm") }
}

fn share_home_entries(a: &mut Args, home: &Path) {
    let mut ro: Vec<PathBuf> = Vec::new();
    let mut rw: Vec<PathBuf> = Vec::new();

    // Claude credentials and session state (rw)
    for rel in [".claude", ".claude.json"] {
        let p = home.join(rel);
        if p.exists() {
            rw.push(p);
        }
    }

    // Identity and tool auth (ro)
    for rel in [".gitconfig", ".config/git", ".config/glab-cli", ".config/gh", ".ssh"] {
        let p = home.join(rel);
        if p.exists() {
            ro.push(p);
        }
    }

    // SSH agent socket
    if let Some(sock) = std::env::var_os("SSH_AUTH_SOCK").filter(|s| !s.is_empty()) {
        let sock = PathBuf::from(sock);
        if sock.exists()
            && let Some(dir) = sock.parent()
        {
            ro.push(dir.to_path_buf());
        }
    }

    for p in &ro {
        a.ro_bind(p);
    }
    for p in &rw {
        a.bind(p);
    }
}

fn pnpm_home(home: &Path) -> PathBuf {
    std::env::var_os("PNPM_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share/pnpm"))
}

fn share_tool_paths(a: &mut Args, home: &Path, tools: &ResolvedTools) {
    // ~/.local/bin (claude symlink), ~/.local/share/claude (install dir)
    for rel in [".local/bin", ".local/share/claude"] {
        let p = home.join(rel);
        if p.exists() {
            a.ro_bind(&p);
        }
    }

    // node prefix — e.g. nvm install dir. Node itself is no longer needed by
    // grove, but Claude and the worktree's own npm/pnpm scripts still use it.
    if let Some(node) = &tools.node
        && let Some(bin_dir) = node.parent()
    {
        let prefix = paths::normalize(&bin_dir.join(".."));
        if prefix.exists() && prefix.to_string_lossy().starts_with(&*home.to_string_lossy()) {
            a.ro_bind(&prefix);
        }
    }

    // pnpm global dir
    let pnpm = pnpm_home(home);
    if pnpm.exists() {
        a.ro_bind(&pnpm);
    }
}

/// The gitdir a worktree's `.git` file points at, if any.
pub fn resolve_worktree_git_dir(worktree_path: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(worktree_path.join(".git")).ok()?;
    let target = content.trim().strip_prefix("gitdir:")?.trim();
    if target.is_empty() || target.contains('\n') {
        return None;
    }
    Some(paths::resolve(worktree_path, target))
}

fn env_or(key: &str, fallback: &str) -> OsString {
    std::env::var_os(key).filter(|v| !v.is_empty()).unwrap_or_else(|| fallback.into())
}

fn build_sandbox_env(home: &Path, tools: &ResolvedTools, worktree_path: &Path) -> Env {
    let mut env: Env = vec![
        ("HOME".into(), home.into()),
        ("USER".into(), env_or("USER", "user")),
        ("TERM".into(), env_or("TERM", "xterm-256color")),
        ("SHELL".into(), "/bin/bash".into()),
        ("LANG".into(), env_or("LANG", "en_US.UTF-8")),
        ("PATH".into(), build_sandbox_path(home, tools)),
        ("PWD".into(), worktree_path.into()),
    ];
    for key in ["ANTHROPIC_API_KEY", "SSH_AUTH_SOCK", "COLORTERM"] {
        if let Some(v) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            env.push((key.into(), v));
        }
    }
    env
}

fn build_sandbox_path(home: &Path, tools: &ResolvedTools) -> OsString {
    let mut dirs: Vec<PathBuf> = ["/usr/local/sbin", "/usr/local/bin", "/usr/sbin", "/usr/bin", "/sbin", "/bin"]
        .iter()
        .map(PathBuf::from)
        .collect();
    dirs.push(home.join(".local/bin"));
    dirs.push(pnpm_home(home));
    for tool in tools.all().into_iter().flatten() {
        if let Some(dir) = tool.parent() {
            dirs.push(dir.to_path_buf());
        }
    }
    let brew = Path::new("/home/linuxbrew/.linuxbrew/bin");
    if brew.exists() {
        dirs.push(brew.to_path_buf());
    }

    let mut seen = Vec::new();
    for d in dirs {
        if !seen.contains(&d) {
            seen.push(d);
        }
    }
    std::env::join_paths(seen).unwrap_or_default()
}

fn host_env_for_bwrap() -> Env {
    ["PATH", "HOME", "USER", "TERM"]
        .into_iter()
        .filter_map(|k| std::env::var_os(k).filter(|v| !v.is_empty()).map(|v| (k.into(), v)))
        .collect()
}

fn passthrough_env(worktree_path: &Path) -> Env {
    let mut env: Env = std::env::vars_os().filter(|(k, _)| k != "PWD").collect();
    env.push(("PWD".into(), worktree_path.into()));
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_worktree_gitdir() {
        let dir = std::env::temp_dir().join(format!("grove-gitdir-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".git"), "gitdir: /repo/.git/worktrees/x\n").unwrap();
        assert_eq!(resolve_worktree_git_dir(&dir), Some(PathBuf::from("/repo/.git/worktrees/x")));
        fs::write(dir.join(".git"), "gitdir: ../rel\n").unwrap();
        assert_eq!(resolve_worktree_git_dir(&dir), Some(dir.parent().unwrap().join("rel")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn native_sandbox_patch() {
        let cfg = SandboxConfig { enabled: false, ..Default::default() };
        let input = BuildSandboxInput {
            root_dir: Path::new("/nonexistent-root"),
            worktree_path: Path::new("/nonexistent-root/.grove/feat/x"),
            git_dirs: &[],
            sandbox: &cfg,
            program: "claude",
            program_args: &[],
        };
        assert_eq!(build_native_sandbox(&input).local_settings, Some(json!({"sandbox": {"enabled": false}})));
        let cfg = SandboxConfig::default();
        let input = BuildSandboxInput { sandbox: &cfg, ..input };
        assert_eq!(
            build_native_sandbox(&input).local_settings,
            Some(
                json!({"sandbox": {"enabled": true, "autoAllowBashIfSandboxed": true, "filesystem": {"allowWrite": []}}})
            )
        );
    }
}
