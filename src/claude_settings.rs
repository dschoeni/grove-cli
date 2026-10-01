use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::error::Result;

/// Deep-merge `patch` into `<worktree_dir>/.claude/settings.local.json`,
/// preserving any keys the patch doesn't touch. Nested objects merge; arrays
/// and scalars are replaced wholesale (so grove-managed lists like
/// `sandbox.filesystem.allowWrite` reflect the current config, not an
/// accumulation of past runs).
///
/// `.claude/settings.local.json` is the per-project, gitignored override Claude
/// Code reads from cwd at startup — the right place for per-worktree launch
/// state without polluting the committed settings.
pub fn merge_local_settings(worktree_dir: &Path, patch: &Value) -> Result<()> {
    let claude_dir = worktree_dir.join(".claude");
    let settings_path = claude_dir.join("settings.local.json");

    // Missing or malformed — start fresh and overwrite.
    let mut settings = fs::read_to_string(&settings_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default();

    if let Value::Object(patch) = patch {
        deep_merge(&mut settings, patch);
    }

    fs::create_dir_all(&claude_dir)?;
    let mut out = serde_json::to_string_pretty(&Value::Object(settings)).map_err(std::io::Error::other)?;
    out.push('\n');
    fs::write(&settings_path, out)?;
    Ok(())
}

/// Copy `<root_dir>/.claude/settings.local.json` into a freshly created worktree
/// so local-only settings (permission allowlists, env, …) carry over. A one-time
/// snapshot: later edits on either side stay independent. Skipped when the root
/// has no such file or the worktree already has one. Returns true if copied.
///
/// Must run before `ensure_shared_links`, so a real file occupies the path and a
/// share entry for it is skipped — otherwise grove's own settings writes would
/// follow the symlink chain into the root's file.
pub fn seed_local_settings(root_dir: &Path, worktree_dir: &Path) -> Result<bool> {
    let src = root_dir.join(".claude").join("settings.local.json");
    let dest = worktree_dir.join(".claude").join("settings.local.json");
    if !src.exists() || dest.exists() {
        return Ok(false);
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::copy(&src, &dest)?;
    Ok(true)
}

/// Ensure the settings file has a statusLine command that prints `text`.
pub fn ensure_status_line(worktree_dir: &Path, text: &str) -> Result<()> {
    merge_local_settings(
        worktree_dir,
        &json!({
            "statusLine": {
                "type": "command",
                "command": format!("printf '%s' {}", shell_quote(text)),
            }
        }),
    )
}

fn deep_merge(target: &mut Map<String, Value>, patch: &Map<String, Value>) {
    for (key, value) in patch {
        match (target.get_mut(key), value) {
            (Some(Value::Object(existing)), Value::Object(p)) => deep_merge(existing, p),
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_merges_objects_and_replaces_arrays() {
        let mut target =
            json!({"a": 1, "sandbox": {"enabled": true, "filesystem": {"allowWrite": ["/x", "/y"]}, "keep": 1}})
                .as_object()
                .unwrap()
                .clone();
        let patch = json!({"sandbox": {"enabled": false, "filesystem": {"allowWrite": ["/z"]}}, "b": [1]});
        deep_merge(&mut target, patch.as_object().unwrap());
        assert_eq!(
            Value::Object(target),
            json!({"a": 1, "sandbox": {"enabled": false, "filesystem": {"allowWrite": ["/z"]}, "keep": 1}, "b": [1]})
        );
    }

    #[test]
    fn quotes_for_shell() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
