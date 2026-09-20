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

pub struct RawModeGuard {
    orig_termios: Option<nix::sys::termios::Termios>,
}

impl RawModeGuard {
    pub fn new() -> Result<Self> {
        use nix::sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg};
        use std::os::fd::AsRawFd;

        let stdin = std::io::stdin();
        let is_tty = unsafe { libc::isatty(stdin.as_raw_fd()) == 1 };

        if !is_tty {
            return Ok(Self { orig_termios: None });
        }

        let orig = tcgetattr(&stdin)?;
        let mut raw = orig.clone();
        cfmakeraw(&mut raw);
        tcsetattr(&stdin, SetArg::TCSANOW, &raw)?;

        Ok(Self {
            orig_termios: Some(orig),
        })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if let Some(ref orig) = self.orig_termios {
            use nix::sys::termios::{tcsetattr, SetArg};
            let stdin = std::io::stdin();
            let _ = tcsetattr(&stdin, SetArg::TCSANOW, orig);
        }
    }
}

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
            crate::net::NetworkMode::Tsi => {
                self.no_network = false;
                self.net_sock_path = None;
            }
        }
        self.network_mode = mode;
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

        // 2. Resolve rootfs (from bundle, local OCI layout, or OCI client pull)
        let (cached_rootfs, oci_config) = if let Some(ref custom_rootfs) = self.rootfs_override {
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

        // 3. Create unique instance directory & CoW clone rootfs
        let instance_id = format!(
            "vm-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis()
        );
        let instance_dir = data_dir.join("instances").join(&instance_id);
        let instance_rootfs = instance_dir.join("rootfs");

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
        let final_cmd = if self.cmd_override.is_some() {
            oci_config.resolve_cmd(self.cmd_override)
        } else if !self.env_vars.is_empty() && oci_config.cmd.is_empty() {
            // If created from bundle, cmd is already stored or defaults
            oci_config.resolve_cmd(self.cmd_override)
        } else {
            oci_config.resolve_cmd(self.cmd_override)
        };
        let mut final_env = oci_config.env;
        final_env.extend(self.env_vars);
        let final_workdir = self.workdir.or(oci_config.working_dir);

        let krun_cfg = KrunConfig::new(final_cmd, final_env, final_workdir);
        krun_cfg.write_to(&instance_rootfs)?;

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

        // 6. Spawn runner subprocess
        let runner_cfg = RunnerConfig {
            root_path: instance_rootfs,
            num_vcpus: self.vcpus,
            ram_mib: self.ram_mib,
            port_forwards: self.port_forwards.clone(),
            net_sock_path: self.net_sock_path,
            virtiofs_mounts: final_virtiofs_mounts,
            vsock_ports: self.vsock_ports,
            console_log_path: final_console_log.clone(),
            log_level: self.log_level,
            interactive: self.interactive,
            tty: self.tty,
            no_network: self.no_network,
            rlimits: self.rlimits,
            detach: self.detach,
            dax_window_size_bytes: self.dax_window_size,
            image_acceleration: self.image_acceleration.clone(),
            gpu: self.gpu,
            gpu_shm_size_bytes: self.gpu_shm_size,
            gpu_flags: self.gpu_flags,
        };

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
        } else if self.interactive || self.tty {
            cmd.stdin(std::process::Stdio::inherit());
            cmd.stdout(std::process::Stdio::inherit());
            cmd.stderr(std::process::Stdio::inherit());
        }

        let raw_guard = if self.tty && !self.detach {
            Some(RawModeGuard::new()?)
        } else {
            None
        };

        let child = cmd.spawn().with_context(|| {
            format!("Failed to spawn runner binary {}", runner_binary.display())
        })?;

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
        };
        let _ = crate::state::StateManager::save(&data_dir, &vm_state);

        Ok(MicroVm {
            id: instance_id,
            child,
            instance_dir,
            data_dir,
            is_detached: self.detach,
            _raw_guard: raw_guard,
        })
    }
}

pub struct MicroVm {
    id: String,
    child: Child,
    instance_dir: PathBuf,
    data_dir: PathBuf,
    is_detached: bool,
    _raw_guard: Option<RawModeGuard>,
}

impl MicroVm {
    pub fn id(&self) -> &str {
        &self.id
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

    /// Gracefully stops the microVM (SIGTERM with fallback to SIGKILL).
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(pid) = self.pid() {
            let nix_pid = Pid::from_raw(pid as i32);
            let _ = signal::kill(nix_pid, Signal::SIGTERM);

            let timeout = Duration::from_secs(5);
            let start = std::time::Instant::now();

            while start.elapsed() < timeout {
                if !self.is_alive() {
                    self.cleanup();
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
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
}
