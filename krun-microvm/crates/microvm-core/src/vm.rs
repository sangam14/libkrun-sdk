use crate::config::KrunConfig;
use crate::oci::{ImageReference, OciClient};
use crate::preflight::Preflight;
use crate::rootfs::clone_rootfs;
use crate::types::{PortForward, RunnerConfig, VirtioFsMount, VsockPort};
use anyhow::{bail, Context, Result};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;
use tokio::process::{Child, Command};

use crate::tty::RawModeGuard;

static INSTANCE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct ArtifactMount {
    pub reference: String,
    pub tag: String,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub struct CowWorkspace {
    pub host_path: PathBuf,
    pub tag: String,
}

pub struct MicroVmBuilder {
    image: String,
    vcpus: u8,
    ram_mib: u32,
    cmd_override: Option<Vec<String>>,
    env_vars: Vec<String>,
    workdir: Option<String>,
    port_forwards: Vec<PortForward>,
    virtiofs_mounts: Vec<VirtioFsMount>,
    artifacts: Vec<ArtifactMount>,
    cow_workspaces: Vec<CowWorkspace>,
    vsock_ports: Vec<VsockPort>,
    data_dir: Option<PathBuf>,
    runner_path: Option<PathBuf>,
    console_log_path: Option<PathBuf>,
    perform_preflight: bool,
    log_level: Option<u32>,
    interactive: bool,
    tty: bool,
    detach: bool,
    no_network: bool,
    rlimits: Option<String>,
    rootfs_override: Option<PathBuf>,
    bundle_dir: Option<PathBuf>,
    net_sock_path: Option<String>,
    dns_servers: Vec<String>,
    hostname: Option<String>,
    network_mode: crate::net::NetworkMode,
    file_mounts: Vec<(PathBuf, PathBuf)>,
    dax_window_size: Option<u64>,
    image_acceleration: Option<crate::acceleration::ImageAcceleration>,
    gpu: bool,
    gpu_shm_size: Option<u64>,
    gpu_flags: Option<u32>,
    sandbox: bool,
    allow_hosts: Vec<String>,
    secrets: Vec<(String, String)>,
    max_tokens: Option<u64>,
    boot_payload: Option<crate::types::BootPayload>,
    disks: Vec<crate::types::DiskAttachment>,
}

impl MicroVmBuilder {
    pub fn new(image: impl Into<String>) -> Self {
        Self {
            image: image.into(),
            vcpus: 2,
            ram_mib: 512,
            cmd_override: None,
            env_vars: Vec::new(),
            workdir: None,
            port_forwards: Vec::new(),
            virtiofs_mounts: Vec::new(),
            artifacts: Vec::new(),
            cow_workspaces: Vec::new(),
            vsock_ports: Vec::new(),
            data_dir: None,
            runner_path: None,
            console_log_path: None,
            perform_preflight: true,
            log_level: None,
            interactive: false,
            tty: false,
            detach: false,
            no_network: false,
            rlimits: None,
            rootfs_override: None,
            bundle_dir: None,
            net_sock_path: None,
            dns_servers: Vec::new(),
            hostname: None,
            network_mode: crate::net::NetworkMode::Tsi,
            file_mounts: Vec::new(),
            dax_window_size: None,
            image_acceleration: None,
            gpu: false,
            gpu_shm_size: None,
            gpu_flags: None,
            sandbox: true,
            allow_hosts: Vec::new(),
            secrets: Vec::new(),
            max_tokens: None,
            boot_payload: None,
            disks: Vec::new(),
        }
    }

    /// Creates a MicroVmBuilder from an unpacked OCI runtime bundle directory (containing config.json and rootfs/).
    pub fn from_bundle(bundle_dir: impl AsRef<Path>) -> Result<Self> {
        let bundle = crate::bundle::OciBundle::load(bundle_dir)?;
        let mut builder = Self::new(format!("bundle:{}", bundle.bundle_dir.display()))
            .virtiofs_mounts(bundle.virtiofs_mounts);

        if let Some(cpus) = bundle.vcpus {
            builder = builder.cpus(cpus);
        }
        if let Some(ram) = bundle.ram_mib {
            builder = builder.memory_mb(ram);
        }
        builder.file_mounts = bundle.file_mounts;

        if !bundle.cmd.is_empty() {
            builder = builder.cmd(bundle.cmd);
        }
        if !bundle.env.is_empty() {
            builder.env_vars = bundle.env;
        }
        if let Some(w) = bundle.workdir {
            builder = builder.workdir(w);
        }
        if let Some(lim) = bundle.rlimits {
            builder = builder.rlimits(lim);
        }
        let network_annotation = bundle.spec.annotations().as_ref().and_then(|a| {
            a.get("krun.network")
                .or_else(|| a.get("krun.io/network-mode"))
                .or_else(|| a.get("io.katacontainers.config.hypervisor.network_model"))
        });

        if let Some(net_model) = network_annotation {
            if net_model == "gvproxy" {
                builder = builder.network_mode(crate::net::NetworkMode::Gvproxy);
            } else if net_model == "tsi" {
                builder = builder.network_mode(crate::net::NetworkMode::Tsi);
            } else if net_model == "none" {
                builder = builder.network_mode(crate::net::NetworkMode::None);
            }
        } else if let Some(netns) = bundle.netns_path {
            builder = builder.network_mode(crate::net::NetworkMode::Cni {
                netns,
                socket_path: None,
            });
        } else {
            // Default to gvproxy for rootless / desktop execution when CNI netns is omitted
            builder = builder.network_mode(crate::net::NetworkMode::Gvproxy);
        }

        if let Some(allow_str) = bundle
            .spec
            .annotations()
            .as_ref()
            .and_then(|a| a.get("krun.network.allow").or_else(|| a.get("krun.io/allow-egress")))
        {
            for target in allow_str.split(',') {
                let trimmed = target.trim();
                if !trimmed.is_empty() {
                    builder = builder.allow_host(trimmed);
                }
            }
        }
        for pf in bundle.port_forwards {
            builder = builder.port_forward(pf.host, pf.guest);
        }
        if let Some(ref ann) = bundle.spec.annotations() {
            if let Some(gpu_str) = ann.get("krun.gpu").or_else(|| ann.get("krun.io/gpu")) {
                if gpu_str == "true" || gpu_str == "1" {
                    builder = builder.gpu(true);
                }
            }
            if let Some(dax_str) = ann.get("krun.dax").or_else(|| ann.get("krun.io/dax-window-size")) {
                if dax_str == "true" || dax_str == "1" {
                    builder = builder.dax_window_size(2 * 1024 * 1024 * 1024);
                } else if !dax_str.is_empty() && dax_str != "false" {
                    if let Ok(bytes) = crate::config::parse_size_to_bytes(dax_str) {
                        builder = builder.dax_window_size(bytes);
                    }
                }
            }
            if let Some(sb_str) = ann.get("krun.sandbox").or_else(|| ann.get("krun.io/sandbox")) {
                if sb_str == "false" || sb_str == "0" {
                    builder = builder.sandbox(false);
                }
            }
        }
        builder.rootfs_override = Some(bundle.rootfs_path);
        builder.bundle_dir = Some(bundle.bundle_dir);
        Ok(builder)
    }

    pub fn file_mount(mut self, src: impl Into<PathBuf>, dest: impl Into<PathBuf>) -> Self {
        self.file_mounts.push((src.into(), dest.into()));
        self
    }

    pub fn file_mounts(mut self, mounts: Vec<(PathBuf, PathBuf)>) -> Self {
        self.file_mounts.extend(mounts);
        self
    }

    pub fn get_vcpus(&self) -> u8 {
        self.vcpus
    }

    pub fn get_ram_mib(&self) -> u32 {
        self.ram_mib
    }

    pub fn get_file_mounts(&self) -> &[(PathBuf, PathBuf)] {
        &self.file_mounts
    }

    pub fn get_gpu(&self) -> bool {
        self.gpu
    }

    pub fn get_gpu_shm_size(&self) -> Option<u64> {
        self.gpu_shm_size
    }

    pub fn net_sock_path(mut self, path: impl Into<String>) -> Self {
        self.net_sock_path = Some(path.into());
        self
    }

    pub fn virtiofs_mounts(mut self, mounts: Vec<VirtioFsMount>) -> Self {
        self.virtiofs_mounts.extend(mounts);
        self
    }

    /// Attaches an arbitrary OCI artifact (e.g. model weights, dataset, toolchain) from an OCI registry,
    /// caching it locally and mounting it via VirtioFS with the specified tag.
    pub fn attach_artifact(
        mut self,
        reference: impl Into<String>,
        tag: impl Into<String>,
        read_only: bool,
    ) -> Self {
        self.artifacts.push(ArtifactMount {
            reference: reference.into(),
            tag: tag.into(),
            read_only,
        });
        self
    }

    /// Mounts a host directory as an instant Copy-on-Write (CoW) sandbox via VirtioFS.
    /// Any modifications made inside the microVM are isolated and will not affect the host directory.
    pub fn workspace_cow(mut self, host_path: impl AsRef<Path>, tag: impl Into<String>) -> Self {
        self.cow_workspaces.push(CowWorkspace {
            host_path: host_path.as_ref().to_path_buf(),
            tag: tag.into(),
        });
        self
    }

    pub fn cpus(mut self, vcpus: u8) -> Self {
        self.vcpus = vcpus;
        self
    }

    pub fn memory_mb(mut self, ram_mib: u32) -> Self {
        self.ram_mib = ram_mib;
        self
    }

    pub fn cmd(mut self, cmd: Vec<String>) -> Self {
        self.cmd_override = Some(cmd);
        self
    }

    pub fn env(mut self, key: &str, val: &str) -> Self {
        self.env_vars.push(format!("{key}={val}"));
        self
    }

    pub fn workdir(mut self, dir: impl Into<String>) -> Self {
        self.workdir = Some(dir.into());
        self
    }

    pub fn port_forward(mut self, host: u16, guest: u16) -> Self {
        self.port_forwards.push(PortForward::new(host, guest));
        self
    }

    pub fn virtiofs(mut self, tag: &str, path: impl Into<PathBuf>, read_only: bool) -> Self {
        self.virtiofs_mounts
            .push(VirtioFsMount::new(tag, path, read_only));
        self
    }

    /// Sets the VirtioFS DAX (Direct Access) shared memory window size in bytes.
    pub fn dax_window_size(mut self, bytes: u64) -> Self {
        self.dax_window_size = Some(bytes);
        self
    }

    /// Sets the VirtioFS DAX window size from a human-readable string (e.g. "4G", "512M", "1024K").
    pub fn dax_window_size_str(mut self, s: &str) -> Result<Self> {
        self.dax_window_size = Some(crate::config::parse_size_to_bytes(s)?);
        Ok(self)
    }

    /// Configures Dragonfly Nydus RAFSv6 / EROFS image acceleration and lazy loading.
    pub fn image_acceleration(
        mut self,
        acceleration: crate::acceleration::ImageAcceleration,
    ) -> Self {
        self.image_acceleration = Some(acceleration);
        self
    }

    /// Toggles on-demand lazy loading (RAFSv6 chunk streaming).
    pub fn lazy_load(mut self, enabled: bool) -> Self {
        let mut acc = self.image_acceleration.take().unwrap_or_default();
        acc.lazy_load = enabled;
        self.image_acceleration = Some(acc);
        self
    }

    /// Configures explicit path to a local RAFS v6 or EROFS metadata bootstrap image.
    pub fn nydus_bootstrap(mut self, path: impl Into<PathBuf>) -> Self {
        let mut acc = self.image_acceleration.take().unwrap_or_default();
        acc.bootstrap_path = Some(path.into());
        self.image_acceleration = Some(acc);
        self
    }

    /// Sets the directory where downloaded chunk blobs are cached and deduplicated.
    pub fn chunk_cache_dir(mut self, path: impl Into<PathBuf>) -> Self {
        let mut acc = self.image_acceleration.take().unwrap_or_default();
        acc.chunk_cache_dir = Some(path.into());
        self.image_acceleration = Some(acc);
        self
    }

    /// Sets the chunk or macro-chunk size for streaming (e.g. "4M", "64M").
    pub fn chunk_size_str(mut self, s: &str) -> Result<Self> {
        let bytes = crate::config::parse_size_to_bytes(s)?;
        let mut acc = self.image_acceleration.take().unwrap_or_default();
        acc.chunk_size_bytes = Some(bytes);
        self.image_acceleration = Some(acc);
        Ok(self)
    }

    pub fn get_image_acceleration(&self) -> Option<&crate::acceleration::ImageAcceleration> {
        self.image_acceleration.as_ref()
    }

    pub fn vsock_port(mut self, port: u32, path: impl Into<PathBuf>) -> Self {
        self.vsock_ports.push(VsockPort::new(port, path));
        self
    }

    pub fn data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.data_dir = Some(path.into());
        self
    }

    pub fn runner_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.runner_path = Some(path.into());
        self
    }

    pub fn console_log(mut self, path: impl Into<PathBuf>) -> Self {
        self.console_log_path = Some(path.into());
        self
    }

    pub fn preflight(mut self, enable: bool) -> Self {
        self.perform_preflight = enable;
        self
    }

    pub fn log_level(mut self, level: u32) -> Self {
        self.log_level = Some(level);
        self
    }

    pub fn interactive(mut self, enabled: bool) -> Self {
        self.interactive = enabled;
        self
    }

    pub fn tty(mut self, enabled: bool) -> Self {
        self.tty = enabled;
        self
    }

    pub fn detach(mut self, enabled: bool) -> Self {
        self.detach = enabled;
        self
    }

    pub fn no_network(mut self, enabled: bool) -> Self {
        self.no_network = enabled;
        self
    }

    pub fn rlimits(mut self, rlimits: impl Into<String>) -> Self {
        self.rlimits = Some(rlimits.into());
        self
    }

    /// Enable or disable hardware-accelerated virtio-gpu (Metal on Apple Silicon, DRM on Linux).
    pub fn gpu(mut self, enabled: bool) -> Self {
        self.gpu = enabled;
        self
    }

    /// Configure shared memory vRAM host window size in bytes for virtio-gpu device.
    pub fn gpu_shm_size(mut self, bytes: u64) -> Self {
        self.gpu_shm_size = Some(bytes);
        self
    }

    /// Custom virglrenderer flags (defaults to Venus/Metal acceleration flags).
    pub fn gpu_flags(mut self, flags: u32) -> Self {
        self.gpu_flags = Some(flags);
        self
    }

    /// Enables or disables zero-trust host sandboxing (Landlock LSM and Seccomp syscall filtering on Linux).
    pub fn sandbox(mut self, enabled: bool) -> Self {
        self.sandbox = enabled;
        self
    }

    /// Restricts outbound network egress to explicitly permitted `host:port` destinations (default-deny egress).
    pub fn allow_host(mut self, host: impl Into<String>) -> Self {
        self.allow_hosts.push(host.into());
        self
    }

    /// Adds multiple allowed outbound destinations.
    pub fn allow_hosts(mut self, hosts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        for h in hosts {
            self.allow_hosts.push(h.into());
        }
        self
    }

    /// Injects a secret using zero-trust in-flight substitution:
    /// The guest sees `KEY=krun-secret:KEY`, while the host proxy substitutes the real secret on outbound requests.
    pub fn secret(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.secrets.push((key.into(), value.into()));
        self
    }

    /// Sets a hard ceiling on cumulative LLM tokens (prompt + completion) consumed by the microVM.
    pub fn max_tokens(mut self, limit: u64) -> Self {
        self.max_tokens = Some(limit);
        self
    }

    /// Appends a DNS nameserver (or comma/semicolon-separated nameservers, e.g. "8.8.8.8,1.1.1.1").
    pub fn dns(mut self, dns: impl Into<String>) -> Self {
        self.dns_servers.push(dns.into());
        self
    }

    /// Sets multiple DNS nameservers.
    pub fn dns_servers(mut self, servers: Vec<String>) -> Self {
        self.dns_servers.extend(servers);
        self
    }

    /// Sets the guest microVM hostname.
    pub fn hostname(mut self, name: impl Into<String>) -> Self {
        self.hostname = Some(name.into());
        self
    }

    /// Sets the network mode (TSI, UnixStream, or None) for high-performance guest isolation.
    pub fn network_mode(mut self, mode: crate::net::NetworkMode) -> Self {
        match &mode {
            crate::net::NetworkMode::None => {
                self.no_network = true;
            }
            crate::net::NetworkMode::UnixStream(path) => {
                self.no_network = false;
                self.net_sock_path = Some(path.to_string_lossy().to_string());
            }
            crate::net::NetworkMode::Cni { netns, socket_path } => {
                self.no_network = false;
                if let Some(sock) = socket_path {
                    self.net_sock_path = Some(sock.to_string_lossy().to_string());
                } else {
                    let default_sock = netns.parent().unwrap_or(netns).join("cni-krun.sock");
                    if default_sock.exists() {
                        self.net_sock_path = Some(default_sock.to_string_lossy().to_string());
                    }
                }
            }
            crate::net::NetworkMode::Tsi | crate::net::NetworkMode::Gvproxy => {
                self.no_network = false;
                self.net_sock_path = None;
            }
        }
        self.network_mode = mode;
        self
    }

    /// Sets explicit multi-boot payload configuration.
    pub fn boot_payload(mut self, payload: crate::types::BootPayload) -> Self {
        self.boot_payload = Some(payload);
        self
    }

    /// Configures direct kernel boot (e.g. Linux direct bzImage/ELF, NetBSD, FreeBSD Firecracker kernel).
    pub fn kernel(
        mut self,
        kernel_path: impl Into<PathBuf>,
        initramfs: Option<PathBuf>,
        cmdline: Option<String>,
    ) -> Self {
        self.boot_payload = Some(crate::types::BootPayload::Kernel(
            crate::types::KernelPayload {
                kernel_path: kernel_path.into(),
                kernel_format: krun_sys::kernel_formats::KRUN_KERNEL_FORMAT_ELF,
                initramfs,
                cmdline,
            },
        ));
        self
    }

    /// Configures UEFI firmware boot (e.g. booting full disk images via EDK2 / KRUN_EFI.fd).
    pub fn firmware(mut self, firmware_path: impl Into<PathBuf>) -> Self {
        self.boot_payload = Some(crate::types::BootPayload::Firmware(
            crate::types::FirmwarePayload {
                firmware_path: firmware_path.into(),
            },
        ));
        self
    }

    /// Configures unikernel boot (e.g. Unikraft, Nanos, OSv, Solo5/Mirage).
    pub fn unikernel(mut self, kernel_path: impl Into<PathBuf>, cmdline: Option<String>) -> Self {
        self.boot_payload = Some(crate::types::BootPayload::Unikernel(
            crate::types::KernelPayload {
                kernel_path: kernel_path.into(),
                kernel_format: krun_sys::kernel_formats::KRUN_KERNEL_FORMAT_ELF,
                initramfs: None,
                cmdline,
            },
        ));
        self
    }

    /// Attaches an additional block disk device to the microVM.
    pub fn disk(
        mut self,
        id: impl Into<String>,
        path: impl Into<PathBuf>,
        read_only: bool,
    ) -> Self {
        self.disks
            .push(crate::types::DiskAttachment::new(id, path, read_only));
        self
    }

    /// Pulls the image (if not cached), creates a CoW rootfs snapshot, prepares .krun_config.json,
    /// and spawns the microVM runner subprocess.
    pub async fn run(self) -> Result<MicroVm> {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let data_dir = self
            .data_dir
            .unwrap_or_else(|| PathBuf::from(home).join(".cache/krun-microvm"));

        // 1. Run Preflight checks if enabled
        if self.perform_preflight {
            let host_ports: Vec<u16> = self.port_forwards.iter().map(|p| p.host).collect();
            let reports = Preflight::run_all(&host_ports, &data_dir);
            for r in &reports {
                if !r.passed {
                    bail!("Preflight check '{}' failed: {}", r.name, r.message);
                }
            }
        }

        let is_direct_boot = matches!(
            self.boot_payload,
            Some(crate::types::BootPayload::Kernel(_))
                | Some(crate::types::BootPayload::Firmware(_))
                | Some(crate::types::BootPayload::Unikernel(_))
        );

        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
        let millis = now.as_millis();
        let nanos = now.subsec_nanos();
        let counter = INSTANCE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let instance_id = format!("vm-{millis}-{nanos:06}-{counter:03}");
        let instance_dir = data_dir.join("instances").join(&instance_id);
        let instance_rootfs = instance_dir.join("rootfs");

        if is_direct_boot {
            let _ = fs::create_dir_all(&instance_rootfs);
        } else {
            // 2. Resolve rootfs (from bundle, local OCI layout, or OCI client pull)
            let (cached_rootfs, oci_config) = if let Some(ref custom_rootfs) = self.rootfs_override
            {
                let empty_cfg = crate::config::OciConfig {
                    entrypoint: Vec::new(),
                    cmd: Vec::new(),
                    env: Vec::new(),
                    working_dir: None,
                    user: None,
                };
                (custom_rootfs.clone(), empty_cfg)
            } else {
                let reference = ImageReference::parse(&self.image)?;
                if reference.is_local_layout {
                    let layout_path = reference.layout_path.as_ref().unwrap();
                    tracing::info!(
                        "Loading image from local OCI layout at {} (tag: {})...",
                        layout_path.display(),
                        reference.tag
                    );
                    crate::oci::OciLayout::load(layout_path, Some(&reference.tag), &data_dir)?
                } else {
                    let oci_client = OciClient::new();
                    tracing::info!(
                        "Ensuring image '{}' is ready...",
                        reference.canonical_name()
                    );
                    oci_client.pull_and_unpack(&reference, &data_dir).await?
                }
            };

            tracing::info!("Cloning rootfs to instance {} (CoW)...", instance_id);
            clone_rootfs(&cached_rootfs, &instance_rootfs)
                .context("Failed to perform CoW clone of rootfs")?;

            // 3b. Inject single-file mounts (e.g. Kubernetes ConfigMaps, Secrets, projected service account tokens)
            for (src, dest) in &self.file_mounts {
                let relative_dest = dest.strip_prefix("/").unwrap_or(dest);
                let target_path = instance_rootfs.join(relative_dest);
                if let Some(parent) = target_path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if let Err(e) = fs::copy(src, &target_path) {
                    tracing::warn!(
                        "Failed to copy file mount from {} to {}: {}",
                        src.display(),
                        target_path.display(),
                        e
                    );
                } else {
                    tracing::debug!(
                        "Injected file mount {} -> {}",
                        src.display(),
                        target_path.display()
                    );
                }
            }

            // 4. Resolve Cmd, Env, and write /.krun_config.json
            let mut final_cmd = if self.cmd_override.is_some() {
                oci_config.resolve_cmd(self.cmd_override)
            } else if !self.env_vars.is_empty() && oci_config.cmd.is_empty() {
                // If created from bundle, cmd is already stored or defaults
                oci_config.resolve_cmd(self.cmd_override)
            } else {
                oci_config.resolve_cmd(self.cmd_override)
            };

            // If the command binary is not an absolute path, resolve it against standard guest rootfs PATH
            if let Some(first) = final_cmd.first_mut() {
                if !first.starts_with('/') {
                    let search_paths = [
                        "/usr/local/sbin",
                        "/usr/local/bin",
                        "/usr/sbin",
                        "/usr/bin",
                        "/sbin",
                        "/bin",
                    ];
                    for sp in search_paths {
                        let rel = sp.trim_start_matches('/');
                        let candidate = instance_rootfs.join(rel).join(&first);
                        if candidate.exists() {
                            *first = format!("{sp}/{first}");
                            break;
                        }
                    }
                }
            }

            if self.network_mode.is_gvproxy() && !final_cmd.is_empty() {
                let init_script = r#"#!/bin/sh
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:$PATH
ip link set eth0 up 2>/dev/null || true
(udhcpc -i eth0 -q -n -t 2 || (ip addr add 192.168.127.2/24 dev eth0 && ip route add default via 192.168.127.1)) 2>/dev/null || true
exec "$@"
"#;
                let script_path = instance_rootfs.join("krun-init.sh");
                let _ = std::fs::write(&script_path, init_script);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &script_path,
                        std::fs::Permissions::from_mode(0o755),
                    );
                }
                final_cmd.insert(0, "/krun-init.sh".to_string());
            }

            let mut final_env = oci_config.env;
            final_env.extend(self.env_vars);
            if self.network_mode.is_gvproxy() {
                final_env.push("KRUN_DHCP=1".to_string());
            }
            for (k, _) in &self.secrets {
                final_env.push(format!("{k}=krun-secret:{k}"));
            }
            let final_workdir = self.workdir.or(oci_config.working_dir);

            let krun_cfg = KrunConfig::new(final_cmd, final_env, final_workdir);
            krun_cfg.write_to(&instance_rootfs)?;
        }

        // 5. Locate runner binary (auto-codesigns if on macOS)
        let runner_binary = match self.runner_path {
            Some(p) => {
                ensure_runner_signed(&p)?;
                p
            }
            None => find_runner_binary()?,
        };

        let final_console_log = if self.detach {
            Some(
                self.console_log_path
                    .unwrap_or_else(|| instance_dir.join("console.log")),
            )
        } else {
            self.console_log_path
        };

        // 5b. Attach any requested OCI artifacts (pull and add to VirtioFS)
        let mut final_virtiofs_mounts = self.virtiofs_mounts;
        if !self.artifacts.is_empty() {
            let oci_client = OciClient::new();
            for art in &self.artifacts {
                let art_ref = ImageReference::parse(&art.reference)?;
                tracing::info!(
                    "Ensuring OCI artifact '{}' is ready...",
                    art_ref.canonical_name()
                );
                let art_path = oci_client.pull_artifact(&art_ref, &data_dir).await?;
                final_virtiofs_mounts.push(VirtioFsMount::new(
                    art.tag.clone(),
                    art_path,
                    art.read_only,
                ));
            }
        }

        // 5c. Setup any requested CoW workspaces (APFS clonefile / FICLONE snapshot)
        for ws in &self.cow_workspaces {
            let cow_dir = instance_dir.join("workspaces").join(&ws.tag);
            tracing::info!(
                "Creating CoW workspace snapshot for '{}' at {}...",
                ws.tag,
                cow_dir.display()
            );
            clone_rootfs(&ws.host_path, &cow_dir).with_context(|| {
                format!(
                    "Failed to create CoW workspace snapshot for '{}'",
                    ws.host_path.display()
                )
            })?;
            final_virtiofs_mounts.push(VirtioFsMount::new(
                ws.tag.clone(),
                cow_dir,
                false, // Read-write isolated workspace!
            ));
        }

        // 5c-2. Attach accelerated Nydus chunk blob cache directory if lazy loading is enabled
        if let Some(ref acc) = self.image_acceleration {
            if acc.lazy_load {
                let cache_dir = acc
                    .chunk_cache_dir
                    .clone()
                    .unwrap_or_else(|| data_dir.join("nydus-cache"));
                let _ = fs::create_dir_all(&cache_dir);
                tracing::info!(
                    "Configuring accelerated chunk cache at {} (format: {:?})...",
                    cache_dir.display(),
                    acc.format
                );
                let mut chunk_mount = VirtioFsMount::new("nydus-blobs", &cache_dir, true);
                if let Some(dax_bytes) = self.dax_window_size {
                    chunk_mount = chunk_mount.with_dax(dax_bytes);
                }
                final_virtiofs_mounts.push(chunk_mount);
            }
        }

        // Ensure mount directory exists in guest rootfs for all virtiofs tags
        for m in &final_virtiofs_mounts {
            let _ = fs::create_dir_all(instance_rootfs.join(&m.tag));
        }

        // 5d. Setup network configuration files (resolv.conf, hosts, hostname) with autonomous resilient DNS
        let guest_hostname = self.hostname.unwrap_or_else(|| instance_id.clone());
        if !self.no_network {
            let nameservers = crate::net::DnsConfig::resolve_nameservers(&self.dns_servers);
            tracing::info!(
                "Configuring guest network files (hostname: {}, DNS: {:?})...",
                guest_hostname,
                nameservers
            );
            crate::net::DnsConfig::write_network_files(
                &instance_rootfs,
                &guest_hostname,
                &nameservers,
            )?;
        } else {
            // In air-gapped mode, write loopback only
            crate::net::DnsConfig::write_network_files(&instance_rootfs, &guest_hostname, &[])?;
        }

        // 5e. Start host egress proxy server if filtering, secrets, or token budget are specified
        let (egress_proxy, proxy_port) = if !self.allow_hosts.is_empty()
            || !self.secrets.is_empty()
            || self.max_tokens.is_some()
        {
            let policy = crate::net::EgressPolicy::new(self.allow_hosts.clone());
            let secrets = crate::net::SecretSubstitution::new(&self.secrets);
            let budget = crate::net::LlmTokenBudget::new(self.max_tokens);
            let proxy_server =
                crate::net::EgressProxyServer::start(policy, secrets, budget).await?;
            let port = proxy_server.port();
            let url = proxy_server.proxy_url();
            crate::net::DnsConfig::inject_proxy_environment(&instance_rootfs, &url)?;
            tracing::info!("Egress proxy active on port {port} for microVM {instance_id}");
            (Some(proxy_server), Some(port))
        } else {
            (None, None)
        };

        // 5c. Setup user-mode gvproxy path if requested
        let final_net_sock_path = if self.network_mode.is_gvproxy() {
            let gvproxy_sock = instance_dir.join("gvproxy.sock");
            Some(gvproxy_sock.to_string_lossy().to_string())
        } else if let Some(p) = self.network_mode.unix_socket_path() {
            Some(p.to_string_lossy().to_string())
        } else {
            self.net_sock_path.clone()
        };

        // 6. Spawn runner subprocess
        let runner_cfg = RunnerConfig {
            root_path: instance_rootfs,
            num_vcpus: self.vcpus,
            ram_mib: self.ram_mib,
            boot_payload: self.boot_payload.clone(),
            disks: self.disks.clone(),
            port_forwards: self.port_forwards.clone(),
            net_sock_path: final_net_sock_path,
            virtiofs_mounts: final_virtiofs_mounts,
            vsock_ports: self.vsock_ports,
            console_log_path: final_console_log.clone(),
            log_level: self.log_level,
            interactive: self.interactive,
            tty: self.tty,
            no_network: self.no_network,
            gvproxy: self.network_mode.is_gvproxy(),
            netns: self.network_mode.netns_path().map(|p| p.to_path_buf()),
            rlimits: self.rlimits,
            detach: self.detach,
            dax_window_size_bytes: self.dax_window_size,
            image_acceleration: self.image_acceleration.clone(),
            gpu: self.gpu,
            gpu_shm_size_bytes: self.gpu_shm_size,
            gpu_flags: self.gpu_flags,
            sandbox: self.sandbox,
            allow_hosts: self.allow_hosts.clone(),
            secrets: self.secrets.clone(),
            max_tokens: self.max_tokens,
            proxy_port,
            supervisor_sock_path: Some(instance_dir.join("supervisor.sock")),
        };

        let supervisor_sock = instance_dir.join("supervisor.sock");
        let runner_cfg_path = instance_dir.join("runner_config.json");
        let cfg_json = serde_json::to_string_pretty(&runner_cfg)?;
        std::fs::write(&runner_cfg_path, &cfg_json)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&runner_cfg_path, std::fs::Permissions::from_mode(0o600));
        }

        tracing::info!("Spawning microvm runner (PID isolated)...");

        let mut cmd = Command::new(&runner_binary);
        cmd.arg("--config").arg(&runner_cfg_path);
        if !self.detach {
            cmd.kill_on_drop(true);
        }

        if self.detach {
            cmd.stdin(std::process::Stdio::null());
            if let Some(ref log_path) = final_console_log {
                if let Some(parent) = log_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let file = std::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(log_path)?;
                let err_file = file.try_clone()?;
                cmd.stdout(file);
                cmd.stderr(err_file);
            } else {
                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::null());
            }

            #[cfg(unix)]
            unsafe {
                cmd.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        } else {
            if self.interactive || self.tty {
                cmd.stdin(std::process::Stdio::inherit());
            } else {
                cmd.stdin(std::process::Stdio::null());
            }
            cmd.stdout(std::process::Stdio::inherit());
            cmd.stderr(std::process::Stdio::inherit());
        }

        let raw_guard = if self.tty && !self.detach {
            Some(RawModeGuard::new()?)
        } else {
            None
        };

        let mut child = cmd.spawn().with_context(|| {
            format!("Failed to spawn runner binary {}", runner_binary.display())
        })?;

        // Wait briefly for supervisor socket readiness (with fallback if runner runs non-socket mode)
        let connect_timeout = Duration::from_secs(3);
        let start_time = std::time::Instant::now();
        let mut supervisor_ready = false;
        while start_time.elapsed() < connect_timeout {
            if supervisor_sock.exists() {
                if let Ok(mut stream) = tokio::net::UnixStream::connect(&supervisor_sock).await {
                    if let Ok(Ok(msg)) = tokio::time::timeout(
                        Duration::from_millis(500),
                        crate::protocol::read_frame_async(&mut stream),
                    )
                    .await
                    {
                        if matches!(msg.payload, crate::protocol::MessagePayload::Ready { .. }) {
                            supervisor_ready = true;
                            break;
                        }
                    }
                }
            }
            if let Ok(Some(status)) = child.try_wait() {
                bail!("microvm-runner exited prematurely with status: {status}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if !supervisor_ready {
            tracing::debug!(
                "Supervisor socket did not signal Ready within timeout; proceeding in fallback mode"
            );
        }

        let pid = child.id().unwrap_or(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let vm_state = crate::state::VmState {
            id: instance_id.clone(),
            pid,
            image: self.image.clone(),
            created_at: now,
            port_forwards: self.port_forwards.clone(),
            instance_dir: instance_dir.clone(),
            status: crate::state::VmStatus::Running,
            vcpus: Some(self.vcpus),
            memory_mib: Some(self.ram_mib),
        };
        let _ = crate::state::StateManager::save(&data_dir, &vm_state);

        Ok(MicroVm {
            id: instance_id,
            child,
            instance_dir,
            data_dir,
            is_detached: self.detach,
            supervisor_sock,
            _raw_guard: raw_guard,
            _proxy: egress_proxy,
            _gvproxy: None,
        })
    }
}

pub struct MicroVm {
    id: String,
    child: Child,
    instance_dir: PathBuf,
    data_dir: PathBuf,
    is_detached: bool,
    supervisor_sock: PathBuf,
    _raw_guard: Option<RawModeGuard>,
    _proxy: Option<crate::net::EgressProxyServer>,
    _gvproxy: Option<crate::net::GvproxyInstance>,
}

impl MicroVm {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn proxy_port(&self) -> Option<u16> {
        self._proxy.as_ref().map(|p| p.port())
    }

    pub fn gvproxy(&self) -> Option<&crate::net::GvproxyInstance> {
        self._gvproxy.as_ref()
    }

    pub fn egress_blocked_count(&self) -> u64 {
        self._proxy.as_ref().map(|p| p.blocked_count()).unwrap_or(0)
    }

    pub fn llm_tokens_consumed(&self) -> u64 {
        self._proxy
            .as_ref()
            .map(|p| p.tokens_consumed())
            .unwrap_or(0)
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    pub fn is_detached(&self) -> bool {
        self.is_detached
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Asynchronously waits for the microVM to exit and cleans up ephemeral state.
    pub async fn wait(&mut self) -> Result<ExitStatus> {
        let status = self.child.wait().await?;
        self.cleanup();
        Ok(status)
    }

    /// Asynchronously waits for the microVM to exit within the specified timeout.
    ///
    /// If the timeout expires before the VM halts, initiates a graceful stop (`stop()`)
    /// and returns a timeout error.
    pub async fn wait_timeout(&mut self, timeout: Duration) -> Result<ExitStatus> {
        match tokio::time::timeout(timeout, self.child.wait()).await {
            Ok(status_res) => {
                let status = status_res?;
                self.cleanup();
                Ok(status)
            }
            Err(_) => {
                tracing::warn!(
                    "MicroVM '{}' exceeded timeout of {:?}; terminating...",
                    self.id,
                    timeout
                );
                let _ = self.stop().await;
                bail!(
                    "MicroVM '{}' execution timed out after {:?}",
                    self.id,
                    timeout
                );
            }
        }
    }

    pub fn supervisor_sock_path(&self) -> &Path {
        &self.supervisor_sock
    }

    /// Sends a heartbeat ping to the supervisor and measures round-trip latency.
    pub async fn ping(&self) -> Result<Duration> {
        if !self.supervisor_sock.exists() {
            bail!("Supervisor socket does not exist");
        }
        let mut stream = tokio::net::UnixStream::connect(&self.supervisor_sock).await?;
        let ping_msg =
            crate::protocol::ControlMessage::new(1, crate::protocol::MessagePayload::Ping);
        let start = std::time::Instant::now();
        crate::protocol::write_frame_async(&mut stream, &ping_msg).await?;
        let resp = tokio::time::timeout(
            Duration::from_secs(3),
            crate::protocol::read_frame_async(&mut stream),
        )
        .await
        .context("Ping timed out")??;
        let elapsed = start.elapsed();
        if matches!(resp.payload, crate::protocol::MessagePayload::Pong { .. }) {
            Ok(elapsed)
        } else {
            bail!("Unexpected response to Ping: {:?}", resp.payload);
        }
    }

    /// Queries the supervisor process for live resource usage statistics.
    pub async fn stats(&self) -> Result<crate::metrics::ProcessStats> {
        if self.supervisor_sock.exists() {
            if let Ok(mut stream) = tokio::net::UnixStream::connect(&self.supervisor_sock).await {
                let stats_msg =
                    crate::protocol::ControlMessage::new(1, crate::protocol::MessagePayload::Stats);
                if crate::protocol::write_frame_async(&mut stream, &stats_msg)
                    .await
                    .is_ok()
                {
                    if let Ok(Ok(resp)) = tokio::time::timeout(
                        Duration::from_secs(3),
                        crate::protocol::read_frame_async(&mut stream),
                    )
                    .await
                    {
                        if let crate::protocol::MessagePayload::StatsResponse(s) = resp.payload {
                            return Ok(s);
                        }
                    }
                }
            }
        }
        // Fallback to host process inspection
        let pid = self.pid().unwrap_or(0);
        crate::metrics::collect_process_stats(pid)
            .context("Failed to collect host process telemetry")
    }

    /// Suspends vCPUs and pauses the microVM.
    pub async fn pause(&mut self) -> Result<()> {
        if self.supervisor_sock.exists() {
            if let Ok(mut stream) = tokio::net::UnixStream::connect(&self.supervisor_sock).await {
                let pause_msg =
                    crate::protocol::ControlMessage::new(1, crate::protocol::MessagePayload::Pause);
                let _ = crate::protocol::write_frame_async(&mut stream, &pause_msg).await;
            }
        } else if let Some(pid) = self.pid() {
            let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGSTOP);
        }
        let _ = crate::state::StateManager::pause(&self.data_dir, &self.id);
        Ok(())
    }

    /// Resumes suspended vCPUs and unpauses the microVM.
    pub async fn resume(&mut self) -> Result<()> {
        if self.supervisor_sock.exists() {
            if let Ok(mut stream) = tokio::net::UnixStream::connect(&self.supervisor_sock).await {
                let resume_msg = crate::protocol::ControlMessage::new(
                    1,
                    crate::protocol::MessagePayload::Resume,
                );
                let _ = crate::protocol::write_frame_async(&mut stream, &resume_msg).await;
            }
        } else if let Some(pid) = self.pid() {
            let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGCONT);
        }
        let _ = crate::state::StateManager::resume(&self.data_dir, &self.id);
        Ok(())
    }

    /// Gracefully stops the microVM (via supervisor socket with fallback to SIGTERM and SIGKILL).
    pub async fn stop(&mut self) -> Result<()> {
        // 1. Try graceful stop over supervisor socket
        if self.supervisor_sock.exists() {
            if let Ok(mut stream) = tokio::net::UnixStream::connect(&self.supervisor_sock).await {
                let stop_msg = crate::protocol::ControlMessage::new(
                    1,
                    crate::protocol::MessagePayload::Stop {
                        timeout_secs: 5,
                        force: false,
                    },
                );
                let _ = crate::protocol::write_frame_async(&mut stream, &stop_msg).await;
            }
        }

        // 2. Poll for termination or fallback to OS signals
        if let Some(pid) = self.pid() {
            let nix_pid = Pid::from_raw(pid as i32);
            let timeout = Duration::from_secs(5);
            let start = std::time::Instant::now();

            while start.elapsed() < timeout {
                if !self.is_alive() {
                    self.cleanup();
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            // Fallback to SIGTERM if still running
            let _ = signal::kill(nix_pid, Signal::SIGTERM);
            let term_start = std::time::Instant::now();
            while term_start.elapsed() < Duration::from_secs(2) {
                if !self.is_alive() {
                    self.cleanup();
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            // Force kill if not stopped
            let _ = signal::kill(nix_pid, Signal::SIGKILL);
            let _ = self.child.wait().await;
        }

        self.cleanup();
        Ok(())
    }

    /// Copies a host file or directory into the guest rootfs.
    pub fn copy_to_guest(&self, src_host: &Path, guest_path: &str) -> Result<()> {
        crate::state::StateManager::copy_into(&self.data_dir, &self.id, src_host, guest_path)
    }

    /// Copies a file or directory from the guest rootfs to the host.
    pub fn copy_from_guest(&self, guest_path: &str, dst_host: &Path) -> Result<()> {
        crate::state::StateManager::copy_from(&self.data_dir, &self.id, guest_path, dst_host)
    }

    pub fn instance_dir(&self) -> &Path {
        &self.instance_dir
    }

    pub fn console_log_path(&self) -> PathBuf {
        self.instance_dir.join("console.log")
    }

    /// Explicitly purges instance state and directory from disk.
    pub fn purge(&self) {
        let _ = crate::state::StateManager::remove(&self.data_dir, &self.id);
        if self.instance_dir.exists() {
            let _ = std::fs::remove_dir_all(&self.instance_dir);
        }
    }

    fn cleanup(&self) {
        if self.is_detached {
            return;
        }
        self.purge();
    }
}

fn find_runner_binary() -> Result<PathBuf> {
    // 1. Next to current executable
    if let Ok(curr) = std::env::current_exe() {
        if let Some(dir) = curr.parent() {
            let candidate = dir.join("microvm-runner");
            if candidate.exists() {
                ensure_runner_signed(&candidate)?;
                return Ok(candidate);
            }
        }
    }

    // 2. Check target directory of current workspace
    let candidates = [
        PathBuf::from("target/debug/microvm-runner"),
        PathBuf::from("target/release/microvm-runner"),
        PathBuf::from("../target/debug/microvm-runner"),
        PathBuf::from("../target/release/microvm-runner"),
    ];

    for c in &candidates {
        if c.exists() {
            let path = c.canonicalize()?;
            ensure_runner_signed(&path)?;
            return Ok(path);
        }
    }

    // 3. System PATH
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("microvm-runner");
            if candidate.exists() {
                ensure_runner_signed(&candidate)?;
                return Ok(candidate);
            }
        }
    }

    bail!("microvm-runner binary not found. Please build microvm-runner or pass runner_path()");
}

#[cfg(target_os = "macos")]
fn ensure_runner_signed(runner_path: &Path) -> Result<()> {
    let check = std::process::Command::new("codesign")
        .args(["-d", "--entitlements", "-"])
        .arg(runner_path)
        .output();

    let needs_signing = match check {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            !stdout.contains("com.apple.security.hypervisor")
                && !stderr.contains("com.apple.security.hypervisor")
        }
        Err(_) => true,
    };

    if needs_signing {
        const PLIST_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.hypervisor</key>
    <true/>
</dict>
</plist>
"#;
        let mut tmp_plist = tempfile::NamedTempFile::new()
            .context("Failed to create temporary plist file for codesign")?;
        std::io::Write::write_all(&mut tmp_plist, PLIST_XML.as_bytes())?;

        let status = std::process::Command::new("codesign")
            .arg("--entitlements")
            .arg(tmp_plist.path())
            .args(["--force", "-s", "-"])
            .arg(runner_path)
            .status()
            .context("Failed to execute codesign command")?;

        if !status.success() {
            tracing::warn!("Failed to codesign runner binary with hypervisor entitlement");
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn ensure_runner_signed(_runner_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_microvm_builder_from_bundle() {
        let dir = tempdir().unwrap();
        let rootfs = dir.path().join("rootfs");
        std::fs::create_dir_all(&rootfs).unwrap();

        let host_file = dir.path().join("app.yaml");
        std::fs::write(&host_file, "env: production\n").unwrap();

        let config_json = format!(
            r#"{{
            "ociVersion": "1.0.2",
            "root": {{
                "path": "rootfs"
            }},
            "process": {{
                "user": {{
                    "uid": 0,
                    "gid": 0
                }},
                "cwd": "/",
                "args": ["/bin/sh", "-c", "echo ready"],
                "env": ["PORT=8080"]
            }},
            "linux": {{
                "resources": {{
                    "memory": {{
                        "limit": 1073741824
                    }},
                    "cpu": {{
                        "quota": 200000,
                        "period": 100000
                    }}
                }}
            }},
            "mounts": [
                {{
                    "source": "{}",
                    "destination": "/etc/config/app.yaml"
                }}
            ]
        }}"#,
            host_file.display()
        );

        std::fs::write(dir.path().join("config.json"), config_json).unwrap();

        let builder = MicroVmBuilder::from_bundle(dir.path()).unwrap();
        assert_eq!(builder.get_vcpus(), 2);
        assert_eq!(builder.get_ram_mib(), 1024);
        assert_eq!(builder.get_file_mounts().len(), 1);
        assert_eq!(
            builder.get_file_mounts()[0],
            (host_file, PathBuf::from("/etc/config/app.yaml"))
        );
    }

    #[test]
    fn test_builder_image_acceleration() {
        let builder = MicroVmBuilder::new("alpine:latest")
            .lazy_load(true)
            .chunk_size_str("64M")
            .unwrap()
            .chunk_cache_dir("/var/cache/nydus");

        let acc = builder
            .get_image_acceleration()
            .expect("Expected image acceleration");
        assert!(acc.lazy_load);
        assert_eq!(acc.chunk_size_bytes, Some(64 * 1024 * 1024));
        assert_eq!(acc.chunk_cache_dir, Some(PathBuf::from("/var/cache/nydus")));
    }

    #[test]
    fn test_builder_lazy_load_with_bootstrap() {
        let bootstrap_path = PathBuf::from("/tmp/nydus-bootstrap.rafs");
        let builder = MicroVmBuilder::new("my-image:latest")
            .nydus_bootstrap(&bootstrap_path)
            .dax_window_size_str("4G")
            .unwrap();

        let acc = builder.get_image_acceleration().unwrap();
        assert!(acc.lazy_load);
        assert_eq!(acc.bootstrap_path, Some(bootstrap_path));
        assert_eq!(builder.dax_window_size, Some(4 * 1024 * 1024 * 1024));
    }

    #[test]
    fn test_builder_gpu_config() {
        let builder = MicroVmBuilder::new("pytorch/pytorch:latest")
            .gpu(true)
            .gpu_shm_size(8 * 1024 * 1024 * 1024)
            .gpu_flags(0x40);

        assert!(builder.get_gpu());
        assert_eq!(builder.get_gpu_shm_size(), Some(8 * 1024 * 1024 * 1024));
        assert_eq!(builder.gpu_flags, Some(0x40));
    }

    #[test]
    fn test_builder_egress_and_secret_config() {
        let builder = MicroVmBuilder::new("alpine:latest")
            .allow_host("api.openai.com:443")
            .allow_host("pypi.org:443")
            .secret("OPENAI_KEY", "sk-proj-test123")
            .max_tokens(500_000);

        assert_eq!(
            builder.allow_hosts,
            vec!["api.openai.com:443", "pypi.org:443"]
        );
        assert_eq!(
            builder.secrets,
            vec![("OPENAI_KEY".to_string(), "sk-proj-test123".to_string())]
        );
        assert_eq!(builder.max_tokens, Some(500_000));
    }

    #[test]
    fn test_builder_multi_boot_config() {
        let kernel_vm = MicroVmBuilder::new("")
            .kernel(
                "/boot/vmlinuz",
                Some(PathBuf::from("/boot/initrd")),
                Some("console=ttyS0".to_string()),
            )
            .disk("disk0", "/dev/vda.raw", false);

        match kernel_vm.boot_payload.as_ref().unwrap() {
            crate::types::BootPayload::Kernel(k) => {
                assert_eq!(k.kernel_path, PathBuf::from("/boot/vmlinuz"));
                assert_eq!(k.initramfs, Some(PathBuf::from("/boot/initrd")));
                assert_eq!(k.cmdline, Some("console=ttyS0".to_string()));
            }
            _ => panic!("Expected BootPayload::Kernel"),
        }
        assert_eq!(kernel_vm.disks.len(), 1);
        assert_eq!(kernel_vm.disks[0].id, "disk0");

        let fw_vm = MicroVmBuilder::new("").firmware("/opt/homebrew/share/libkrun/KRUN_EFI.fd");
        match fw_vm.boot_payload.as_ref().unwrap() {
            crate::types::BootPayload::Firmware(f) => {
                assert_eq!(
                    f.firmware_path,
                    PathBuf::from("/opt/homebrew/share/libkrun/KRUN_EFI.fd")
                );
            }
            _ => panic!("Expected BootPayload::Firmware"),
        }

        let unikernel_vm =
            MicroVmBuilder::new("").unikernel("app.unikraft", Some("ip=192.168.1.5".to_string()));
        match unikernel_vm.boot_payload.as_ref().unwrap() {
            crate::types::BootPayload::Unikernel(u) => {
                assert_eq!(u.kernel_path, PathBuf::from("app.unikraft"));
                assert_eq!(u.cmdline, Some("ip=192.168.1.5".to_string()));
            }
            _ => panic!("Expected BootPayload::Unikernel"),
        }
    }

    #[test]
    fn test_concurrent_instance_id_uniqueness() {
        use std::collections::HashSet;
        use std::sync::{Arc, Mutex};
        use std::thread;

        let ids = Arc::new(Mutex::new(HashSet::new()));
        let mut handles = Vec::new();

        for _ in 0..50 {
            let ids_clone = Arc::clone(&ids);
            handles.push(thread::spawn(move || {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap();
                let millis = now.as_millis();
                let nanos = now.subsec_nanos();
                let counter = INSTANCE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let id = format!("vm-{millis}-{nanos:06}-{counter:03}");
                let mut set = ids_clone.lock().unwrap();
                assert!(!set.contains(&id), "Duplicate instance ID generated: {id}");
                set.insert(id);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(ids.lock().unwrap().len(), 50);
    }
}
