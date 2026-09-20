use super::tar_security::{ensure_no_symlink_parents, sanitize_tar_path};
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

    let limited_reader = reader.take(MAX_EXTRACTION_BYTES);

    if is_gzipped {
        let gz = GzDecoder::new(limited_reader);
        extract_tar_archive(gz, root_dir)
    } else {
        extract_tar_archive(limited_reader, root_dir)
    }
}

/// Extracts an uncompressed tar archive into root_dir applying whiteout and security validations.
fn extract_tar_archive<R: Read>(reader: R, root_dir: &Path) -> Result<()> {
    let mut archive = Archive::new(reader);

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
                std::io::copy(&mut entry, &mut out_file)?;

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

    Ok(())
}
