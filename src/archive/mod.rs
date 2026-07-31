//! Archive format writers (7z non-solid create/store, seekable-zstd, tar.zst, tar.lz4).

pub mod seekable_zstd;
pub mod sevenz;
pub mod tar_common;
pub mod tar_lz4;
pub mod tar_zstd;

/// Per-encode outcome for create writers (after selection).
///
/// `members_written` counts selected content members that were archived
/// (files / symlinks / hard links). Parent directory stubs on tar formats are
/// not counted here. `skipped_vanished` is soft-skipped at open/encode.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CreateWriteStats {
    pub members_written: u64,
    pub skipped_vanished: u64,
}

pub use seekable_zstd::{
    extract_member, extract_member_bytes, list_members, verify_archive as verify_seekable_zstd,
    write_seekable_zstd, MemberIndex, MemberIndexEntry, DEFAULT_FRAME_SIZE, INDEX_MAGIC,
    INDEX_VERSION,
};
pub use sevenz::{
    write_raw_header, write_start_header, CompressMethod, HeaderFile, NonsolidLzma2Writer,
    NonsolidStoreWriter, SIG_HEADER_SIZE,
};
pub use tar_common::{TarMemberIndex, TarMemberIndexEntry};
pub use tar_lz4::{
    decompress_tar_lz4_payload_to_tar_bytes, extract_tar_lz4_member, extract_tar_lz4_member_bytes,
    list_tar_lz4_members, verify_tar_lz4, write_tar_lz4, DEFAULT_FRAME_SIZE as TAR_LZ4_FRAME_SIZE,
    FRAME_TABLE_MAGIC as TAR_LZ4_FRAME_TABLE_MAGIC,
};
pub use tar_zstd::{
    decompress_tar_zstd_payload_to_tar_bytes, extract_tar_zstd_member,
    extract_tar_zstd_member_bytes, list_tar_zstd_members, verify_tar_zstd, write_tar_zstd,
    DEFAULT_FRAME_SIZE as TAR_ZSTD_FRAME_SIZE, INDEX_MAGIC as TAR_ZSTD_INDEX_MAGIC,
    INDEX_VERSION as TAR_ZSTD_INDEX_VERSION,
};
