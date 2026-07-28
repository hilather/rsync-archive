//! Create and embed orchestration.

pub mod embed;
pub mod output;

pub use embed::{has_sevenz_magic, member_name, plan_embed, run_embed, EmbedMember, SEVENZ_MAGIC};
pub use output::{
    cleanup_partial, commit_output, output_exists, partial_path_for, prepare_output, OutputPaths,
};
