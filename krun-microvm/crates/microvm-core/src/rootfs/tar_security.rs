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

/// Validates that a symlink target (whether relative or absolute) does not escape `root_dir`.
/// Relative symlinks are resolved from `symlink_path`'s parent directory.
/// Absolute symlinks are interpreted as relative to `root_dir` (container root), and validated to ensure
/// they do not escape `root_dir` via parent components ('..').
pub fn validate_symlink_target(root_dir: &Path, symlink_path: &Path, target: &Path) -> Result<()> {
    if target.is_relative() {
        let parent = symlink_path.parent().unwrap_or(root_dir);
        let mut current = parent.to_path_buf();
        for comp in target.components() {
            match comp {
                Component::Normal(c) => current.push(c),
                Component::ParentDir => {
                    if current == root_dir || !current.starts_with(root_dir) {
                        bail!(
                            "Security violation: symlink target {:?} from {:?} escapes container root {:?}",
                            target,
                            symlink_path,
                            root_dir
                        );
                    }
                    current.pop();
                }
                _ => {}
            }
        }
    } else {
        let mut current = root_dir.to_path_buf();
        for comp in target.components() {
            match comp {
                Component::Normal(c) => current.push(c),
                Component::ParentDir => {
                    if current == root_dir || !current.starts_with(root_dir) {
                        bail!(
                            "Security violation: absolute symlink target {:?} from {:?} escapes container root {:?}",
                            target,
                            symlink_path,
                            root_dir
                        );
                    }
                    current.pop();
                }
                Component::RootDir | Component::Prefix(_) | Component::CurDir => {}
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

    #[test]
    fn test_validate_symlink_target_safe() {
        let root = Path::new("/tmp/rootfs");
        let link = Path::new("/tmp/rootfs/bin/sh");
        let target = Path::new("bash");
        assert!(validate_symlink_target(root, link, target).is_ok());

        let link_nested = Path::new("/tmp/rootfs/usr/bin/python3");
        let target_nested = Path::new("../lib/python3.11/bin/python3");
        assert!(validate_symlink_target(root, link_nested, target_nested).is_ok());
    }

    #[test]
    fn test_validate_symlink_target_escaping() {
        let root = Path::new("/tmp/rootfs");
        let link = Path::new("/tmp/rootfs/bin/sh");
        let target = Path::new("../../../../etc/shadow");
        assert!(validate_symlink_target(root, link, target).is_err());

        let link2 = Path::new("/tmp/rootfs/opt/tool");
        let target2 = Path::new("../../outside");
        assert!(validate_symlink_target(root, link2, target2).is_err());
    }

    #[test]
    fn test_validate_symlink_target_absolute() {
        let root = Path::new("/tmp/rootfs");
        let link = Path::new("/tmp/rootfs/bin/sh");

        // Safe absolute targets inside container root
        let target_safe = Path::new("/bin/busybox");
        assert!(validate_symlink_target(root, link, target_safe).is_ok());

        let target_nested = Path::new("/usr/lib/libc.so");
        assert!(validate_symlink_target(root, link, target_nested).is_ok());

        // Dangerous absolute targets attempting to traverse out with '..'
        let target_escape = Path::new("/../../etc/shadow");
        assert!(validate_symlink_target(root, link, target_escape).is_err());

        let target_escape2 = Path::new("/opt/../../../host_secret");
        assert!(validate_symlink_target(root, link, target_escape2).is_err());
    }

    #[test]
    fn test_ensure_no_symlink_parents_with_real_fs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();

        let real_dir = root.join("real");
        std::fs::create_dir_all(&real_dir).unwrap();

        let symlink_dir = root.join("sym_dir");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_dir, &symlink_dir).unwrap();

        // Target path passing through symlink should be rejected
        let dangerous_target = symlink_dir.join("evil_file");
        #[cfg(unix)]
        {
            let res = ensure_no_symlink_parents(root, &dangerous_target);
            assert!(res.is_err());
        }

        // Target path with no symlinks should be accepted
        let safe_target = real_dir.join("safe_file");
        assert!(ensure_no_symlink_parents(root, &safe_target).is_ok());
    }
}
