//! Error types for rsync-archive.

use std::path::PathBuf;
use thiserror::Error;

/// Library and CLI operational errors (exit code 1 when reported from main).
#[derive(Debug, Error)]
pub enum Error {
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Message(String),

    #[error("CLI: {0}")]
    Cli(String),

    #[error("selection: {0}")]
    Selection(String),

    #[error("archive: {0}")]
    Archive(String),

    #[error("output already exists: {0} (use --force to overwrite)")]
    OutputExists(PathBuf),

    #[error("empty archive: no members to write")]
    EmptyArchive,

    #[error("path traversal rejected: {0}")]
    PathTraversal(String),

    #[error("duplicate archive member name: {0}")]
    Collision(String),

    #[error("invalid member name: {0}")]
    InvalidMemberName(String),

    #[error("filter file too large: {path} ({detail})")]
    FilterFileTooLarge { path: PathBuf, detail: String },

    #[error("not a regular file: {0}")]
    NotRegularFile(PathBuf),

    #[error("invalid UTF-8 path: {0}")]
    InvalidUtf8Path(String),

    #[error("file too large for current encoder path: {path} ({size} bytes; limit {limit})")]
    FileTooLarge {
        path: PathBuf,
        size: u64,
        limit: u64,
    },

    #[error("compression error: {0}")]
    Compress(String),

    /// Source vanished or became inaccessible between selection and open (soft-skipped by create).
    #[error("vanished: {0}")]
    Vanished(PathBuf),
}

/// Convenient result alias.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Build a message error from a displayable value.
    pub fn msg(s: impl Into<String>) -> Self {
        Error::Message(s.into())
    }
}
