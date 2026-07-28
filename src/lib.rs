//! rsync-archive library: selection, non-solid 7z create, and store embed.
//!
//! Stage 0 is scaffolding only. Pipelines land in later stages.

pub mod cli;
pub mod error;

pub use error::{Error, Result};

/// Library version (same as package).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
