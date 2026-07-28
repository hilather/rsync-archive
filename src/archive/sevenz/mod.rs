//! Non-solid 7z header layout, Copy store writer, and LZMA2 create writer.

mod codec;
mod header;
mod lzma2_writer;
mod store_writer;

pub use codec::{
    compress_bytes, compress_path, compress_reader, compress_reader_append_pack,
    compress_reader_to_writer, dict_size_for_level, lzma2_dict_prop, options_for_level,
    Lzma2Compressed, PackCrcWriter,
};
pub use header::{
    filetime_from_unix_secs, filetime_now, write_raw_header, write_start_header, write_u64,
    HeaderFile, ATTR_FILE, SIG, SIG_HEADER_SIZE, K_CODERS_UNPACK_SIZE, K_CRC, K_EMPTY_FILE,
    K_EMPTY_STREAM, K_END, K_FILES_INFO, K_FOLDER, K_HEADER, K_MAIN_STREAMS_INFO, K_M_TIME, K_NAME,
    K_PACK_INFO, K_SIZE, K_SUB_STREAMS_INFO, K_UNPACK_INFO, K_WIN_ATTRIBUTES,
};
pub use lzma2_writer::NonsolidLzma2Writer;
pub use store_writer::NonsolidStoreWriter;
