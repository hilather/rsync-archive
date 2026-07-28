//! Non-solid 7z header layout, Copy store writer, and multi-method create writer.

mod codec;
mod header;
mod lzma2_writer;
mod method;
mod store_writer;

pub use codec::{
    compress_bytes, compress_path, compress_path_with_size, compress_reader,
    compress_reader_append_pack, compress_reader_append_pack_sized, compress_reader_to_writer,
    dict_size_for_level, dict_size_for_member, lz4_hc_available, lz4_level_byte, lzma2_backend_name,
    lzma2_dict_prop, zstd_level, CompressedPack, Lzma2Compressed, PackCrcWriter,
};
pub use header::{
    filetime_from_unix_secs, filetime_now, write_raw_header, write_start_header, write_u64,
    HeaderFile, ATTR_FILE, SIG, SIG_HEADER_SIZE, K_CODERS_UNPACK_SIZE, K_CRC, K_EMPTY_FILE,
    K_EMPTY_STREAM, K_END, K_FILES_INFO, K_FOLDER, K_HEADER, K_MAIN_STREAMS_INFO, K_M_TIME, K_NAME,
    K_PACK_INFO, K_SIZE, K_SUB_STREAMS_INFO, K_UNPACK_INFO, K_WIN_ATTRIBUTES,
};
pub use lzma2_writer::NonsolidLzma2Writer;
pub use method::CompressMethod;
pub use store_writer::NonsolidStoreWriter;
