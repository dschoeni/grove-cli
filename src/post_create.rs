use std::path::Path;
use std::process::Command;

use crate::error::{Result, bail};
use crate::term;

/// Run each command through the platform shell in `worktree_path`, stopping
/// at the first failure.
pub fn run_post_create_commands(worktree_path: &Path, commands: &[String]) -> Result<()> {
    for cmd in commands {
        term::info(format_args!("running: {cmd}"));
        let status = match shell(cmd).current_dir(worktree_path).status() {
            Ok(s) => s,
            Err(e) => bail!("postCreateCommand failed to launch: {e}"),
        };
        if let Some(code) = status.code() {
            if code != 0 {
                bail!("postCreateCommand exited with status {code}: {cmd}");
            }
            continue;
        }
        bail!("postCreateCommand killed by signal {}: {cmd}", signal_name(&status));
    }
    Ok(())
}

#[cfg(unix)]
fn shell(cmd: &str) -> Command {
    let mut c = Command::new("/bin/sh");
    c.arg("-c").arg(cmd);
    c
}

#[cfg(windows)]
fn shell(cmd: &str) -> Command {
    let mut c = Command::new("cmd.exe");
    c.args(["/d", "/s", "/c"]).arg(cmd);
    c
}

#[cfg(unix)]
fn signal_name(status: &std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match status.signal() {
        Some(1) => "SIGHUP".into(),
        Some(2) => "SIGINT".into(),
        Some(3) => "SIGQUIT".into(),
        Some(6) => "SIGABRT".into(),
        Some(9) => "SIGKILL".into(),
        Some(13) => "SIGPIPE".into(),
        Some(15) => "SIGTERM".into(),
        Some(n) => format!("{n}"),
        None => "unknown".into(),
    }
}

#[cfg(not(unix))]
fn signal_name(_status: &std::process::ExitStatus) -> String {
    "unknown".into()
}
