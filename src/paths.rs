//! Path helpers with Node `path` semantics (lexical normalization, `join`
//! that never lets an absolute tail escape the base) plus a few fs utilities.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Lexically normalize: drop `.`, fold `..` against preceding components.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// `path.join(base, tail)`: append `tail` (even when absolute) and normalize.
pub fn join(base: &Path, tail: impl AsRef<Path>) -> PathBuf {
    let mut p = base.to_path_buf();
    for c in tail.as_ref().components() {
        match c {
            Component::RootDir | Component::Prefix(_) => {}
            other => p.push(other.as_os_str()),
        }
    }
    normalize(&p)
}

/// Make `p` absolute against the current directory and normalize it.
pub fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() { normalize(p) } else { normalize(&cwd().join(p)) }
}

/// `path.resolve(base, p)`: an absolute `p` wins, a relative one is joined.
pub fn resolve(base: &Path, p: impl AsRef<Path>) -> PathBuf {
    let p = p.as_ref();
    if p.is_absolute() { normalize(p) } else { normalize(&absolute(base).join(p)) }
}

/// `path.relative(from, to)`. Empty when both resolve to the same path.
pub fn relative(from: &Path, to: &Path) -> PathBuf {
    let from = absolute(from);
    let to = absolute(to);
    let f: Vec<_> = from.components().collect();
    let t: Vec<_> = to.components().collect();
    let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..f.len() {
        out.push("..");
    }
    for c in &t[common..] {
        out.push(c.as_os_str());
    }
    out
}

pub fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// `$HOME` (or `%USERPROFILE%` on Windows), like Node's `os.homedir()`.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Canonicalize (resolving symlinks like /tmp → /private/tmp), falling back
/// to a lexical absolute path when the target doesn't exist.
pub fn canonical(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| absolute(p))
}

/// Locate an executable the way `which` does: a name containing a separator
/// is checked directly, anything else is searched on `$PATH`.
pub fn which(bin: &str) -> Option<PathBuf> {
    if bin.contains('/') {
        let p = absolute(Path::new(bin));
        return is_executable(&p).then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(bin)).find(|p| is_executable(p))
}

#[cfg(unix)]
pub fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
pub fn is_executable(p: &Path) -> bool {
    fs::metadata(p).is_ok_and(|m| m.is_file())
}

/// `lstat` that maps "missing" to `None`.
pub fn lstat(p: &Path) -> Option<fs::Metadata> {
    fs::symlink_metadata(p).ok()
}

pub fn is_symlink(p: &Path) -> bool {
    lstat(p).is_some_and(|m| m.file_type().is_symlink())
}

/// `fs.rmSync(p, { recursive: true, force: true })`.
pub fn remove_all(p: &Path) -> io::Result<()> {
    let result = match lstat(p) {
        None => return Ok(()),
        Some(m) if m.is_dir() => fs::remove_dir_all(p),
        Some(_) => fs::remove_file(p),
    };
    match result {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// True for an existing directory with no entries.
pub fn is_empty_dir(p: &Path) -> bool {
    fs::read_dir(p).is_ok_and(|mut it| it.next().is_none())
}

#[cfg(unix)]
pub fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
pub fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    let resolved = match link.parent() {
        Some(parent) => parent.join(target),
        None => target.to_path_buf(),
    };
    if resolved.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

/// Final path component as a string (`path.basename`).
pub fn basename(p: &Path) -> String {
    p.file_name().map(OsStr::to_string_lossy).unwrap_or_default().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_keeps_absolute_tail_inside_base() {
        assert_eq!(join(Path::new("/a/b"), "/c/d"), PathBuf::from("/a/b/c/d"));
        assert_eq!(join(Path::new("/a/b"), "../c"), PathBuf::from("/a/c"));
        assert_eq!(join(Path::new("/a"), "./b/./c"), PathBuf::from("/a/b/c"));
    }

    #[test]
    fn resolve_prefers_absolute() {
        assert_eq!(resolve(Path::new("/a"), "/x/../y"), PathBuf::from("/y"));
        assert_eq!(resolve(Path::new("/a"), "b/c"), PathBuf::from("/a/b/c"));
    }

    #[test]
    fn relative_paths() {
        assert_eq!(
            relative(Path::new("/w/node_modules/.."), Path::new("/w/.grove/shared/x")),
            PathBuf::from(".grove/shared/x")
        );
        assert_eq!(
            relative(Path::new("/w/a/b"), Path::new("/w/.grove/shared/a/b/c")),
            PathBuf::from("../../.grove/shared/a/b/c")
        );
        assert_eq!(relative(Path::new("/w"), Path::new("/w")), PathBuf::new());
    }

    #[test]
    fn normalize_does_not_climb_above_root() {
        assert_eq!(normalize(Path::new("/../a")), PathBuf::from("/a"));
        assert_eq!(normalize(Path::new("../a")), PathBuf::from("../a"));
    }
}
