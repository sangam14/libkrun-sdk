use anyhow::{bail, Context, Result};
use krun_sys::KrunContext;
use microvm_core::types::RunnerConfig;
use std::process::ExitCode;

fn validate_config(cfg: &RunnerConfig) -> Result<()> {
    if !cfg.root_path.exists() {
        bail!("Root path does not exist: {}", cfg.root_path.display());
    }
    if !cfg.root_path.is_dir() {
        bail!("Root path is not a directory: {}", cfg.root_path.display());
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
    Ok(())
}

fn run_vm(cfg: RunnerConfig) -> Result<()> {
    validate_config(&cfg)?;

    if let Some(level) = cfg.log_level {
        let _ = krun_sys::set_log_level(level);
    }

    let mut ctx = KrunContext::create().context("Failed to create libkrun context")?;

    ctx.set_vm_config(cfg.num_vcpus, cfg.ram_mib)
        .context("Failed to set VM resources")?;

    ctx.set_root(&cfg.root_path)
        .context("Failed to set VM rootfs")?;

    // Configure optional resource limits (rlimits)
    if let Some(ref rlimits) = cfg.rlimits {
        ctx.set_rlimits(rlimits)
            .context("Failed to configure guest resource limits (rlimits)")?;
    }

    // Configure networking
    if cfg.no_network {
        // Air-gapped isolation: instruct libkrun to expose zero ports and disable network
        let empty_map: Vec<String> = Vec::new();
        let _ = ctx.set_port_map(&empty_map);
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

    // Launch the VM.
    // WARNING: This never returns on success. The calling process becomes the VM supervisor.
    ctx.start_enter().context("libkrun VM startup failed")?;

    bail!("krun_start_enter returned unexpectedly");
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
