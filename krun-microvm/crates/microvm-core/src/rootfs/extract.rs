use super::tar_security::{ensure_no_symlink_parents, sanitize_tar_path, validate_symlink_target};
use super::whiteout::{apply_whiteout, is_whiteout};
use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use tar::{Archive, EntryType};

/// Maximum allowed total uncompressed extraction size (30 GiB limit to prevent zip bombs)
const MAX_EXTRACTION_BYTES: u64 = 30 * 1024 * 1024 * 1024;

/// Extracts an OCI layer tar stream (optionally gzip compressed) into the destination rootfs.
pub fn extract_layer<R: Read>(reader: R, root_dir: &Path, is_gzipped: bool) -> Result<()> {
    fs::create_dir_all(root_dir)
        .with_context(|| format!("Failed to create rootfs dir: {}", root_dir.display()))?;

    // Apply the extraction limit to the UNCOMPRESSED stream to protect against zip bombs
    if is_gzipped {
        let gz = GzDecoder::new(reader);
        let limited_uncompressed = gz.take(MAX_EXTRACTION_BYTES);
        extract_tar_archive(limited_uncompressed, root_dir)
    } else {
        let limited_uncompressed = reader.take(MAX_EXTRACTION_BYTES);
        extract_tar_archive(limited_uncompressed, root_dir)
    }
}

/// Extracts an uncompressed tar archive into root_dir applying whiteout and security validations.
fn extract_tar_archive<R: Read>(reader: R, root_dir: &Path) -> Result<()> {
    let mut archive = Archive::new(reader);
    let mut total_extracted: u64 = 0;

    for entry_result in archive.entries()? {
        let mut entry = entry_result?;
        let entry_path = entry.path()?.to_path_buf();

        // Check if this is a whiteout marker
        if let Some(file_name) = entry_path.file_name().and_then(|s| s.to_str()) {
            if is_whiteout(file_name) {
                apply_whiteout(root_dir, &entry_path)?;
                continue; // Do not unpack the .wh marker file itself
            }
        }

        let target_path = sanitize_tar_path(root_dir, &entry_path)?;
        ensure_no_symlink_parents(root_dir, &target_path)?;

        let entry_type = entry.header().entry_type();
        match entry_type {
            EntryType::Directory => {
                fs::create_dir_all(&target_path)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = entry.header().mode().unwrap_or(0o755);
                    let _ = fs::set_permissions(&target_path, fs::Permissions::from_mode(mode));
                }
            }
            EntryType::Regular | EntryType::Continuous => {
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                // Avoid overwriting through an existing symlink leaf
                if target_path.is_symlink() {
                    let _ = fs::remove_file(&target_path);
                }
                let mut out_file = File::create(&target_path)
                    .with_context(|| format!("Failed to create file {}", target_path.display()))?;
                let copied = std::io::copy(&mut entry, &mut out_file)?;
                total_extracted += copied;
                if total_extracted > MAX_EXTRACTION_BYTES {
                    anyhow::bail!(
                        "Layer uncompressed extraction size exceeded maximum limit of {MAX_EXTRACTION_BYTES} bytes"
                    );
                }

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = entry.header().mode().unwrap_or(0o644);
                    let _ = fs::set_permissions(&target_path, fs::Permissions::from_mode(mode));
                }
            }
            EntryType::Symlink => {
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                if target_path.exists() || target_path.is_symlink() {
                    let _ = fs::remove_file(&target_path);
                }
                if let Some(link_target) = entry.link_name()? {
                    validate_symlink_target(root_dir, &target_path, &link_target)?;
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(link_target, &target_path)?;
                }
            }
            EntryType::Link => {
                // Hard link with fallback to copy
                if let Some(link_target) = entry.link_name()? {
                    let src_path = sanitize_tar_path(root_dir, &link_target)?;
                    if let Some(parent) = target_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    if target_path.exists() || target_path.is_symlink() {
                        let _ = fs::remove_file(&target_path);
                    }
                    if fs::hard_link(&src_path, &target_path).is_err() {
                        let _ = fs::copy(&src_path, &target_path);
                    }
                }
            }
            _ => {
                // Skip character/block devices, fifos for security in user-space
            }
        }
    }

    // Invariant: Guarantee /tmp and /var/tmp exist with world-writable sticky permissions (0o1777)
    // to prevent host umask stripping from causing EACCES when container processes drop privileges.
    ensure_tmp_sticky_bit(root_dir)?;

    Ok(())
}

/// Invariant: Guarantee /tmp and /var/tmp exist with world-writable sticky permissions (0o1777)
/// to prevent host umask stripping from causing EACCES when container processes drop privileges.
pub fn ensure_tmp_sticky_bit(root_dir: &Path) -> Result<()> {
    for tmp_rel in &["tmp", "var/tmp"] {
        let p = root_dir.join(tmp_rel);
        let _ = fs::create_dir_all(&p);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o1777));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_layer_preserves_tmp_sticky_bit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("rootfs");

        // Build a minimal uncompressed tar archive with a dummy file
        let mut tar_builder = tar::Builder::new(Vec::new());
        let data = b"hello";
        let mut header = tar::Header::new_gnu();
        header.set_path("etc/issue").unwrap();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar_builder.append(&header, &data[..]).unwrap();
        let tar_bytes = tar_builder.into_inner().unwrap();

        extract_layer(&tar_bytes[..], &root, false).unwrap();

        assert!(root.join("etc/issue").exists());

        // Verify /tmp and /var/tmp exist and have sticky permissions
        let tmp_path = root.join("tmp");
        let var_tmp_path = root.join("var/tmp");
        assert!(tmp_path.is_dir());
        assert!(var_tmp_path.is_dir());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let tmp_mode = fs::metadata(&tmp_path).unwrap().permissions().mode() & 0o7777;
            let var_tmp_mode = fs::metadata(&var_tmp_path).unwrap().permissions().mode() & 0o7777;
            assert_eq!(tmp_mode, 0o1777);
            assert_eq!(var_tmp_mode, 0o1777);
        }
    }
}
