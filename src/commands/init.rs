use std::fs;

use serde_json::json;

use crate::args::{self, HELP, flag, value};
use crate::error::{Result, bail};
use crate::paths;
use crate::project::{current_branch, ensure_grove_ignored, find_repo_root};
use crate::term::stdout;

const USAGE: &str = "\
grove init — write a starter .groverc at the repo root

Usage:
  grove init [--base-branch <name>] [--force]

Flags:
  --base-branch <name>   Default base branch for new worktrees. Defaults to current HEAD.
  --force                Overwrite an existing .groverc.
";

pub fn run(argv: &[String]) -> Result<i32> {
    let p = args::parse(argv, &[HELP, value("base-branch"), flag("force")], false)?;
    if p.flag("help") {
        stdout!("{USAGE}");
        return Ok(0);
    }

    let repo_root = find_repo_root(&paths::cwd())?;
    let groverc_path = repo_root.join(".groverc");
    if groverc_path.exists() && !p.flag("force") {
        bail!(".groverc already exists at {} (use --force to overwrite)", groverc_path.display());
    }

    let base_branch = p.value("base-branch").or_else(|| current_branch(&repo_root)).unwrap_or_else(|| "main".into());
    let starter = json!({
        "baseBranch": base_branch,
        "postCreateCommands": [],
        "sandbox": {
            "enabled": true,
            "shareReadOnly": [],
            "shareReadWrite": [],
        },
    });
    let mut out = serde_json::to_string_pretty(&starter).map_err(std::io::Error::other)?;
    out.push('\n');
    fs::write(&groverc_path, out)?;
    ensure_grove_ignored(&repo_root)?;
    stdout!("Wrote {}\n", groverc_path.display());
    Ok(0)
}
