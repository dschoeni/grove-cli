use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};
use crate::sandbox::SandboxArgv;

/// Hand the terminal over to the sandboxed program.
///
/// On Unix this `execve`s, replacing grove entirely: signals, the tty and the
/// exit status belong to the child with no shim in between. Elsewhere it
/// spawns, waits, and returns the child's exit code.
pub fn exec_interactive(argv: &SandboxArgv, cwd: &Path) -> Result<i32> {
    let mut cmd = Command::new(&argv.command);
    cmd.args(&argv.args).env_clear().envs(argv.env.iter().map(|(k, v)| (k, v))).current_dir(cwd);
    launch(cmd, argv)
}

#[cfg(unix)]
fn launch(mut cmd: Command, argv: &SandboxArgv) -> Result<i32> {
    use std::os::unix::process::CommandExt;
    let err = cmd.exec();
    Err(Error::Grove(format!("failed to launch {}: {err}", argv.command.to_string_lossy())))
}

#[cfg(not(unix))]
fn launch(mut cmd: Command, argv: &SandboxArgv) -> Result<i32> {
    let status =
        cmd.status().map_err(|e| Error::Grove(format!("failed to launch {}: {e}", argv.command.to_string_lossy())))?;
    Ok(status.code().unwrap_or(1))
}
