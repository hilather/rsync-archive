//! Non-solid 7z header layout and Copy (store) writer.
//!
//! Foundation for **embed** (store whole member blobs). Streaming LZMA2 create
//! lands in a later stage.

mod header;
mod store_writer;

pub use header::{
    filetime_now, write_raw_header, write_start_header, write_u64, HeaderFile, ATTR_FILE,
    SIG, SIG_HEADER_SIZE, K_CODERS_UNPACK_SIZE, K_CRC, K_EMPTY_FILE, K_EMPTY_STREAM, K_END,
    K_FILES_INFO, K_FOLDER, K_HEADER, K_MAIN_STREAMS_INFO, K_M_TIME, K_NAME, K_PACK_INFO, K_SIZE,
    K_SUB_STREAMS_INFO, K_UNPACK_INFO, K_WIN_ATTRIBUTES,
};
pub use store_writer::NonsolidStoreWriter;
