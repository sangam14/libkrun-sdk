use anyhow::{bail, Result};
use std::path::{Component, Path, PathBuf};

/// Sanitizes a path from a tar entry to prevent Zip-Slip and directory traversal attacks.
/// Returns the canonical destination path guaranteed to be strictly inside `root_dir`.
pub fn sanitize_tar_path(root_dir: &Path, entry_name: &Path) -> Result<PathBuf> {
    let mut clean_rel = PathBuf::new();

    for comp in entry_name.components() {
        match comp {
            Component::Normal(part) => {
                clean_rel.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                bail!(
                    "Tar entry contains illegal '..' traversal component: {:?}",
                    entry_name
                );
            }
            Component::RootDir | Component::Prefix(_) => {
                // Ignore leading root '/' inside tar, treat as relative to root_dir
            }
        }
    }

    if clean_rel.as_os_str().is_empty() {
        return Ok(root_dir.to_path_buf());
    }

    let target = root_dir.join(&clean_rel);

    // Verify the resolved path starts with root_dir
    if !target.starts_with(root_dir) {
        bail!(
            "Path traversal detected: {:?} escapes target root {:?}",
            entry_name,
            root_dir
        );
    }

    Ok(target)
}

/// Verifies that no parent directory component between `root_dir` and `target` is a symlink.
/// This prevents writing through a pre-existing symlink planted by an earlier layer.
pub fn ensure_no_symlink_parents(root_dir: &Path, target: &Path) -> Result<()> {
    let mut current = root_dir.to_path_buf();
    let rel = match target.strip_prefix(root_dir) {
        Ok(r) => r,
        Err(_) => bail!("Target is not inside root_dir"),
    };

    let parent = match rel.parent() {
        Some(p) => p,
        None => return Ok(()),
    };

    for comp in parent.components() {
        if let Component::Normal(part) = comp {
            current.push(part);
            if current.is_symlink() {
                bail!(
                    "Security violation: intermediate path {:?} is a symlink, potential container escape",
                    current
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_tar_path_normal() {
        let root = Path::new("/tmp/rootfs");
        let safe = sanitize_tar_path(root, Path::new("etc/passwd")).unwrap();
        assert_eq!(safe, Path::new("/tmp/rootfs/etc/passwd"));
    }

    #[test]
    fn test_sanitize_tar_path_leading_slash() {
        let root = Path::new("/tmp/rootfs");
        let safe = sanitize_tar_path(root, Path::new("/etc/hosts")).unwrap();
        assert_eq!(safe, Path::new("/tmp/rootfs/etc/hosts"));
    }

    #[test]
    fn test_sanitize_tar_path_traversal_rejected() {
        let root = Path::new("/tmp/rootfs");
        let result = sanitize_tar_path(root, Path::new("../../../etc/shadow"));
        assert!(result.is_err());
    }
}
