use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

#[cfg(target_os = "macos")]
mod darwin {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int};
    use std::path::Path;

    const CLONE_NOFOLLOW: u32 = 0x0001;

    extern "C" {
        fn clonefile(src: *const c_char, dst: *const c_char, flags: u32) -> c_int;
    }

    pub fn clone_entry(src: &Path, dst: &Path) -> std::io::Result<()> {
        let src_c = CString::new(src.to_string_lossy().as_bytes())?;
        let dst_c = CString::new(dst.to_string_lossy().as_bytes())?;

        let rc = unsafe { clonefile(src_c.as_ptr(), dst_c.as_ptr(), CLONE_NOFOLLOW) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::File;
    use std::os::unix::io::AsRawFd;
    use std::path::Path;

    // FICLONE ioctl constant: 0x40049409 (or _IOW(0x94, 9, int))
    const FICLONE: libc::c_ulong = 0x40049409;

    pub fn clone_file(src: &Path, dst: &Path) -> std::io::Result<()> {
        let src_file = File::open(src)?;
        let dst_file = File::create(dst)?;

        let rc = unsafe { libc::ioctl(dst_file.as_raw_fd(), FICLONE, src_file.as_raw_fd()) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

/// Recursively clones a directory using Copy-on-Write (APFS clonefile on macOS,
/// FICLONE on Linux) falling back to standard copy if CoW is unsupported.
pub fn clone_rootfs<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();

    if dst.exists() {
        fs::remove_dir_all(dst)
            .with_context(|| format!("Failed to clean existing destination: {}", dst.display()))?;
    }

    #[cfg(target_os = "macos")]
    {
        // On APFS, clonefile can clone entire directory trees atomically in one call!
        if darwin::clone_entry(src, dst).is_ok() {
            return Ok(());
        }
    }

    // Fallback recursive clone/copy
    copy_dir_recursive(src, dst)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)
        .with_context(|| format!("Failed to create directory: {}", dst.display()))?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(&src_path)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &dst_path)?;
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::FileTypeExt;
                if file_type.is_fifo() || file_type.is_socket() || file_type.is_char_device() || file_type.is_block_device() {
                    continue;
                }
            }

            // Try CoW clone for file
            #[cfg(target_os = "macos")]
            let cloned = darwin::clone_entry(&src_path, &dst_path).is_ok();
            #[cfg(target_os = "linux")]
            let cloned = linux::clone_file(&src_path, &dst_path).is_ok();
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            let cloned = false;

            if !cloned {
                fs::copy(&src_path, &dst_path)?;
            }
        }
    }

    Ok(())
}
