use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub const WHITEOUT_PREFIX: &str = ".wh.";
pub const OPAQUE_WHITEOUT: &str = ".wh..wh..opq";

pub fn is_whiteout(file_name: &str) -> bool {
    file_name.starts_with(WHITEOUT_PREFIX)
}

pub fn is_opaque_whiteout(file_name: &str) -> bool {
    file_name == OPAQUE_WHITEOUT
}

/// Applies a whiteout marker:
/// If `.wh..wh..opq`: removes all contents inside the parent directory.
/// If `.wh.<filename>`: removes `<filename>` inside the parent directory.
pub fn apply_whiteout(root_dir: &Path, rel_path: &Path) -> Result<()> {
    // Sanitize path to prevent Zip-Slip / traversal in whiteout marker paths
    let safe_rel = crate::rootfs::tar_security::sanitize_tar_path(root_dir, rel_path)?;

    let file_name = match safe_rel.file_name().and_then(|s| s.to_str()) {
        Some(s) => s.to_string(),
        None => return Ok(()),
    };

    let parent_dir = match safe_rel.parent() {
        Some(p) => p,
        None => return Ok(()),
    };

    if !parent_dir.starts_with(root_dir) || !parent_dir.exists() {
        return Ok(());
    }

    // Verify parent is not a symlink to prevent writing through escaping paths
    if parent_dir.is_symlink() {
        anyhow::bail!(
            "Security violation: parent directory of whiteout marker is a symlink: {:?}",
            parent_dir
        );
    }

    if is_opaque_whiteout(&file_name) {
        // Remove all entries in parent_dir
        if parent_dir.is_dir() {
            for entry in fs::read_dir(parent_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    fs::remove_dir_all(&path)?;
                } else {
                    fs::remove_file(&path)?;
                }
            }
        }
    } else if let Some(target_name) = file_name.strip_prefix(WHITEOUT_PREFIX) {
        let target_path = parent_dir.join(target_name);
        if target_path.exists() || target_path.is_symlink() {
            if target_path.is_dir() {
                fs::remove_dir_all(&target_path).with_context(|| {
                    format!("Failed to remove whiteout dir {}", target_path.display())
                })?;
            } else {
                fs::remove_file(&target_path).with_context(|| {
                    format!("Failed to remove whiteout file {}", target_path.display())
                })?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_whiteout_detection() {
        assert!(is_whiteout(".wh.somefile"));
        assert!(is_whiteout(".wh..wh..opq"));
        assert!(!is_whiteout("normal_file.txt"));
    }

    #[test]
    fn test_apply_regular_whiteout() {
        let dir = tempdir().unwrap();
        let file_to_delete = dir.path().join("deleted_me.txt");
        fs::write(&file_to_delete, "hello").unwrap();
        assert!(file_to_delete.exists());

        apply_whiteout(dir.path(), Path::new(".wh.deleted_me.txt")).unwrap();
        assert!(!file_to_delete.exists());
    }

    #[test]
    fn test_apply_opaque_whiteout() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();
        let f1 = sub.join("f1.txt");
        let f2 = sub.join("f2.txt");
        fs::write(&f1, "1").unwrap();
        fs::write(&f2, "2").unwrap();

        apply_whiteout(dir.path(), Path::new("subdir/.wh..wh..opq")).unwrap();
        assert!(sub.exists());
        assert!(!f1.exists());
        assert!(!f2.exists());
    }

    #[test]
    fn test_apply_whiteout_traversal_rejected() {
        let dir = tempdir().unwrap();
        let res = apply_whiteout(dir.path(), Path::new("../../../.wh.target"));
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("illegal '..'"));
    }
}
