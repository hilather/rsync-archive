//! Create and embed orchestration.

pub mod create;
pub mod embed;
pub mod output;

pub use create::{build_rules, build_selection, run_create};
pub use embed::{has_sevenz_magic, member_name, plan_embed, run_embed, EmbedMember, SEVENZ_MAGIC};
pub use output::{
    cleanup_partial, commit_output, output_exists, partial_path_for, prepare_output, OutputPaths,
};
