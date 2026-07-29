//! Shared tar header / RATAIDX1 member-index helpers for tar.zst and tar.lz4.

use crate::error::{Error, Result};
use crate::select::{meta_owner_mode, names_for_uid_gid, SelectedEntry};
use std::collections::HashSet;
use std::time::UNIX_EPOCH;

/// Magic prefix of the uncompressed tar member index (8 bytes).
/// Plan text said `RATARIDX1` (9 chars); on-disk uses 8-byte `RATAIDX1`.
pub const INDEX_MAGIC: &[u8; 8] = b"RATAIDX1";

/// Index format version.
pub const INDEX_VERSION: u32 = 1;

/// Default permission bits for directory members when the real dir cannot be statted.
pub const DEFAULT_DIR_MODE: u32 = 0o755;

/// Max value storable in a classic ustar 7-digit octal field (mode/uid/gid style).
const USTAR_OCTAL7_MAX: u64 = 0o7777777;

/// Ustar uname (265–296) / gname (297–328) field width in bytes.
const USTAR_UGNAME_LEN: usize = 32;

/// One member in the RATAIDX1 trailer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TarMemberIndexEntry {
    pub name: String,
    /// Uncompressed offset of the ustar (or pax+ustar) header start.
    pub tar_header_offset: u64,
    /// Uncompressed offset of the first file content byte.
    pub tar_data_offset: u64,
    pub data_len: u64,
    pub mode: u32,
    pub mtime_unix: u64,
    pub uid: u32,
    pub gid: u32,
}

/// Full member index for a tar-in-compressed-stream archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TarMemberIndex {
    pub version: u32,
    pub members: Vec<TarMemberIndexEntry>,
}

impl TarMemberIndex {
    pub fn get(&self, name: &str) -> Option<&TarMemberIndexEntry> {
        self.members.iter().find(|m| m.name == name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.members.iter().map(|m| m.name.as_str())
    }
}

/// Metadata written into each tar member header (from [`SelectedEntry`](crate::select::SelectedEntry)).
///
/// `uname` / `gname` are written into ustar fields (32 bytes each). When a name
/// exceeds 32 bytes, the full value is also stored via pax `uname=` / `gname=`.
/// Names are **not** stored in the RATAIDX1 trailer (headers only).
///
/// Directory members use `is_dir = true` (ustar `typeflag` `'5'`, `size` 0).
/// Their archive names should end with `/`.
///
/// Links use `link_target = Some(...)` with size 0:
/// - `is_hard_link = false` → typeflag `'2'` (symlink; target as stored on disk)
/// - `is_hard_link = true` → typeflag `'1'` (hard link; target is first member path)
///
/// The target is written to the ustar `linkname` field (100 bytes) or pax
/// `linkpath=` when longer. Do not set both `is_dir` and `link_target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TarMemberMeta {
    pub size: u64,
    pub mtime: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub uname: String,
    pub gname: String,
    /// When true, emit typeflag `'5'` (directory); size must be 0.
    pub is_dir: bool,
    /// When `Some`, emit typeflag `'1'` (hard) or `'2'` (symlink) per `is_hard_link`.
    pub link_target: Option<String>,
    /// When true with `link_target`, emit hard-link typeflag `'1'`.
    pub is_hard_link: bool,
}

/// Parent directory prefixes of an archive file path, root-first, each ending with `/`.
///
/// Example: `"a/b/c.txt"` → `["a/", "a/b/"]`. Top-level files yield an empty list.
/// Empty directories with no selected files are never produced from this helper.
pub fn parent_dir_names(archive_name: &str) -> Vec<String> {
    let name = archive_name
        .trim_start_matches('/')
        .trim_end_matches('/');
    if name.is_empty() {
        return Vec::new();
    }
    let parts: Vec<&str> = name.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= 1 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(parts.len() - 1);
    let mut acc = String::new();
    for p in &parts[..parts.len() - 1] {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(p);
        out.push(format!("{acc}/"));
    }
    out
}

/// Number of RATAIDX1 members for a tar create: each selected file plus unique
/// parent directory prefixes derived from those files.
pub fn expected_tar_member_count(entries: &[SelectedEntry]) -> usize {
    let mut dirs = HashSet::new();
    for e in entries {
        for d in parent_dir_names(&e.archive_name) {
            dirs.insert(d);
        }
    }
    entries.len() + dirs.len()
}

fn default_dir_meta() -> TarMemberMeta {
    TarMemberMeta {
        size: 0,
        mtime: 0,
        mode: DEFAULT_DIR_MODE,
        uid: 0,
        gid: 0,
        uname: String::new(),
        gname: String::new(),
        is_dir: true,
        link_target: None,
        is_hard_link: false,
    }
}

/// Build directory member metadata for `dir_name` (with trailing `/`) using the
/// real filesystem directory under `entry.abs_path` when possible.
pub fn dir_meta_for_entry(entry: &SelectedEntry, dir_name: &str) -> TarMemberMeta {
    let prefix = dir_name.trim_end_matches('/');
    if prefix.is_empty() {
        return default_dir_meta();
    }
    let file = entry.archive_name.trim_start_matches('/');
    let suffix = match file.strip_prefix(prefix) {
        Some(rest) if rest.starts_with('/') => &rest[1..],
        _ => return default_dir_meta(),
    };
    let hops = suffix.split('/').filter(|s| !s.is_empty()).count();
    if hops == 0 {
        return default_dir_meta();
    }
    let mut abs = entry.abs_path.as_path();
    for _ in 0..hops {
        abs = match abs.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => return default_dir_meta(),
        };
    }
    match std::fs::metadata(abs) {
        Ok(meta) => {
            let (mode, uid, gid) = meta_owner_mode(&meta);
            let (uname, gname) = names_for_uid_gid(uid, gid);
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            TarMemberMeta {
                size: 0,
                mtime,
                mode,
                uid,
                gid,
                uname,
                gname,
                is_dir: true,
                link_target: None,
                is_hard_link: false,
            }
        }
        Err(_) => default_dir_meta(),
    }
}

pub fn truncate_ustar_name(path: &str) -> String {
    let b = path.as_bytes();
    if b.len() <= 100 {
        return path.to_string();
    }
    let start = b.len() - 100;
    let start = (start..b.len())
        .find(|&i| path.is_char_boundary(i))
        .unwrap_or(start);
    path[start..].to_string()
}

fn octal7_fits(val: u64) -> bool {
    val <= USTAR_OCTAL7_MAX
}

/// Build one pax length-prefixed record, adjusting the length field until stable.
fn pax_record(key: &str, value: &str) -> Result<String> {
    // "LEN key=value\n" where LEN is the total record length in decimal.
    let mut len = format!(" {key}={value}\n").len() + 1;
    loop {
        let record = format!("{len} {key}={value}\n");
        if record.len() == len {
            return Ok(record);
        }
        len = record.len();
        if len > 8192 {
            return Err(Error::Archive(format!("pax {key} record too large")));
        }
    }
}

/// Extended header body for path, linkpath, oversized numeric ids, and long uname/gname.
fn pax_body(
    path: Option<&str>,
    linkpath: Option<&str>,
    uid: Option<u32>,
    gid: Option<u32>,
    uname: Option<&str>,
    gname: Option<&str>,
) -> Result<Vec<u8>> {
    let mut body = String::new();
    if let Some(p) = path {
        body.push_str(&pax_record("path", p)?);
    }
    if let Some(p) = linkpath {
        body.push_str(&pax_record("linkpath", p)?);
    }
    if let Some(u) = uid {
        body.push_str(&pax_record("uid", &u.to_string())?);
    }
    if let Some(g) = gid {
        body.push_str(&pax_record("gid", &g.to_string())?);
    }
    if let Some(n) = uname {
        body.push_str(&pax_record("uname", n)?);
    }
    if let Some(n) = gname {
        body.push_str(&pax_record("gname", n)?);
    }
    if body.is_empty() {
        return Err(Error::Archive("empty pax extended header".into()));
    }
    Ok(body.into_bytes())
}

fn pax_extended_block(
    path: Option<&str>,
    linkpath: Option<&str>,
    uid: Option<u32>,
    gid: Option<u32>,
    uname: Option<&str>,
    gname: Option<&str>,
) -> Result<Vec<u8>> {
    let rec_bytes = pax_body(path, linkpath, uid, gid, uname, gname)?;
    let mut out = Vec::new();
    // Pax header itself uses trivial ownership (root / 0644).
    out.extend_from_slice(&ustar_header_raw(
        "PaxHeader",
        "",
        rec_bytes.len() as u64,
        0,
        0o644,
        0,
        0,
        "",
        "",
        b'x',
        "",
    )?);
    out.extend_from_slice(&rec_bytes);
    let pad = (512 - (rec_bytes.len() % 512)) % 512;
    out.resize(out.len() + pad, 0);
    Ok(out)
}

/// True when `name` exceeds the ustar uname/gname field (32 bytes).
fn ugname_needs_pax(name: &str) -> bool {
    name.as_bytes().len() > USTAR_UGNAME_LEN
}

/// Truncate to at most `max` bytes on a UTF-8 char boundary (for ustar fields).
fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.as_bytes().len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn typeflag_for(meta: &TarMemberMeta) -> u8 {
    if meta.is_dir {
        b'5'
    } else if meta.link_target.is_some() {
        if meta.is_hard_link {
            b'1'
        } else {
            b'2'
        }
    } else {
        b'0'
    }
}

/// Ustar linkname field width (bytes); longer targets use pax `linkpath=`.
const USTAR_LINKNAME_LEN: usize = 100;

fn linkname_needs_pax(target: &str) -> bool {
    target.as_bytes().len() > USTAR_LINKNAME_LEN
}

/// Normalize archive path for headers: strip leading `/`; dirs gain a trailing `/`.
fn normalize_member_path(path: &str, is_dir: bool) -> Result<String> {
    let path = path.trim_start_matches('/');
    if path.is_empty() || path == "/" {
        return Err(Error::Archive("empty tar member name".into()));
    }
    if path.contains('\0') {
        return Err(Error::Archive("tar member name contains NUL".into()));
    }
    if is_dir {
        if path.ends_with('/') {
            Ok(path.to_string())
        } else {
            Ok(format!("{path}/"))
        }
    } else {
        Ok(path.trim_end_matches('/').to_string())
    }
}

/// Linkname written into the ustar field (truncated); full target may be in pax.
fn ustar_linkname_field(meta: &TarMemberMeta) -> &str {
    match &meta.link_target {
        Some(t) => truncate_bytes(t, USTAR_LINKNAME_LEN),
        None => "",
    }
}

/// Try to fit path into ustar name (100) + prefix (155).
pub fn try_ustar_header(path: &str, meta: &TarMemberMeta) -> Result<Option<Vec<u8>>> {
    let typeflag = typeflag_for(meta);
    let linkname = ustar_linkname_field(meta);
    let bytes = path.as_bytes();
    if bytes.len() <= 100 {
        let h = ustar_header_raw(
            path,
            "",
            meta.size,
            meta.mtime,
            meta.mode,
            meta.uid,
            meta.gid,
            &meta.uname,
            &meta.gname,
            typeflag,
            linkname,
        )?;
        return Ok(Some(h.to_vec()));
    }
    if bytes.len() > 100 + 155 + 1 {
        return Ok(None);
    }
    for i in (1..bytes.len().min(156)).rev() {
        if bytes[i] != b'/' {
            continue;
        }
        let prefix = &path[..i];
        let name = &path[i + 1..];
        // Directory names may be empty after the last split only if path ends with `/`
        // and we split on the final slash — prefer keeping trailing slash in `name`.
        if name.is_empty() || name.len() > 100 || prefix.len() > 155 {
            continue;
        }
        let h = ustar_header_raw(
            name,
            prefix,
            meta.size,
            meta.mtime,
            meta.mode,
            meta.uid,
            meta.gid,
            &meta.uname,
            &meta.gname,
            typeflag,
            linkname,
        )?;
        return Ok(Some(h.to_vec()));
    }
    Ok(None)
}

/// Build ustar (and optional pax) headers for a file, directory, symlink, or hard-link member.
pub fn build_tar_headers(path: &str, meta: &TarMemberMeta) -> Result<Vec<u8>> {
    if meta.is_dir && meta.link_target.is_some() {
        return Err(Error::Archive(
            "tar member cannot be both directory and link".into(),
        ));
    }
    if meta.is_hard_link && meta.link_target.is_none() {
        return Err(Error::Archive(
            "hard-link tar member requires link_target".into(),
        ));
    }
    if meta.is_dir && meta.size != 0 {
        return Err(Error::Archive(
            "directory tar member must have size 0".into(),
        ));
    }
    if meta.link_target.is_some() && meta.size != 0 {
        return Err(Error::Archive(
            "link tar member must have size 0".into(),
        ));
    }
    if let Some(t) = &meta.link_target {
        if t.is_empty() {
            return Err(Error::Archive("link tar member has empty target".into()));
        }
        if t.contains('\0') {
            return Err(Error::Archive("link target contains NUL".into()));
        }
    }
    let path = normalize_member_path(path, meta.is_dir)?;
    let typeflag = typeflag_for(meta);
    let linkname_field = ustar_linkname_field(meta);

    let path_needs_pax = try_ustar_header(&path, meta)?.is_none();
    let link_needs_pax = meta
        .link_target
        .as_deref()
        .map(linkname_needs_pax)
        .unwrap_or(false);
    let uid_needs_pax = !octal7_fits(meta.uid as u64);
    let gid_needs_pax = !octal7_fits(meta.gid as u64);
    let uname_needs_pax = ugname_needs_pax(&meta.uname);
    let gname_needs_pax = ugname_needs_pax(&meta.gname);

    let mut out = Vec::new();
    if path_needs_pax
        || link_needs_pax
        || uid_needs_pax
        || gid_needs_pax
        || uname_needs_pax
        || gname_needs_pax
    {
        out.extend_from_slice(&pax_extended_block(
            path_needs_pax.then_some(path.as_str()),
            link_needs_pax
                .then(|| meta.link_target.as_deref())
                .flatten(),
            uid_needs_pax.then_some(meta.uid),
            gid_needs_pax.then_some(meta.gid),
            uname_needs_pax.then_some(meta.uname.as_str()),
            gname_needs_pax.then_some(meta.gname.as_str()),
        )?);
    }

    if path_needs_pax {
        let short = truncate_ustar_name(&path);
        out.extend_from_slice(&ustar_header_raw(
            &short,
            "",
            meta.size,
            meta.mtime,
            meta.mode,
            meta.uid,
            meta.gid,
            &meta.uname,
            &meta.gname,
            typeflag,
            linkname_field,
        )?);
    } else if let Some(hdr) = try_ustar_header(&path, meta)? {
        // May still have pax only for oversized ids / long names / long linkpath.
        out.extend_from_slice(&hdr);
    } else {
        return Err(Error::Archive("tar header build failed".into()));
    }
    Ok(out)
}

/// Compatibility wrapper used by older call sites (mode only; uid/gid/names empty).
pub fn build_tar_headers_fixed(path: &str, size: u64, mtime: u64, mode: u32) -> Result<Vec<u8>> {
    build_tar_headers(
        path,
        &TarMemberMeta {
            size,
            mtime,
            mode,
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            is_dir: false,
            link_target: None,
            is_hard_link: false,
        },
    )
}

pub fn ustar_header_raw(
    name: &str,
    prefix: &str,
    size: u64,
    mtime: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    uname: &str,
    gname: &str,
    typeflag: u8,
    linkname: &str,
) -> Result<[u8; 512]> {
    if name.len() > 100 || prefix.len() > 155 {
        return Err(Error::Archive("ustar name/prefix too long".into()));
    }
    if linkname.as_bytes().len() > USTAR_LINKNAME_LEN {
        return Err(Error::Archive("ustar linkname too long".into()));
    }
    let mut h = [0u8; 512];
    write_str_field(&mut h[0..100], name);
    write_octal_field(&mut h[100..108], (mode as u64) & USTAR_OCTAL7_MAX, 7);
    // Oversize uid/gid are also carried in pax; clamp classic fields to fit.
    let uid_field = if octal7_fits(uid as u64) {
        uid as u64
    } else {
        USTAR_OCTAL7_MAX
    };
    let gid_field = if octal7_fits(gid as u64) {
        gid as u64
    } else {
        USTAR_OCTAL7_MAX
    };
    write_octal_field(&mut h[108..116], uid_field, 7);
    write_octal_field(&mut h[116..124], gid_field, 7);
    write_octal_field(&mut h[124..136], size, 11);
    write_octal_field(&mut h[136..148], mtime, 11);
    // checksum field temporarily spaces
    h[148..156].fill(b' ');
    h[156] = typeflag;
    // linkname 157-256 (100 bytes)
    write_str_field(&mut h[157..257], linkname);
    h[257..262].copy_from_slice(b"ustar");
    h[262] = 0;
    h[263] = b'0';
    h[264] = b'0';
    // uname 265-296 / gname 297-328 (truncated to 32 bytes; full name in pax if longer)
    write_str_field(&mut h[265..297], truncate_bytes(uname, USTAR_UGNAME_LEN));
    write_str_field(&mut h[297..329], truncate_bytes(gname, USTAR_UGNAME_LEN));
    write_str_field(&mut h[345..500], prefix);

    let sum: u32 = h.iter().map(|&b| b as u32).sum();
    // 6 octal digits, null, space — common variant
    let ck = format!("{sum:06o}\0 ");
    let ck_b = ck.as_bytes();
    h[148..148 + ck_b.len().min(8)].copy_from_slice(&ck_b[..ck_b.len().min(8)]);
    Ok(h)
}

fn write_str_field(dst: &mut [u8], s: &str) {
    let b = s.as_bytes();
    let n = b.len().min(dst.len());
    dst[..n].copy_from_slice(&b[..n]);
}

/// Write octal ASCII into field of `width` digits + trailing NUL (classic tar).
fn write_octal_field(dst: &mut [u8], val: u64, digits: usize) {
    let width = dst.len();
    let s = format!("{val:0digits$o}");
    let s = if s.len() > digits {
        format!("{val:o}")
    } else {
        s
    };
    let b = s.as_bytes();
    if b.len() >= width {
        let start = b.len() + 1 - width;
        dst[..width - 1].copy_from_slice(&b[start..b.len()]);
        dst[width - 1] = 0;
    } else {
        dst[..b.len()].copy_from_slice(b);
        dst[b.len()] = 0;
    }
}

pub fn encode_index(members: &[TarMemberIndexEntry]) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    buf.extend_from_slice(INDEX_MAGIC);
    buf.extend_from_slice(&INDEX_VERSION.to_le_bytes());
    buf.extend_from_slice(&(members.len() as u64).to_le_bytes());
    for m in members {
        let name_bytes = m.name.as_bytes();
        buf.extend_from_slice(&(name_bytes.len() as u64).to_le_bytes());
        buf.extend_from_slice(name_bytes);
        buf.extend_from_slice(&m.tar_header_offset.to_le_bytes());
        buf.extend_from_slice(&m.tar_data_offset.to_le_bytes());
        buf.extend_from_slice(&m.data_len.to_le_bytes());
        buf.extend_from_slice(&m.mode.to_le_bytes());
        buf.extend_from_slice(&m.mtime_unix.to_le_bytes());
        buf.extend_from_slice(&m.uid.to_le_bytes());
        buf.extend_from_slice(&m.gid.to_le_bytes());
    }
    Ok(buf)
}

pub fn parse_index(buf: &[u8]) -> Result<TarMemberIndex> {
    if buf.len() < 8 + 4 + 8 {
        return Err(Error::Archive("tar index truncated (header)".into()));
    }
    if &buf[0..8] != INDEX_MAGIC {
        return Err(Error::Archive(
            "tar index: bad magic (expected RATAIDX1)".into(),
        ));
    }
    let version = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    if version != INDEX_VERSION {
        return Err(Error::Archive(format!(
            "tar index: unsupported version {version}"
        )));
    }
    let count = u64::from_le_bytes(buf[12..20].try_into().unwrap());
    let mut pos = 20usize;
    let mut members = Vec::with_capacity(count as usize);
    for i in 0..count {
        if pos + 8 > buf.len() {
            return Err(Error::Archive(format!(
                "tar index truncated at member {i} name_len"
            )));
        }
        let name_len = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap()) as usize;
        pos += 8;
        // name + header_off + data_off + data_len + mode + mtime + uid + gid
        let need = name_len + 8 + 8 + 8 + 4 + 8 + 4 + 4;
        if pos + need > buf.len() {
            return Err(Error::Archive(format!(
                "tar index truncated at member {i} body"
            )));
        }
        let name = std::str::from_utf8(&buf[pos..pos + name_len])
            .map_err(|e| Error::Archive(format!("tar index: invalid UTF-8 name: {e}")))?
            .to_string();
        pos += name_len;
        let tar_header_offset = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let tar_data_offset = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let data_len = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let mode = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let mtime_unix = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let uid = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
        pos += 4;
        let gid = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
        pos += 4;
        members.push(TarMemberIndexEntry {
            name,
            tar_header_offset,
            tar_data_offset,
            data_len,
            mode,
            mtime_unix,
            uid,
            gid,
        });
    }
    if pos != buf.len() {
        return Err(Error::Archive(format!(
            "tar index has {} trailing bytes",
            buf.len() - pos
        )));
    }
    Ok(TarMemberIndex { version, members })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field_cstr(buf: &[u8]) -> &str {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        std::str::from_utf8(&buf[..end]).unwrap()
    }

    fn file_meta(size: u64, mode: u32, uid: u32, gid: u32) -> TarMemberMeta {
        TarMemberMeta {
            size,
            mtime: 0,
            mode,
            uid,
            gid,
            uname: String::new(),
            gname: String::new(),
            is_dir: false,
            link_target: None,
            is_hard_link: false,
        }
    }

    #[test]
    fn header_embeds_mode_uid_gid() {
        let mut meta = file_meta(3, 0o750, 1000, 1001);
        meta.mtime = 1_700_000_000;
        let hdr = build_tar_headers("file.txt", &meta).unwrap();
        assert_eq!(hdr.len(), 512);
        assert_eq!(hdr[156], b'0');
        // mode field 100..108 is octal
        let mode_field = std::str::from_utf8(&hdr[100..108])
            .unwrap()
            .trim_end_matches('\0')
            .trim();
        assert_eq!(u32::from_str_radix(mode_field, 8).unwrap(), 0o750);
        let uid_field = std::str::from_utf8(&hdr[108..116])
            .unwrap()
            .trim_end_matches('\0')
            .trim();
        assert_eq!(u32::from_str_radix(uid_field, 8).unwrap(), 1000);
        let gid_field = std::str::from_utf8(&hdr[116..124])
            .unwrap()
            .trim_end_matches('\0')
            .trim();
        assert_eq!(u32::from_str_radix(gid_field, 8).unwrap(), 1001);
    }

    #[test]
    fn dir_header_typeflag_5_trailing_slash() {
        let meta = TarMemberMeta {
            size: 0,
            mtime: 42,
            mode: 0o755,
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            is_dir: true,
            link_target: None,
            is_hard_link: false,
        };
        let hdr = build_tar_headers("a/b", &meta).unwrap();
        assert_eq!(hdr.len(), 512);
        assert_eq!(hdr[156], b'5');
        assert_eq!(field_cstr(&hdr[0..100]), "a/b/");
        let size_field = std::str::from_utf8(&hdr[124..136])
            .unwrap()
            .trim_end_matches('\0')
            .trim();
        assert_eq!(u64::from_str_radix(size_field, 8).unwrap(), 0);
        // Already-trailing slash is preserved (not doubled).
        let hdr2 = build_tar_headers("a/b/", &meta).unwrap();
        assert_eq!(field_cstr(&hdr2[0..100]), "a/b/");
    }

    #[test]
    fn parent_dir_names_nested() {
        assert!(parent_dir_names("c.txt").is_empty());
        assert_eq!(parent_dir_names("a/b/c.txt"), vec!["a/".to_string(), "a/b/".to_string()]);
        assert_eq!(parent_dir_names("/a/b/c.txt"), vec!["a/".to_string(), "a/b/".to_string()]);
    }

    #[test]
    fn expected_member_count_includes_unique_dirs() {
        use std::path::PathBuf;
        let e = |name: &str| SelectedEntry {
            abs_path: PathBuf::from(format!("/tmp/{name}")),
            archive_name: name.into(),
            size: 1,
            mtime_unix: None,
            mode: 0o644,
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            kind: crate::select::MemberKind::File,
        };
        // a/b/c.txt → a/, a/b/; a/d.txt → a/ (shared); top.txt → none
        let entries = vec![e("a/b/c.txt"), e("a/d.txt"), e("top.txt")];
        assert_eq!(expected_tar_member_count(&entries), 3 + 2); // files + a/, a/b/
    }

    #[test]
    fn header_embeds_uname_gname() {
        let mut meta = file_meta(1, 0o644, 1000, 100);
        meta.uname = "alice".into();
        meta.gname = "staff".into();
        let hdr = build_tar_headers("f.txt", &meta).unwrap();
        assert_eq!(hdr.len(), 512);
        assert_eq!(field_cstr(&hdr[265..297]), "alice");
        assert_eq!(field_cstr(&hdr[297..329]), "staff");
    }

    #[test]
    fn long_uname_uses_pax() {
        let long = "u".repeat(40); // > 32
        let mut meta = file_meta(1, 0o644, 1, 1);
        meta.uname = long.clone();
        let hdr = build_tar_headers("x", &meta).unwrap();
        assert!(hdr.len() > 512);
        assert_eq!(hdr[156], b'x');
        let body = String::from_utf8_lossy(&hdr[512..]);
        assert!(body.contains(&format!("uname={long}")));
        // Ustar member header still carries truncated uname.
        let ustar = &hdr[hdr.len() - 512..];
        assert_eq!(field_cstr(&ustar[265..297]), &long[..32]);
    }

    #[test]
    fn large_uid_uses_pax() {
        let meta = file_meta(1, 0o644, 3_000_000, 0); // uid > 0o7777777
        let hdr = build_tar_headers("x", &meta).unwrap();
        assert!(hdr.len() > 512);
        // First header is pax typeflag 'x'
        assert_eq!(hdr[156], b'x');
        let body = String::from_utf8_lossy(&hdr[512..]);
        assert!(body.contains("uid=3000000"));
    }

    #[test]
    fn symlink_header_typeflag_2_and_linkname() {
        let meta = TarMemberMeta {
            size: 0,
            mtime: 99,
            mode: 0o777,
            uid: 1000,
            gid: 100,
            uname: String::new(),
            gname: String::new(),
            is_dir: false,
            link_target: Some("target.txt".into()),
            is_hard_link: false,
        };
        let hdr = build_tar_headers("link.txt", &meta).unwrap();
        assert_eq!(hdr.len(), 512);
        assert_eq!(hdr[156], b'2');
        assert_eq!(field_cstr(&hdr[0..100]), "link.txt");
        assert_eq!(field_cstr(&hdr[157..257]), "target.txt");
        let size_field = std::str::from_utf8(&hdr[124..136])
            .unwrap()
            .trim_end_matches('\0')
            .trim();
        assert_eq!(u64::from_str_radix(size_field, 8).unwrap(), 0);
    }

    #[test]
    fn hardlink_header_typeflag_1_and_linkname() {
        let meta = TarMemberMeta {
            size: 0,
            mtime: 99,
            mode: 0o644,
            uid: 1000,
            gid: 100,
            uname: String::new(),
            gname: String::new(),
            is_dir: false,
            link_target: Some("a.txt".into()),
            is_hard_link: true,
        };
        let hdr = build_tar_headers("b.txt", &meta).unwrap();
        assert_eq!(hdr.len(), 512);
        assert_eq!(hdr[156], b'1');
        assert_eq!(field_cstr(&hdr[0..100]), "b.txt");
        assert_eq!(field_cstr(&hdr[157..257]), "a.txt");
        let size_field = std::str::from_utf8(&hdr[124..136])
            .unwrap()
            .trim_end_matches('\0')
            .trim();
        assert_eq!(u64::from_str_radix(size_field, 8).unwrap(), 0);
    }

    #[test]
    fn long_link_target_uses_pax_linkpath() {
        let long_target = format!("dir/{}", "t".repeat(120));
        let meta = TarMemberMeta {
            size: 0,
            mtime: 1,
            mode: 0o777,
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            is_dir: false,
            link_target: Some(long_target.clone()),
            is_hard_link: false,
        };
        let hdr = build_tar_headers("l", &meta).unwrap();
        assert!(hdr.len() > 512);
        assert_eq!(hdr[156], b'x'); // pax
        let body = String::from_utf8_lossy(&hdr[512..]);
        assert!(body.contains(&format!("linkpath={long_target}")));
        // Ustar member header (last 512) has typeflag 2 and truncated linkname
        let ustar = &hdr[hdr.len() - 512..];
        assert_eq!(ustar[156], b'2');
        assert_eq!(
            field_cstr(&ustar[157..257]),
            &long_target[..USTAR_LINKNAME_LEN.min(long_target.len())]
        );
    }

    #[test]
    fn index_roundtrip_includes_owner() {
        let members = vec![TarMemberIndexEntry {
            name: "a".into(),
            tar_header_offset: 0,
            tar_data_offset: 512,
            data_len: 1,
            mode: 0o640,
            mtime_unix: 99,
            uid: 42,
            gid: 43,
        }];
        let bytes = encode_index(&members).unwrap();
        let parsed = parse_index(&bytes).unwrap();
        assert_eq!(parsed.members[0].mode, 0o640);
        assert_eq!(parsed.members[0].uid, 42);
        assert_eq!(parsed.members[0].gid, 43);
    }
}
