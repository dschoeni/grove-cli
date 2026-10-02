//! grove — git worktree + sandbox harness for Claude Code.

mod args;
mod claude;
mod claude_settings;
mod commands;
mod error;
mod exec;
mod git;
mod paths;
mod post_create;
mod project;
mod sandbox;
mod shared;
mod status_line;
mod term;
mod types;
mod worktree;

use error::{Error, Result};
use term::{RED, RESET, stderr, stdout};

const TOP_HELP: &str = "\
grove — git worktree + bwrap sandbox harness for Claude Code

Usage:
  grove <command> [args]

Commands:
  init                 Write a starter .groverc at the repo root.
  new <slug>           Create a worktree and launch Claude inside a sandbox.
  resume <slug>        Re-enter an existing worktree and continue the last session.
  sync <slug>          Update a worktree's branch(es) from their remote.
  ls                   List Grove-managed worktrees.
  rm <slug>            Remove a worktree and its branch.
  pull [branch]        Fast-forward the base branch to its latest remote state.
  completion <shell>   Output a bash or zsh completion script.

Use \"grove <command> --help\" for per-command flags.
";

fn run(argv: &[String]) -> Result<i32> {
    let Some((subcommand, rest)) = argv.split_first() else {
        stdout!("{TOP_HELP}");
        return Ok(0);
    };

    match subcommand.as_str() {
        "-h" | "--help" | "help" => {
            stdout!("{TOP_HELP}");
            Ok(0)
        }
        "-V" | "--version" | "version" => {
            stdout!("grove {}\n", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        "init" => commands::init::run(rest),
        "new" => commands::new::run(rest),
        "resume" => commands::resume::run(rest),
        "sync" => commands::sync::run(rest),
        "ls" => commands::ls::run(rest),
        "rm" => commands::rm::run(rest),
        "pull" => commands::pull::run(rest),
        "completion" => Ok(commands::completion::run(rest)),
        "__complete-slugs" => {
            commands::complete_slugs::run();
            Ok(0)
        }
        other => {
            stderr!("Unknown command: {other}\n\n{TOP_HELP}");
            Ok(64)
        }
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = match run(&argv) {
        Ok(code) => code,
        Err(Error::Grove(msg)) => {
            stderr!("{RED}error:{RESET} {msg}\n");
            1
        }
        Err(Error::Other(msg)) => {
            stderr!("{RED}unexpected error:{RESET} {msg}\n");
            1
        }
    };
    std::process::exit(code);
}
