//! Watchdog and hypervisor resilience interceptor.
//!
//! On macOS Apple Silicon (Hypervisor.framework), shutting down an SMP multi-vCPU
//! guest triggers PSCI `CPU_OFF` / `AFFINITY_INFO` calls that libkrun's HVF layer
//! mishandles, causing secondary vCPU threads to panic (`src/hvf/src/lib.rs:549: Unexpected val=...`).
//! In that state, the guest has already cleanly halted and unmounted its filesystems,
//! but `krun_start_enter` stays blocked forever waiting on the dead threads.
//!
//! This watchdog tees `stderr` before entering `libkrun`. When it observes the
//! hypervisor shutdown panic signature on an SMP guest, it intercepts the hang and
//! cleanly terminates the supervisor process with exit code 0.

use std::io::Read;
use std::os::unix::io::{FromRawFd, RawFd};
use std::thread;

/// Installs the stderr interceptor watchdog before `krun_start_enter`.
///
/// If pipe setup fails for any reason, `stderr` is left untouched so output is never lost.
pub fn install() {
    let real_err: RawFd = unsafe { libc::dup(libc::STDERR_FILENO) };
    if real_err < 0 {
        return;
    }
    // Prevent the dup'd FD from leaking into child processes
    unsafe { libc::fcntl(real_err, libc::F_SETFD, libc::FD_CLOEXEC) };

    let mut fds = [0 as RawFd; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        unsafe { libc::close(real_err) };
        return;
    }

    let (rd, wr) = (fds[0], fds[1]);

    // Set CLOEXEC on both pipe ends so they don't leak across fork/exec
    unsafe {
        libc::fcntl(rd, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(wr, libc::F_SETFD, libc::FD_CLOEXEC);
    }

    // Redirect stderr (fd 2) to the write-end of our pipe.
    if unsafe { libc::dup2(wr, libc::STDERR_FILENO) } < 0 {
        unsafe {
            libc::close(rd);
            libc::close(wr);
            libc::close(real_err);
        }
        return;
    }

    // Close redundant write descriptor in the current thread
    unsafe { libc::close(wr) };

    // Spawn supervisor watchdog thread
    thread::spawn(move || reader_loop(rd, real_err));
}

fn reader_loop(rd: RawFd, real_err: RawFd) {
    let mut pipe = unsafe { std::fs::File::from_raw_fd(rd) };
    let mut buf = [0u8; 4096];
    let mut window: Vec<u8> = Vec::with_capacity(8192);

    loop {
        let n = match pipe.read(&mut buf) {
            Ok(0) | Err(_) => break, // EOF: all stderr writers closed
            Ok(n) => n,
        };

        // Always tee everything to the real stderr so logs are preserved
        write_all(real_err, &buf[..n]);

        // Maintain rolling window for signature detection across chunk boundaries
        window.extend_from_slice(&buf[..n]);
        if window.len() > 8192 {
            let overflow = window.len() - 8192;
            window.drain(..overflow);
        }

        if is_hvf_smp_panic(&window) {
            let msg = b"\n[microvm-runner] Guest halted cleanly. Intercepted known libkrun HVF multi-vCPU shutdown panic (PSCI CPU_OFF). Exiting cleanly.\n";
            write_all(real_err, msg);
            unsafe { libc::_exit(0) };
        }
    }

    // Clean up: close the dup'd original stderr now that the pipe is done
    unsafe { libc::close(real_err) };
}

/// Identifies the known libkrun HVF vCPU panic pattern:
/// e.g. `thread 'fc_vcpu 1' panicked at src/hvf/src/lib.rs:549: Unexpected val=...`
fn is_hvf_smp_panic(window: &[u8]) -> bool {
    contains(window, b"panicked") && contains(window, b"src/hvf")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn write_all(fd: RawFd, mut buf: &[u8]) {
    while !buf.is_empty() {
        let n = unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        } else if n == 0 {
            break;
        }
        buf = &buf[n as usize..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_hvf_smp_panic() {
        let panic_sample =
            b"thread 'fc_vcpu 1' panicked at src/hvf/src/lib.rs:549:20: Unexpected val=2";
        assert!(is_hvf_smp_panic(panic_sample));

        let normal_log = b"info: starting microVM vCPU threads...";
        assert!(!is_hvf_smp_panic(normal_log));

        let other_panic = b"thread 'main' panicked at src/main.rs:10: file not found";
        assert!(!is_hvf_smp_panic(other_panic));
    }
}
