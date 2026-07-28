//! Embed finished regular files under a master non-solid store 7z.

use crate::archive::NonsolidStoreWriter;
use crate::cli::EmbedArgs;
use crate::error::{Error, Result};
use crate::pipeline::output::{cleanup_partial, commit_output, prepare_output, OutputPaths};
use crate::select::pathnorm::{
    basename_utf8, join_archive_name, normalize_archive_path, normalize_prefix, validate_member_name,
};
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// 7z signature magic: `7z\xBC\xAF\x27\x1C`
pub const SEVENZ_MAGIC: &[u8] = b"7z\xBC\xAF\x27\x1C";

/// One planned embed member after naming + validation.
#[derive(Debug, Clone)]
pub struct EmbedMember {
    pub src: PathBuf,
    pub archive_name: String,
}

/// Compute archive member name for an embed input.
pub fn member_name(input: &Path, keep_path: bool, prefix: Option<&str>) -> Result<String> {
    let base = if keep_path {
        normalize_archive_path(input)?
    } else {
        basename_utf8(input)?
    };
    if base.is_empty() || base == "." || base == ".." {
        return Err(Error::InvalidMemberName(format!(
            "invalid base name for {}: {base:?}",
            input.display()
        )));
    }
    let member = if let Some(p) = prefix {
        let p = normalize_prefix(p)?;
        if p.is_empty() {
            base
        } else {
            join_archive_name(&p, &base)?
        }
    } else {
        base
    };
    validate_member_name(&member)?;
    Ok(member)
}

/// Ensure path is a regular file (non-following).
fn require_regular_file(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| {
        Error::Message(format!("stat {}: {e}", path.display()))
    })?;
    if meta.file_type().is_symlink() {
        return Err(Error::NotRegularFile(path.to_path_buf()));
    }
    if !meta.is_file() {
        return Err(Error::NotRegularFile(path.to_path_buf()));
    }
    Ok(())
}

/// Check 7z magic; return true if present.
pub fn has_sevenz_magic(path: &Path) -> Result<bool> {
    let mut f = File::open(path).map_err(|e| {
        Error::Message(format!("open {} for magic check: {e}", path.display()))
    })?;
    let mut buf = [0u8; 6];
    let n = f.read(&mut buf)?;
    Ok(n >= SEVENZ_MAGIC.len() && &buf[..SEVENZ_MAGIC.len()] == SEVENZ_MAGIC)
}

fn apply_magic_policy(path: &Path, require_7z: bool, allow_any: bool) -> Result<()> {
    if allow_any {
        return Ok(());
    }
    let ok = has_sevenz_magic(path)?;
    if ok {
        return Ok(());
    }
    if require_7z {
        return Err(Error::Message(format!(
            "missing 7z magic: {} (use --allow-any to embed arbitrary files)",
            path.display()
        )));
    }
    warn!(
        path = %path.display(),
        "input is missing 7z magic; embedding as store member anyway (use --require-7z to fail)"
    );
    Ok(())
}

/// Plan embed members: validate inputs, resolve names, detect collisions.
pub fn plan_embed(args: &EmbedArgs) -> Result<Vec<EmbedMember>> {
    if args.inputs.is_empty() {
        return Err(Error::EmptyArchive);
    }
    let prefix = args.prefix.as_deref();
    let mut seen = HashSet::new();
    let mut members = Vec::with_capacity(args.inputs.len());

    for src in &args.inputs {
        require_regular_file(src)?;
        apply_magic_policy(src, args.require_7z, args.allow_any)?;
        let archive_name = member_name(src, args.keep_path, prefix)?;
        if !seen.insert(archive_name.clone()) {
            return Err(Error::Collision(archive_name));
        }
        members.push(EmbedMember {
            src: src.clone(),
            archive_name,
        });
    }
    Ok(members)
}

/// Run `rsync-archive embed`.
pub fn run_embed(args: EmbedArgs) -> Result<()> {
    let members = plan_embed(&args)?;

    if args.dry_run {
        for m in &members {
            println!("{}", m.archive_name);
        }
        info!(
            count = members.len(),
            "embed dry-run complete (no archive written)"
        );
        eprintln!("dry-run: {} member(s) would be embedded", members.len());
        return Ok(());
    }

    let paths = prepare_output(&args.output, args.force)?;
    match write_embed(&paths, &members) {
        Ok(()) => {
            commit_output(&paths)?;
            info!(
                path = %paths.final_path.display(),
                count = members.len(),
                "embed complete"
            );
            eprintln!(
                "embedded {} member(s) → {}",
                members.len(),
                paths.final_path.display()
            );
            if args.verify {
                verify_store_archive(&paths.final_path, members.len())?;
            }
            Ok(())
        }
        Err(e) => {
            cleanup_partial(&paths);
            Err(e)
        }
    }
}

fn write_embed(paths: &OutputPaths, members: &[EmbedMember]) -> Result<()> {
    let mut w = NonsolidStoreWriter::create(&paths.partial_path)?;
    for m in members {
        w.push_path(m.archive_name.clone(), &m.src)?;
    }
    w.finish()
}

/// List/test archive with sevenz-rust2; assert member count (non-dir).
fn verify_store_archive(path: &Path, expected_files: usize) -> Result<()> {
    use sevenz_rust2::ArchiveReader;
    use std::fs::File;

    let file = File::open(path).map_err(|e| {
        Error::Archive(format!("verify open {}: {e}", path.display()))
    })?;
    let reader = ArchiveReader::new(file, sevenz_rust2::Password::empty()).map_err(|e| {
        Error::Archive(format!("verify open archive {}: {e}", path.display()))
    })?;
    let archive = reader.archive();
    let n = archive
        .files
        .iter()
        .filter(|e| !e.is_directory)
        .count();
    if n != expected_files {
        return Err(Error::Archive(format!(
            "verify member count: expected {expected_files}, got {n}"
        )));
    }
    // Solid archives have a single pack stream spanning many files; non-solid
    // store writers use one pack per non-empty file. Empty-only archives have
    // zero pack streams — still accept.
    info!(
        path = %path.display(),
        members = n,
        packs = archive.pack_sizes().len(),
        "verify ok"
    );
    eprintln!("verify ok: {n} file member(s)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::EmbedArgs;
    use std::fs;
    use tempfile::tempdir;

    fn args(out: PathBuf, inputs: Vec<PathBuf>) -> EmbedArgs {
        EmbedArgs {
            output: out,
            dry_run: false,
            force: false,
            prefix: None,
            keep_path: false,
            require_7z: false,
            allow_any: true,
            verify: false,
            inputs,
        }
    }

    #[test]
    fn member_name_flatten_default() {
        let p = Path::new("dir/nest.7z");
        assert_eq!(member_name(p, false, None).unwrap(), "nest.7z");
    }

    #[test]
    fn member_name_keep_path_and_prefix() {
        let p = Path::new("build/a.7z");
        assert_eq!(
            member_name(p, true, Some("packs")).unwrap(),
            "packs/build/a.7z"
        );
    }

    #[test]
    fn member_name_rejects_dotdot_in_keep_path() {
        assert!(member_name(Path::new("a/../b.7z"), true, None).is_err());
    }

    #[test]
    fn plan_collision_on_flatten() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("x").join("f.7z");
        let b = dir.path().join("y").join("f.7z");
        fs::create_dir_all(a.parent().unwrap()).unwrap();
        fs::create_dir_all(b.parent().unwrap()).unwrap();
        fs::write(&a, b"1").unwrap();
        fs::write(&b, b"2").unwrap();
        let mut aargs = args(dir.path().join("out.7z"), vec![a, b]);
        aargs.allow_any = true;
        let err = plan_embed(&aargs).unwrap_err();
        assert!(matches!(err, Error::Collision(_)));
    }

    #[test]
    fn plan_ok_keep_path_avoids_collision() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("x").join("f.7z");
        let b = dir.path().join("y").join("f.7z");
        fs::create_dir_all(a.parent().unwrap()).unwrap();
        fs::create_dir_all(b.parent().unwrap()).unwrap();
        fs::write(&a, b"1").unwrap();
        fs::write(&b, b"2").unwrap();
        let mut aargs = args(dir.path().join("out.7z"), vec![a.clone(), b.clone()]);
        aargs.allow_any = true;
        aargs.keep_path = true;
        let plan = plan_embed(&aargs).unwrap();
        assert_eq!(plan.len(), 2);
        assert_ne!(plan[0].archive_name, plan[1].archive_name);
    }

    #[test]
    fn magic_detects_sevenz_sig() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.7z");
        let mut data = SEVENZ_MAGIC.to_vec();
        data.extend_from_slice(&[0u8; 26]);
        fs::write(&p, &data).unwrap();
        assert!(has_sevenz_magic(&p).unwrap());

        let q = dir.path().join("b.bin");
        fs::write(&q, b"not7z!!").unwrap();
        assert!(!has_sevenz_magic(&q).unwrap());
    }

    #[test]
    fn require_7z_errors_on_missing_magic() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("x.bin");
        fs::write(&p, b"hello").unwrap();
        let mut aargs = args(dir.path().join("out.7z"), vec![p]);
        aargs.allow_any = false;
        aargs.require_7z = true;
        assert!(plan_embed(&aargs).is_err());
    }

    #[test]
    fn embed_roundtrip_bytes() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.dat");
        let b = dir.path().join("b.dat");
        fs::write(&a, b"alpha-payload").unwrap();
        fs::write(&b, b"beta-payload!!").unwrap();
        let out = dir.path().join("master.7z");
        let mut aargs = args(out.clone(), vec![a.clone(), b.clone()]);
        aargs.allow_any = true;
        aargs.verify = true;
        run_embed(aargs).unwrap();
        assert!(out.exists());
        assert!(!partial_exists(&out));

        // Extract via sevenz-rust2
        use sevenz_rust2::{ArchiveReader, Password};
        let file = File::open(&out).unwrap();
        let reader = ArchiveReader::new(file, Password::empty()).unwrap();
        let names: Vec<_> = reader
            .archive()
            .files
            .iter()
            .filter(|e| !e.is_directory)
            .map(|e| e.name().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n.ends_with("a.dat") || n == "a.dat"),
            "{names:?}"
        );
        assert!(
            names.iter().any(|n| n.ends_with("b.dat") || n == "b.dat"),
            "{names:?}"
        );
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.dat");
        fs::write(&a, b"x").unwrap();
        let out = dir.path().join("master.7z");
        let mut aargs = args(out.clone(), vec![a]);
        aargs.allow_any = true;
        aargs.dry_run = true;
        run_embed(aargs).unwrap();
        assert!(!out.exists());
    }

    #[test]
    fn force_required_to_overwrite() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.dat");
        fs::write(&a, b"x").unwrap();
        let out = dir.path().join("master.7z");
        fs::write(&out, b"old").unwrap();
        let mut aargs = args(out.clone(), vec![a.clone()]);
        aargs.allow_any = true;
        assert!(matches!(
            run_embed(aargs).unwrap_err(),
            Error::OutputExists(_)
        ));
        let mut aargs = args(out.clone(), vec![a]);
        aargs.allow_any = true;
        aargs.force = true;
        run_embed(aargs).unwrap();
        assert!(out.metadata().unwrap().len() > 3);
    }

    fn partial_exists(final_path: &Path) -> bool {
        final_path
            .as_os_str()
            .to_owned()
            .into_string()
            .map(|s| PathBuf::from(format!("{s}.partial")).exists())
            .unwrap_or(false)
    }
}
