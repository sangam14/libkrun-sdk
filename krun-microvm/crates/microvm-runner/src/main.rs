use anyhow::{bail, Context, Result};
use krun_sys::KrunContext;
use microvm_core::types::RunnerConfig;
use std::process::ExitCode;

mod sandbox;
mod supervisor;
mod watchdog;

/// Returns a file descriptor safe for use with kqueue/epoll as the console input.
///
/// When stdin is not a TTY (daemon mode, redirected input, /dev/null), libkrun's
/// event loop will abort with an assertion failure on non-pollable descriptors.
/// We substitute the read-end of an internal pipe that stays open and pollable.
///
/// Uses `O_CLOEXEC` to prevent FD leaks across fork/exec boundaries, and
/// explicitly closes the write-end since nothing will write to it.
fn safe_console_input_fd() -> Result<i32> {
    if unsafe { libc::isatty(0) } == 1 {
        return Ok(0);
    }

    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        bail!(
            "Failed to create non-pollable stdin fallback pipe: {}",
            std::io::Error::last_os_error()
        );
    }

    // Set CLOEXEC on the read-end to prevent leaking into forked child processes.
    unsafe { libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC) };

    // Close the write-end immediately — we only need the read-end to stay
    // open and pollable. The read-end will EOF if this process exits, which
    // is the correct behavior for a dummy console input.
    unsafe { libc::close(fds[1]) };

    Ok(fds[0])
}

fn validate_config(cfg: &RunnerConfig) -> Result<()> {
    match cfg.boot_payload.as_ref() {
        Some(microvm_core::types::BootPayload::Kernel(k)) => {
            if !k.kernel_path.exists() {
                bail!("Kernel path does not exist: {}", k.kernel_path.display());
            }
        }
        Some(microvm_core::types::BootPayload::Firmware(f)) => {
            if !f.firmware_path.exists() {
                bail!(
                    "Firmware path does not exist: {}",
                    f.firmware_path.display()
                );
            }
        }
        Some(microvm_core::types::BootPayload::Unikernel(u)) => {
            if !u.kernel_path.exists() {
                bail!("Unikernel path does not exist: {}", u.kernel_path.display());
            }
        }
        Some(microvm_core::types::BootPayload::Oci) | None => {
            if !cfg.root_path.exists() {
                bail!("Root path does not exist: {}", cfg.root_path.display());
            }
            if !cfg.root_path.is_dir() {
                bail!("Root path is not a directory: {}", cfg.root_path.display());
            }
        }
    }
    if cfg.num_vcpus == 0 {
        bail!("num_vcpus must be > 0");
    }
    if cfg.ram_mib == 0 {
        bail!("ram_mib must be > 0");
    }
    for m in &cfg.virtiofs_mounts {
        if !m.path.exists() {
            bail!("VirtioFS host path does not exist: {}", m.path.display());
        }
    }
    for d in &cfg.disks {
        if !d.path.exists() {
            bail!("Block disk path does not exist: {}", d.path.display());
        }
    }
    Ok(())
}

fn run_vm(cfg: RunnerConfig) -> Result<()> {
    validate_config(&cfg)?;

    let supervisor = match cfg.supervisor_sock_path.as_ref() {
        Some(p) => Some(supervisor::SupervisorServer::start(p)?),
        None => None,
    };

    if let Some(level) = cfg.log_level {
        let _ = krun_sys::set_log_level(level);
    }

    // On Linux, if a CNI network namespace is specified, join it via setns prior to VM initialization
    #[cfg(target_os = "linux")]
    if let Some(ref netns_path) = cfg.netns {
        if netns_path.exists() {
            match std::fs::File::open(netns_path) {
                Ok(file) => {
                    use std::os::unix::io::AsRawFd;
                    unsafe {
                        if libc::setns(file.as_raw_fd(), libc::CLONE_NEWNET) != 0 {
                            eprintln!(
                                "[microvm-runner] Warning: setns to CNI netns {} failed: {}",
                                netns_path.display(),
                                std::io::Error::last_os_error()
                            );
                        } else {
                            eprintln!(
                                "[microvm-runner] Successfully joined CNI network namespace {}",
                                netns_path.display()
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "[microvm-runner] Warning: Failed to open CNI netns {}: {}",
                        netns_path.display(),
                        e
                    );
                }
            }
        }
    }

    let mut ctx = KrunContext::create().context("Failed to create libkrun context")?;

    ctx.set_vm_config(cfg.num_vcpus, cfg.ram_mib)
        .context("Failed to set VM resources")?;

    // Attach any configured block disk devices
    for disk in &cfg.disks {
        ctx.add_disk(&disk.id, &disk.path, disk.read_only)
            .with_context(|| format!("Failed to add block disk '{}'", disk.id))?;
    }

    // Configure boot payload: OCI Rootfs, Direct Kernel, UEFI Firmware, or Unikernel
    match cfg.boot_payload.as_ref() {
        Some(microvm_core::types::BootPayload::Kernel(k)) => {
            let cmdline = k.cmdline.as_deref().unwrap_or("console=ttyS0");
            ctx.set_kernel(
                &k.kernel_path,
                k.kernel_format,
                k.initramfs.as_deref(),
                Some(cmdline),
            )
            .context("Failed to configure direct kernel payload")?;
        }
        Some(microvm_core::types::BootPayload::Firmware(f)) => {
            ctx.set_firmware(&f.firmware_path)
                .context("Failed to configure UEFI firmware payload")?;
        }
        Some(microvm_core::types::BootPayload::Unikernel(u)) => {
            let cmdline = u.cmdline.as_deref();
            ctx.set_kernel(
                &u.kernel_path,
                krun_sys::kernel_formats::KRUN_KERNEL_FORMAT_ELF,
                None,
                cmdline,
            )
            .context("Failed to configure unikernel payload")?;
        }
        Some(microvm_core::types::BootPayload::Oci) | None => {
            ctx.set_root(&cfg.root_path)
                .context("Failed to set VM rootfs")?;
        }
    }

    // Configure optional resource limits (rlimits)
    if let Some(ref rlimits) = cfg.rlimits {
        ctx.set_rlimits(rlimits)
            .context("Failed to configure guest resource limits (rlimits)")?;
    }

    // Configure networking
    if cfg.no_network {
        // Air-gapped isolation: instruct libkrun to expose zero ports and disable network
        let empty_map: Vec<String> = Vec::new();
        ctx.set_port_map(&empty_map)
            .context("Failed to configure air-gapped port map")?;
    } else if let Some(ref sock_path) = cfg.net_sock_path {
        ctx.add_net_unixstream(Some(sock_path), None)
            .context("Failed to add virtio-net unixstream")?;
    } else if !cfg.port_forwards.is_empty() {
        // Use libkrun's built-in TSI with port mapping
        let mappings: Vec<String> = cfg
            .port_forwards
            .iter()
            .map(|pf| format!("{}:{}", pf.host, pf.guest))
            .collect();
        ctx.set_port_map(&mappings)
            .context("Failed to configure TSI port map")?;
    }

    if let Some(ref acc) = cfg.image_acceleration {
        eprintln!(
            "[microvm-runner] Image Acceleration active: format={:?}, lazy_load={}, chunk_size={} KB",
            acc.format,
            acc.lazy_load,
            acc.chunk_size_bytes.unwrap_or(4 * 1024 * 1024) / 1024
        );
    }

    // Configure optional virtio-gpu (Metal on Apple Silicon, Venus/DRM on Linux)
    if cfg.gpu {
        use krun_sys::virgl_flags::*;
        let flags = cfg.gpu_flags.unwrap_or(
            VIRGLRENDERER_USE_EGL
                | VIRGLRENDERER_THREAD_SYNC
                | VIRGLRENDERER_USE_SURFACELESS
                | VIRGLRENDERER_VENUS
                | VIRGLRENDERER_DRM,
        );
        let shm_mb = cfg.gpu_shm_size_bytes.unwrap_or(0) / (1024 * 1024);
        eprintln!(
            "[microvm-runner] Enabling hardware-accelerated virtio-gpu (vRAM shm: {} MB, flags: 0x{:x})",
            shm_mb, flags
        );
        ctx.set_gpu_options(flags, cfg.gpu_shm_size_bytes)
            .context("Failed to configure virtio-gpu options")?;
    }

    // Configure virtio-fs directory shares
    for mount in &cfg.virtiofs_mounts {
        if let Some(dax) = mount.dax_window_size_bytes.or(cfg.dax_window_size_bytes) {
            eprintln!(
                "[microvm-runner] Enabled VirtioFS DAX window for '{}' ({} MB)",
                mount.tag,
                dax / (1024 * 1024)
            );
        }
        ctx.add_virtiofs(&mount.tag, &mount.path, mount.read_only)
            .with_context(|| format!("Failed to add virtiofs mount '{}'", mount.tag))?;
    }

    // Configure optional console output logging
    if let Some(ref log_path) = cfg.console_log_path {
        ctx.set_console_output(log_path)
            .context("Failed to configure console output")?;
    }

    // Configure vsock ports
    for vp in &cfg.vsock_ports {
        ctx.add_vsock_port(vp.port, &vp.socket_path)
            .with_context(|| format!("Failed to add vsock port {}", vp.port))?;
    }

    // Configure serial console and ensure pollable input descriptor to prevent kqueue/epoll aborts
    let in_fd = safe_console_input_fd()?;
    let _ = ctx.disable_implicit_console();
    ctx.add_serial_console_default(in_fd, 1)
        .context("Failed to configure default serial console")?;

    // Install watchdog for hypervisor resilience (macOS SMP PSCI CPU_OFF panic)
    #[cfg(target_os = "macos")]
    watchdog::install();

    // 7. Enforce zero-trust host sandboxing before entering hypervisor
    sandbox::apply_sandbox(&cfg)?;

    // Launch the VM.
    // The calling process becomes the VM supervisor until the guest halts.
    if let Some(ref s) = supervisor {
        s.emit_booted();
    }

    let res = ctx.start_enter().context("libkrun VM execution failed");
    if let Some(ref s) = supervisor {
        match &res {
            Ok(()) => s.emit_exited(0, "clean guest exit"),
            Err(e) => s.emit_failed(&format!("{e:#}"), "execution"),
        }
    }
    res?;

    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: microvm-runner --config <path-to-runner-config.json>");
        eprintln!("   or: microvm-runner <config-json>");
        eprintln!("\nHelper supervisor binary for krun-microvm.");
        eprintln!("This is spawned by microvm-core and should not be run directly.");
        return ExitCode::from(125);
    }

    let config_content = if (args[1] == "--config" || args[1] == "-c") && args.len() > 2 {
        match std::fs::read_to_string(&args[2]) {
            Ok(content) => content,
            Err(e) => {
                eprintln!(
                    "Error: Failed to read runner config file '{}': {e}",
                    args[2]
                );
                return ExitCode::from(125);
            }
        }
    } else if std::path::Path::new(&args[1]).exists() {
        match std::fs::read_to_string(&args[1]) {
            Ok(content) => content,
            Err(e) => {
                eprintln!(
                    "Error: Failed to read runner config file '{}': {e}",
                    args[1]
                );
                return ExitCode::from(125);
            }
        }
    } else {
        args[1].clone()
    };

    let config: RunnerConfig = match serde_json::from_str(&config_content) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: Failed to parse runner config JSON: {e}");
            return ExitCode::from(125);
        }
    };

    if let Err(e) = run_vm(config) {
        eprintln!("Error: {e:?}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
