//! Compression method selection for non-solid create.

use crate::error::{Error, Result};

/// Create compression method (per-file packs in non-solid 7z).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressMethod {
    /// LZMA2 (`0x21`) — default; best ratio, slower.
    #[default]
    Lzma2,
    /// Zstandard (`04 F7 11 01`) — best speed×ratio; file-level random access via non-solid packs.
    Zstd,
    /// LZ4 (`04 F7 11 04`) — fastest encode/decode, weaker ratio.
    Lz4,
}

impl CompressMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lzma2 => "lzma2",
            Self::Zstd => "zstd",
            Self::Lz4 => "lz4",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "lzma2" | "lzma" | "7z" => Ok(Self::Lzma2),
            "zstd" | "zst" | "zstandard" => Ok(Self::Zstd),
            "lz4" => Ok(Self::Lz4),
            other => Err(Error::Cli(format!(
                "unknown --method '{other}' (expected lzma2, zstd, or lz4)"
            ))),
        }
    }

    /// 7z coder method id bytes.
    pub fn method_id(self) -> &'static [u8] {
        match self {
            Self::Lzma2 => &[0x21],
            Self::Zstd => &[0x04, 0xF7, 0x11, 0x01],
            Self::Lz4 => &[0x04, 0xF7, 0x11, 0x04],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_aliases() {
        assert_eq!(CompressMethod::parse("zstd").unwrap(), CompressMethod::Zstd);
        assert_eq!(CompressMethod::parse("LZ4").unwrap(), CompressMethod::Lz4);
        assert_eq!(CompressMethod::parse("lzma2").unwrap(), CompressMethod::Lzma2);
        assert!(CompressMethod::parse("brotli").is_err());
    }
}
