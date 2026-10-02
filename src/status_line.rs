use std::path::Path;

use crate::paths;
use crate::term::{BOLD, CYAN, DIM, RESET};

pub fn single_status_line_text(slug: &str) -> String {
    format!("{CYAN}grove{RESET}: {BOLD}{slug}{RESET}")
}

pub fn workspace_status_line_text(workspace_root: &Path, slug: &str) -> String {
    let ws_name = paths::basename(workspace_root);
    format!("{CYAN}grove{RESET}{DIM}[{ws_name}]{RESET}: {BOLD}{slug}{RESET}")
}
