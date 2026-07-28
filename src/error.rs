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
}

/// Convenient result alias.
pub type Result<T> = std::result::Result<T, Error>;
