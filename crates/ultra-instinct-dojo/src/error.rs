//! Dojo errors: IO on the diary file, parse errors on untrusted lines,
//! and a schema mismatch that refuses to read a newer diary.

use std::fmt;
use std::path::PathBuf;

/// Errors from reading or writing a battle diary.
#[derive(Debug)]
#[non_exhaustive]
pub enum DojoError {
    /// A filesystem operation failed.
    Io { path: PathBuf, message: String },
    /// A diary line failed validation (1-based line number).
    Parse { line: usize, message: String },
    /// The diary's schema version is not [`crate::line::SCHEMA`].
    Schema { found: u64 },
}

impl fmt::Display for DojoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Parse { line, message } => write!(f, "diary line {line}: {message}"),
            Self::Schema { found } => {
                write!(f, "unsupported diary schema {found}")
            }
        }
    }
}

impl std::error::Error for DojoError {}
