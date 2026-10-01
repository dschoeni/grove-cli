use std::fmt;

/// `Grove` errors reach the user as a one-line `error: …`; `Other` are
/// unexpected failures (I/O, spawn) and are labelled as such.
#[derive(Debug)]
pub enum Error {
    Grove(String),
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn message(&self) -> &str {
        match self {
            Error::Grove(m) | Error::Other(m) => m,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Other(e.to_string())
    }
}

/// Return early with a user-facing `Error::Grove`.
macro_rules! bail {
    ($($arg:tt)*) => {
        return Err($crate::error::Error::Grove(format!($($arg)*)))
    };
}
pub(crate) use bail;
