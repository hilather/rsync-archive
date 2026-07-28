//! Create and embed orchestration (pipelines land in later stages).

pub mod output;

pub use output::{
    cleanup_partial, commit_output, output_exists, partial_path_for, prepare_output, OutputPaths,
};
