use std::ffi::OsString;
use std::path::{Path, PathBuf};

use flate2::write::GzEncoder;
use flate2::Compression;

use crate::{tools, PushArgs};

pub fn run(args: &PushArgs) -> anyhow::Result<()> {
    tools::require("oras")?;

    let dir = args
        .dir
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("directory not found: {}", args.dir.display()))?;

    let disk_path = dir.join("disk.raw");
    if !disk_path.exists() {
        anyhow::bail!(
            "disk.raw not found in {}. Run `steep build` first.",
            dir.display()
        );
    }

    // Collect all regular files, skipping symlinks
    let mut files: Vec<OsString> = Vec::new();
    for entry in fs_err::read_dir(&dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            files.push(entry.file_name());
        }
    }
    files.sort();

    if files.is_empty() {
        anyhow::bail!("no files found in {}", dir.display());
    }

    let name = args
        .name
        .clone()
        .unwrap_or_else(|| args.dir.file_name().unwrap().to_string_lossy().to_string());
    let image_ref = format!("{}/{}:{}", args.registry, name, args.tag);

    if args.cdi {
        push_cdi(&dir, &files, &image_ref)
    } else {
        push_default(&dir, &files, &image_ref)
    }
}

fn push_default(dir: &Path, files: &[OsString], image_ref: &str) -> anyhow::Result<()> {
    println!("Pushing {} files to {}", files.len(), image_ref);
    for f in files {
        println!("  {}", f.to_string_lossy());
    }

    let mut oras_args: Vec<OsString> = vec![
        "push".into(),
        image_ref.into(),
        "--artifact-type".into(),
        "application/vnd.steep.image.v1".into(),
    ];
    oras_args.extend(files.iter().cloned());

    tools::run_command_streaming_in("oras", &oras_args, dir.to_path_buf())?;

    println!("Pushed successfully.");
    Ok(())
}

fn push_cdi(dir: &Path, files: &[OsString], image_ref: &str) -> anyhow::Result<()> {
    println!(
        "Pushing CDI-compatible single-layer image ({} files) to {}",
        files.len(),
        image_ref
    );
    for f in files {
        println!("  {}", f.to_string_lossy());
    }

    // Stream to a temp tarball — honors TMPDIR via std::env::temp_dir().
    let tmp_root = std::env::temp_dir();
    fs_err::create_dir_all(&tmp_root)?;
    let tarball = tempfile::Builder::new()
        .prefix("steep-cdi-")
        .suffix(".tar.gz")
        .tempfile_in(&tmp_root)?;
    let tarball_path: PathBuf = tarball.path().to_path_buf();

    println!("Staging tarball at {}", tarball_path.display());
    build_cdi_tarball(dir, files, &tarball_path)?;

    // Run from the tarball's parent so the layer's "filename" in the manifest
    // is just the basename, matching the historical CDI-compatible layout.
    // oras also rejects absolute paths as layer args (path validation), so the
    // layer must be referenced by its basename relative to the cwd.
    let cwd = tarball_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let tarball_basename = tarball_path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("tarball path has no basename: {}", tarball_path.display()))?;

    // Stage a minimal OCI image config alongside the tarball. CDI's
    // registry importer rejects artifacts whose top-level `artifactType`
    // is set (it only accepts standard OCI image manifests), so we push
    // a real image config blob and skip `--artifact-type`. Body matches
    // what containerd would produce for an empty rootfs.
    let config_path = cwd.join("steep-cdi-config.json");
    fs_err::write(
        &config_path,
        br#"{"architecture":"amd64","os":"linux","config":{},"rootfs":{"type":"layers","diff_ids":[]}}"#,
    )?;
    let config_basename = config_path.file_name().unwrap().to_owned();
    let config_arg = {
        let mut s = OsString::from(&config_basename);
        s.push(":application/vnd.oci.image.config.v1+json");
        s
    };

    let layer_arg = {
        let mut s = OsString::from(tarball_basename);
        s.push(":application/vnd.oci.image.layer.v1.tar+gzip");
        s
    };
    let oras_args: Vec<OsString> = vec![
        "push".into(),
        image_ref.into(),
        "--config".into(),
        config_arg,
        layer_arg,
    ];
    let push_res = tools::run_command_streaming_in("oras", &oras_args, cwd.clone());
    let _ = fs_err::remove_file(&config_path);
    push_res?;

    println!("Pushed successfully.");
    Ok(())
}

/// Build a CDI-compatible tarball at `out_path`.
///
/// Layout:
/// - `disk/disk.raw` (the raw disk image, the only file CDI's importer looks for)
/// - all other regular files at the root of the tar (metadata: OVMF.fd, manifest.json,
///   roothash, uki.efi, etc.)
///
/// Streams from disk into a gzipped tar file — never buffers a layer in RAM.
pub(crate) fn build_cdi_tarball(
    dir: &Path,
    files: &[OsString],
    out_path: &Path,
) -> anyhow::Result<()> {
    let out = fs_err::File::create(out_path)?;
    let gz = GzEncoder::new(out, Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.follow_symlinks(false);

    for f in files {
        let src = dir.join(f);
        let name = f.to_string_lossy();
        let arcname = if name == "disk.raw" {
            "disk/disk.raw".to_string()
        } else {
            name.into_owned()
        };
        let mut file = fs_err::File::open(&src)?;
        tar.append_file(&arcname, file.file_mut())?;
    }

    let gz = tar.into_inner()?;
    gz.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::collections::BTreeSet;
    use tempfile::TempDir;

    fn write(dir: &Path, name: &str, bytes: &[u8]) {
        fs_err::write(dir.join(name), bytes).unwrap();
    }

    #[test]
    fn cdi_tarball_layout_places_disk_under_disk_dir_and_others_at_root() {
        let src = TempDir::new().unwrap();
        write(src.path(), "disk.raw", b"RAWDISK-CONTENT");
        write(src.path(), "OVMF.fd", b"firmware");
        write(src.path(), "manifest.json", b"{}");
        write(src.path(), "roothash", b"deadbeef");
        write(src.path(), "uki.efi", b"efi");

        let files: Vec<OsString> = vec![
            "OVMF.fd".into(),
            "disk.raw".into(),
            "manifest.json".into(),
            "roothash".into(),
            "uki.efi".into(),
        ];

        let out_dir = TempDir::new().unwrap();
        let out = out_dir.path().join("layer.tar.gz");
        build_cdi_tarball(src.path(), &files, &out).unwrap();

        // Re-open and inspect.
        let f = fs_err::File::open(&out).unwrap();
        let gz = GzDecoder::new(f);
        let mut ar = tar::Archive::new(gz);

        let mut names: BTreeSet<String> = BTreeSet::new();
        let mut disk_contents: Option<Vec<u8>> = None;
        for entry in ar.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().to_path_buf();
            let name = path.to_string_lossy().into_owned();
            if name == "disk/disk.raw" {
                let mut buf = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut buf).unwrap();
                disk_contents = Some(buf);
            }
            names.insert(name);
        }

        assert!(
            names.contains("disk/disk.raw"),
            "expected disk/disk.raw in tar, got: {:?}",
            names
        );
        assert!(names.contains("OVMF.fd"), "OVMF.fd should be at tar root");
        assert!(
            names.contains("manifest.json"),
            "manifest.json should be at tar root"
        );
        assert!(names.contains("roothash"), "roothash should be at tar root");
        assert!(names.contains("uki.efi"), "uki.efi should be at tar root");

        // Nothing else snuck under disk/.
        let under_disk: Vec<_> = names
            .iter()
            .filter(|n| n.starts_with("disk/"))
            .cloned()
            .collect();
        assert_eq!(under_disk, vec!["disk/disk.raw".to_string()]);

        assert_eq!(disk_contents.as_deref(), Some(&b"RAWDISK-CONTENT"[..]));
    }
}
