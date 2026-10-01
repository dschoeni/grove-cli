//! Output helpers. Writes ignore errors so a closed pipe (`grove ls | head`)
//! never panics the way `print!` would.

/// Write to stdout, ignoring errors.
macro_rules! stdout {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = write!(std::io::stdout(), $($arg)*);
    }};
}

/// Write to stderr, ignoring errors.
macro_rules! stderr {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = write!(std::io::stderr(), $($arg)*);
    }};
}

pub(crate) use {stderr, stdout};

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const CYAN: &str = "\x1b[36m";
pub const GRAY: &str = "\x1b[90m";

/// `[grove] msg` in cyan on stderr.
pub fn info(msg: impl std::fmt::Display) {
    stderr!("{CYAN}[grove]{RESET} {msg}\n");
}

/// `[grove] msg` in yellow on stderr.
pub fn warn(msg: impl std::fmt::Display) {
    stderr!("{YELLOW}[grove]{RESET} {msg}\n");
}

/// `[grove] msg` in red on stderr.
pub fn fail(msg: impl std::fmt::Display) {
    stderr!("{RED}[grove]{RESET} {msg}\n");
}
