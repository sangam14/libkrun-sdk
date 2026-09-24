use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use microvm_core::{
    collect_process_stats, detect_kernel_format, parse_kernel_format, ImageReference,
    MicroVmBuilder, OciArtifact, OciClient, OciLayout, Preflight, StateManager, VmStatus,
};
use microvm_core::compose::{
    ComposeProject, ComposeProjectState, ComposeServiceState,
};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "microvm")]
#[command(about = "Run OCI container images in hardware-isolated microVMs with libkrun", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Args, Debug, Clone)]
pub struct RunArgs {
    /// OCI image reference (e.g. alpine:latest, ubuntu:22.04) or empty if --bundle is used
    #[arg(default_value = "")]
    pub image: String,

    /// Path to an unpacked OCI runtime bundle (containing config.json and rootfs/)
    #[arg(long)]
    pub bundle: Option<PathBuf>,

    /// Path to a declarative YAML manifest (compose or microvm spec)
    #[arg(short = 'f', long = "file")]
    pub file: Option<PathBuf>,

    /// Number of virtual CPUs
    #[arg(short = 'c', long, default_value_t = 2)]
    pub cpus: u8,

    /// RAM in MiB
    #[arg(short = 'm', long, default_value_t = 512)]
    pub memory: u32,

    /// Port forwarding rules in host:guest format (e.g. 8080:80)
    #[arg(short = 'p', long = "port")]
    pub ports: Vec<String>,

    /// Mount host directory via VirtioFS: <host_path>:<tag>[:ro]
    #[arg(short = 'v', long = "volume")]
    pub volumes: Vec<String>,

    /// Attach an OCI artifact (models, datasets, blobs) via VirtioFS: <reference>:<tag>[:ro]
    #[arg(long = "artifact")]
    pub artifacts: Vec<String>,

    /// Mount an isolated Copy-on-Write (CoW) sandbox of a host directory: <host_path>:<tag>
    #[arg(long = "workspace-cow")]
    pub workspace_cow: Vec<String>,

    /// Environment variables (KEY=VALUE)
    #[arg(short = 'e', long = "env")]
    pub env: Vec<String>,

    /// Working directory inside guest
    #[arg(short = 'w', long)]
    pub workdir: Option<String>,

    /// Custom data cache directory
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Skip preflight virtualization checks
    #[arg(long)]
    pub no_preflight: bool,

    /// libkrun log level (0=Off, 1=Error, 2=Warn, 3=Info, 4=Debug, 5=Trace)
    #[arg(long)]
    pub log_level: Option<u32>,

    /// Keep STDIN open even if not attached
    #[arg(short = 'i', long)]
    pub interactive: bool,

    /// Allocate a pseudo-TTY
    #[arg(short = 't', long)]
    pub tty: bool,

    /// Run container in background and print container ID
    #[arg(short = 'd', long)]
    pub detach: bool,

    /// Completely isolate guest without network interfaces
    #[arg(long)]
    pub no_network: bool,

    /// Network mode: tsi (default), gvproxy (user-mode virtual network), none (air-gapped), unix:<socket_path>, or cni:<netns_path>
    #[arg(long, alias = "network", default_value = "tsi")]
    pub net: String,

    /// Custom DNS nameservers (e.g. 8.8.8.8,1.1.1.1; defaults to autonomous resilient fallback)
    #[arg(long = "dns")]
    pub dns: Vec<String>,

    /// Custom guest hostname (defaults to microVM instance ID)
    #[arg(long)]
    pub hostname: Option<String>,

    /// Custom MAC address for virtio-net interface (e.g. 5a:94:ef:e4:0c:ee)
    #[arg(long = "mac")]
    pub mac: Option<String>,

    /// Network MTU for virtio-net interface (default: 1500)
    #[arg(long = "mtu")]
    pub mtu: Option<usize>,

    /// Guest resource limits (e.g. RLIMIT_NOFILE=1024:2048)
    #[arg(long)]
    pub rlimits: Option<String>,

    /// VirtioFS DAX (Direct Access) shared memory window size (e.g. 4G, 512M)
    #[arg(long)]
    pub dax: Option<String>,

    /// Enable Dragonfly Nydus RAFSv6 instant on-demand lazy loading
    #[arg(long = "lazy-load")]
    pub lazy_load: bool,

    /// Path to local RAFSv6 / EROFS metadata bootstrap image
    #[arg(long = "nydus-bootstrap")]
    pub nydus_bootstrap: Option<PathBuf>,

    /// Directory for caching downloaded Nydus chunk blobs
    #[arg(long = "nydus-cache")]
    pub nydus_cache: Option<PathBuf>,

    /// Chunk or macro-chunk size for streaming (e.g. 4M, 64M)
    #[arg(long = "chunk-size")]
    pub chunk_size: Option<String>,

    /// Enable hardware-accelerated virtio-gpu (Apple Silicon Metal / Linux DRM Venus)
    #[arg(long)]
    pub gpu: bool,

    /// Shared memory vRAM window size for virtio-gpu (e.g. 2G, 4G, 8G)
    #[arg(long = "gpu-shm-size")]
    pub gpu_shm_size: Option<String>,

    /// Disable zero-trust host sandboxing restrictions
    #[arg(long = "no-sandbox")]
    pub no_sandbox: bool,

    /// Allow outbound network egress to explicitly permitted destination (e.g. api.openai.com:443, *.github.com:443)
    #[arg(long = "allow-host", alias = "allow-net")]
    pub allow_hosts: Vec<String>,

    /// Inject secret via in-flight substitution (KEY=VALUE, KEY=env:VAR_NAME, or KEY=file:/path)
    #[arg(long = "secret")]
    pub secrets: Vec<String>,

    /// Hard ceiling on cumulative LLM tokens (prompt + completion) consumed by the microVM
    #[arg(long = "max-tokens")]
    pub max_tokens: Option<u64>,

    /// Direct kernel boot: path to kernel binary (ELF, RAW, bzImage, Image.gz, m1n1)
    #[arg(long = "kernel")]
    pub kernel: Option<PathBuf>,

    /// Direct kernel format override (raw/bin/m1n1, elf/vmlinux, gz/image.gz/vmlinuz, bz2, zstd, pe)
    #[arg(long = "kernel-format", alias = "kformat")]
    pub kernel_format: Option<String>,

    /// Direct kernel boot: optional initramfs/initrd path
    #[arg(long = "initrd")]
    pub initrd: Option<PathBuf>,

    /// Direct kernel boot: kernel command line arguments
    #[arg(long = "cmdline")]
    pub cmdline: Option<String>,

    /// UEFI firmware boot: path to firmware blob (e.g. KRUN_EFI.fd)
    #[arg(long = "firmware")]
    pub firmware: Option<PathBuf>,

    /// Attach block disk image: <path> or <id>:<path>[:ro]
    #[arg(long = "disk")]
    pub disks: Vec<String>,

    /// Optional command to override ENTRYPOINT/CMD
    #[arg(last = true)]
    pub cmd: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run an OCI container image, direct kernel, or firmware disk as a microVM
    Run(Box<RunArgs>),

    /// Boot a unikernel image (Unikraft, Nanos, OSv, Solo5) directly in a microVM
    Unikernel {
        /// Path to the unikernel binary (ELF)
        kernel: PathBuf,

        /// Number of virtual CPUs
        #[arg(short = 'c', long, default_value_t = 1)]
        cpus: u8,

        /// RAM in MiB
        #[arg(short = 'm', long, default_value_t = 512)]
        memory: u32,

        /// Unikernel command line arguments
        #[arg(long = "cmdline", alias = "params")]
        cmdline: Option<String>,

        /// Attach block disk image: <path> or <id>:<path>[:ro]
        #[arg(long = "disk")]
        disks: Vec<String>,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Run an AI coding agent or secure developer workspace in an isolated microVM sandbox
    Sandbox {
        /// Agent or profile to launch (e.g. claude, gemini, codex, dev)
        #[arg(default_value = "dev")]
        agent: String,

        /// Host directory to mount as isolated Copy-on-Write (CoW) workspace
        #[arg(short = 'w', long = "workspace", default_value = ".")]
        workspace: PathBuf,

        /// Git repository URL to clone into workspace on boot
        #[arg(long = "repo")]
        repo: Option<String>,

        /// Git commit or reference to cherry-pick into the workspace on launch
        #[arg(long = "cherry-pick", alias = "pick")]
        cherry_pick: Option<String>,

        /// Automatically cherry-pick sandbox commits back to host workspace on session exit
        #[arg(long = "apply-to-host", alias = "sync-back")]
        apply_to_host: bool,

        /// Container base image
        #[arg(short = 'i', long = "image", default_value = "alpine:latest")]
        image: String,

        /// Number of virtual CPUs
        #[arg(short = 'c', long, default_value_t = 2)]
        cpus: u8,

        /// RAM in MiB
        #[arg(short = 'm', long, default_value_t = 1024)]
        memory: u32,

        /// Secrets to inject into guest (KEY=VAL, KEY (reads from host env), or @/path/to/file)
        #[arg(long = "secret", value_name = "KEY=VAL|KEY|@FILE")]
        secrets: Vec<String>,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,

        /// Optional command to execute inside the sandbox
        #[arg(last = true)]
        cmd: Vec<String>,
    },

    /// Execute a command inside a running microVM
    Exec {
        /// ID or PID of the running microVM
        id: String,

        /// Environment variables (KEY=VALUE)
        #[arg(short = 'e', long = "env")]
        env: Vec<String>,

        /// Working directory inside guest
        #[arg(short = 'w', long)]
        workdir: Option<String>,

        /// Allocate a pseudo-TTY
        #[arg(short = 't', long)]
        tty: bool,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,

        /// Command and arguments to execute
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true, num_args = 1..)]
        cmd: Vec<String>,
    },

    /// List running and recent microVMs
    Ps {
        /// Show all microVMs (including stopped)
        #[arg(short = 'a', long)]
        all: bool,

        /// Do not truncate IDs and image names
        #[arg(long = "no-trunc")]
        no_trunc: bool,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Stop a running microVM by ID or PID
    Stop {
        /// ID or PID of the microVM
        id: String,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Pause a running microVM (freezes all vCPUs)
    Pause {
        /// ID or PID of the microVM
        id: String,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Resume execution of a paused microVM
    Resume {
        /// ID or PID of the microVM
        id: String,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Remove one or more stopped microVMs
    Rm {
        /// ID or PID of the microVM
        id: String,

        /// Force removal of a running microVM (stops it first)
        #[arg(short = 'f', long)]
        force: bool,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Return low-level configuration and runtime details on a microVM
    Inspect {
        /// ID or PID of the microVM
        id: String,

        /// Format output as JSON
        #[arg(long)]
        json: bool,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Display live resource usage statistics for running microVMs
    Stats {
        /// ID or PID of a specific microVM (omit to display all running microVMs)
        id: Option<String>,

        /// Disable streaming live stats and only output the current snapshot
        #[arg(long = "no-stream")]
        no_stream: bool,

        /// Output telemetry formatted as JSON
        #[arg(long)]
        json: bool,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Display running supervisor and thread statistics of a microVM
    Top {
        /// ID or PID of the microVM
        id: String,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Copy files/folders between host and a microVM
    Cp {
        /// Source path (e.g. ./file.txt or <vm-id>:<path>)
        src: String,

        /// Destination path (e.g. <vm-id>:<path> or ./file.txt)
        dest: String,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// View console logs of a microVM
    Logs {
        /// ID or PID of the microVM
        id: String,

        /// Follow log output continuously
        #[arg(short = 'f', long)]
        follow: bool,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Remove stopped instances and temporary cache directories
    Prune {
        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Pull and cache an OCI image rootfs locally
    Pull {
        /// OCI image reference
        image: String,

        /// Custom cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Run preflight checks to verify hypervisor support
    Preflight {
        /// Ports to check availability for
        #[arg(short, long)]
        port: Vec<u16>,
    },

    /// Print system, virtualization, and resource cache status
    Info,

    /// Manage detached OCI artifacts (models, datasets, toolchains)
    Artifact {
        #[command(subcommand)]
        command: ArtifactCommands,
    },

    /// Capture an instant Copy-on-Write snapshot of a microVM
    Snapshot {
        /// ID or PID of the microVM
        id: String,

        /// Output path for snapshot (directory or .tar archive)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Restore a microVM from a snapshot for instant warm-start
    Restore {
        /// Path to the snapshot package (directory or .tar archive)
        snapshot: PathBuf,

        /// Optional new microVM instance ID
        #[arg(short, long)]
        name: Option<String>,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Dynamically resize CPU and memory resources of a running microVM
    Resize {
        /// ID or PID of the running microVM
        id: String,

        /// New memory allocation in MiB
        #[arg(short = 'm', long)]
        memory: Option<u32>,

        /// New virtual CPU count
        #[arg(short = 'c', long)]
        cpus: Option<u8>,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Export Prometheus metrics or start a live scrape HTTP server
    Metrics {
        /// Address to listen on for Prometheus scrapes (e.g. 127.0.0.1:9090 or 0.0.0.0:9090)
        #[arg(short, long)]
        listen: Option<String>,

        /// Format single-shot metrics output as JSON instead of Prometheus format
        #[arg(long)]
        json: bool,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Manage the containerd v2 runtime shim integration
    Containerd {
        #[command(subcommand)]
        command: ContainerdCommands,
    },

    /// Multi-microVM orchestration using declarative compose manifests (docker-compose compatible)
    Compose {
        #[command(subcommand)]
        command: ComposeCommands,
    },

    /// Apply a declarative YAML manifest (Docker Compose or Kubernetes CRD)
    Apply {
        /// Path to YAML manifest file (e.g. krun-compose.yaml, microvm.yaml)
        #[arg(short = 'f', long = "file")]
        file: PathBuf,

        /// Run microVM(s) in background
        #[arg(short = 'd', long)]
        detach: bool,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Cherry-pick commits from an isolated sandbox workspace back to the host git repository
    CherryPick {
        /// ID or PID of the sandbox microVM instance (from microvm ps -a)
        id: String,

        /// Target host workspace directory to apply commits to (defaults to current directory)
        #[arg(short = 'w', long = "workspace", default_value = ".")]
        workspace: PathBuf,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Manage microVM virtual networking, port mappings, and egress policies
    #[command(subcommand)]
    Network(NetworkCommands),
}

#[derive(Subcommand, Debug, Clone)]
pub enum NetworkCommands {
    /// List active microVM network configurations, modes, and interfaces
    #[command(name = "ls", alias = "list")]
    Ls {
        /// Format output as JSON
        #[arg(long)]
        json: bool,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Inspect detailed network topology, IPs, DNS, and egress rules of a microVM
    Inspect {
        /// ID or PID of the microVM
        id: String,

        /// Format output as JSON
        #[arg(long)]
        json: bool,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Display active host-to-guest port forwards across microVMs
    Ports {
        /// Optional microVM ID (if omitted, lists port forwards for all microVMs)
        id: Option<String>,

        /// Format output as JSON
        #[arg(long)]
        json: bool,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Test network connectivity, DNS resolution, and egress security from inside a microVM
    Test {
        /// ID or PID of the running microVM
        id: String,

        /// Target host or IP to test connectivity against (e.g. 1.1.1.1, api.openai.com)
        #[arg(default_value = "1.1.1.1")]
        target: String,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum ComposeCommands {
    /// Create and start microVM containers defined in compose manifest
    Up {
        /// Path to compose manifest file (defaults to auto-discovering krun-compose.yaml, compose.yaml, etc.)
        #[arg(short = 'f', long = "file")]
        file: Option<PathBuf>,

        /// Run containers in background
        #[arg(short = 'd', long)]
        detach: bool,

        /// Specific services to start (defaults to all)
        #[arg(value_name = "SERVICE")]
        services: Vec<String>,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Stop and remove microVM containers and networks
    Down {
        /// Path to compose manifest file
        #[arg(short = 'f', long = "file")]
        file: Option<PathBuf>,

        /// Remove named volumes declared in the volumes section
        #[arg(short = 'v', long = "volumes")]
        volumes: bool,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// List running microVM containers for the compose project
    Ps {
        /// Path to compose manifest file
        #[arg(short = 'f', long = "file")]
        file: Option<PathBuf>,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// View output logs from microVM services
    Logs {
        /// Path to compose manifest file
        #[arg(long = "file")]
        file: Option<PathBuf>,

        /// Specific service name to view logs for
        #[arg(value_name = "SERVICE")]
        service: Option<String>,

        /// Follow log output continuously
        #[arg(short = 'f', long)]
        follow: bool,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Stop running microVM services without removing state
    Stop {
        /// Path to compose manifest file
        #[arg(short = 'f', long = "file")]
        file: Option<PathBuf>,

        /// Specific services to stop (defaults to all)
        #[arg(value_name = "SERVICE")]
        services: Vec<String>,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Start stopped microVM services
    Start {
        /// Path to compose manifest file
        #[arg(short = 'f', long = "file")]
        file: Option<PathBuf>,

        /// Specific services to start (defaults to all)
        #[arg(value_name = "SERVICE")]
        services: Vec<String>,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Restart microVM services
    Restart {
        /// Path to compose manifest file
        #[arg(short = 'f', long = "file")]
        file: Option<PathBuf>,

        /// Specific services to restart (defaults to all)
        #[arg(value_name = "SERVICE")]
        services: Vec<String>,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// Validate and view the resolved compose configuration
    Config {
        /// Path to compose manifest file
        #[arg(short = 'f', long = "file")]
        file: Option<PathBuf>,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ArtifactCommands {
    /// Pull and cache an OCI artifact locally
    Pull {
        /// OCI artifact reference (e.g. ghcr.io/owner/model:latest)
        artifact: String,

        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },

    /// List locally cached OCI artifacts
    List {
        /// Custom data cache directory
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ContainerdCommands {
    /// Generate containerd config.toml runtime snippet and Kubernetes RuntimeClass YAML
    GenerateConfig,

    /// Install (symlink) containerd-shim-krun-v2 into a system path
    Install {
        /// Target directory for the shim binary
        #[arg(short, long, default_value = "/usr/local/bin")]
        target: PathBuf,
    },

    /// Check containerd daemon status and shim binary availability
    Status,
}

fn default_data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".cache/krun-microvm")
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Run(run) => {
            let RunArgs {
                image,
                bundle,
                cpus,
                memory,
                ports,
                volumes,
                artifacts,
                workspace_cow,
                env,
                workdir,
                data_dir,
                no_preflight,
                log_level,
                interactive,
                tty,
                detach,
                no_network,
                net,
                dns,
                hostname,
                mac,
                mtu,
                rlimits,
                dax,
                lazy_load,
                nydus_bootstrap,
                nydus_cache,
                chunk_size,
                gpu,
                gpu_shm_size,
                no_sandbox,
                allow_hosts,
                secrets,
                max_tokens,
                kernel,
                kernel_format,
                initrd,
                cmdline,
                firmware,
                disks,
                cmd,
                file,
            } = *run;

            if let Some(ref manifest_path) = file {
                let base_data_dir = data_dir.clone().unwrap_or_else(default_data_dir);
                let project = ComposeProject::load(manifest_path, Some(base_data_dir.clone()))?;
                return handle_compose_up(
                    &project,
                    manifest_path,
                    &base_data_dir,
                    detach,
                    &Vec::new(),
                )
                .await;
            }

            let mut builder = if let Some(ref b) = bundle {
                if !detach {
                    println!("📦 Loading MicroVM from OCI bundle: {}", b.display());
                }
                MicroVmBuilder::from_bundle(b)?
            } else if let Some(ref kpath) = kernel {
                if !detach {
                    println!("🚀 Direct kernel boot: {}", kpath.display());
                }
                let format_val = if let Some(ref fmt_str) = kernel_format {
                    parse_kernel_format(fmt_str).ok_or_else(|| {
                        anyhow::anyhow!(
                            "Invalid kernel format '{}'. Supported formats: raw, bin, m1n1, elf, vmlinux, gz, image.gz, vmlinuz, bz2, zstd, pe",
                            fmt_str
                        )
                    })?
                } else {
                    detect_kernel_format(kpath)
                };
                MicroVmBuilder::new("").kernel_with_format(kpath.clone(), format_val, initrd, cmdline)
            } else if let Some(ref fpath) = firmware {
                if !detach {
                    println!("🚀 UEFI firmware boot: {}", fpath.display());
                }
                MicroVmBuilder::new("").firmware(fpath.clone())
            } else if !image.is_empty() {
                if !detach {
                    println!("🚀 Launching MicroVM for image '{}'...", image);
                }
                MicroVmBuilder::new(image)
            } else {
                bail!("Please specify an image reference (e.g. 'alpine:latest'), '--kernel <path>', '--firmware <path>', or '--bundle <path>'");
            };

            for (i, d) in disks.iter().enumerate() {
                let (id, path, ro) = parse_disk_arg(d, i);
                builder = builder.disk(id, path, ro);
            }

            builder = builder
                .cpus(cpus)
                .memory_mb(memory)
                .preflight(!no_preflight)
                .interactive(interactive)
                .tty(tty)
                .detach(detach)
                .no_network(no_network)
                .sandbox(!no_sandbox);

            for h in allow_hosts {
                builder = builder.allow_host(h);
            }

            for s in secrets {
                let (k, v) = parse_secret_arg(&s)?;
                builder = builder.secret(k, v);
            }

            if let Some(tokens) = max_tokens {
                builder = builder.max_tokens(tokens);
            }

            if gpu {
                builder = builder.gpu(true);
                if let Some(shm) = gpu_shm_size {
                    let bytes = microvm_core::parse_size_to_bytes(&shm)
                        .context("Invalid --gpu-shm-size format (e.g. 2G, 512M)")?;
                    builder = builder.gpu_shm_size(bytes);
                }
            }

            if let Some(dax_size) = dax {
                builder = builder.dax_window_size_str(&dax_size)?;
            }

            if lazy_load || nydus_bootstrap.is_some() {
                builder = builder.lazy_load(true);
            }
            if let Some(bootstrap) = nydus_bootstrap {
                builder = builder.nydus_bootstrap(bootstrap);
            }
            if let Some(cache) = nydus_cache {
                builder = builder.chunk_cache_dir(cache);
            }
            if let Some(chunk_sz) = chunk_size {
                builder = builder.chunk_size_str(&chunk_sz)?;
            }

            if let Some(rlim) = rlimits {
                builder = builder.rlimits(rlim);
            }

            if let Some(dd) = data_dir {
                builder = builder.data_dir(dd);
            }

            if let Some(w) = workdir {
                builder = builder.workdir(w);
            }

            if let Some(lvl) = log_level {
                builder = builder.log_level(lvl);
            }

            if no_network || net == "none" {
                builder = builder.network_mode(microvm_core::NetworkMode::None);
            } else if net == "gvproxy" {
                builder = builder.network_mode(microvm_core::NetworkMode::Gvproxy);
            } else if let Some(path_str) = net.strip_prefix("unix:") {
                builder = builder.network_mode(microvm_core::NetworkMode::UnixStream(
                    PathBuf::from(path_str),
                ));
            } else if let Some(netns_str) = net.strip_prefix("cni:") {
                builder = builder.network_mode(microvm_core::NetworkMode::Cni {
                    netns: PathBuf::from(netns_str),
                    socket_path: None,
                });
            } else {
                builder = builder.network_mode(microvm_core::NetworkMode::Tsi);
            }

            if !dns.is_empty() {
                builder = builder.dns_servers(dns);
            }

            if let Some(h) = hostname {
                builder = builder.hostname(h);
            }

            if let Some(m) = mac {
                builder = builder.mac_address(m);
            }

            if let Some(mtu_val) = mtu {
                builder = builder.mtu(mtu_val);
            }

            if !cmd.is_empty() {
                builder = builder.cmd(cmd);
            }

            for p in ports {
                let parts: Vec<&str> = p.split(':').collect();
                if parts.len() != 2 {
                    bail!("Invalid port forward format '{}', expected host:guest", p);
                }
                let host: u16 = parts[0].parse().context("Invalid host port")?;
                let guest: u16 = parts[1].parse().context("Invalid guest port")?;
                builder = builder.port_forward(host, guest);
            }

            for v in volumes {
                let parts: Vec<&str> = v.split(':').collect();
                if parts.len() < 2 {
                    bail!("Invalid volume format '{}', expected host_path:tag[:ro]", v);
                }
                let host_path = PathBuf::from(parts[0]);
                let tag = parts[1];
                let ro = parts.get(2).is_some_and(|&s| s == "ro");
                builder = builder.virtiofs(tag, host_path, ro);
            }

            for a in artifacts {
                let (rem, ro) = if let Some(stripped) = a.strip_suffix(":ro") {
                    (stripped, true)
                } else if let Some(stripped) = a.strip_suffix(":rw") {
                    (stripped, false)
                } else {
                    (a.as_str(), false)
                };

                let (reference, tag) = match rem.rfind(':') {
                    Some(idx) => (&rem[..idx], &rem[idx + 1..]),
                    None => bail!(
                        "Invalid artifact format '{}', expected <reference>:<tag>[:ro]",
                        a
                    ),
                };
                builder = builder.attach_artifact(reference, tag, ro);
            }

            for w in workspace_cow {
                let (host_path, tag) = match w.rfind(':') {
                    Some(idx) => (&w[..idx], &w[idx + 1..]),
                    None => bail!(
                        "Invalid workspace-cow format '{}', expected <host_path>:<tag>",
                        w
                    ),
                };
                builder = builder.workspace_cow(PathBuf::from(host_path), tag);
            }

            for e in env {
                if let Some((k, v)) = e.split_once('=') {
                    builder = builder.env(k, v);
                } else {
                    bail!("Invalid env var format '{}', expected KEY=VALUE", e);
                }
            }

            let mut vm = builder.run().await.context("Failed to start microVM")?;
            if detach {
                println!("{}", vm.id());
                return Ok(());
            }

            println!("✅ MicroVM active! ID: {}, PID: {:?}", vm.id(), vm.pid());

            // Concurrently wait for VM exit or Ctrl+C signal
            let exit_code = tokio::select! {
                status_res = vm.wait() => {
                    let status = status_res?;
                    println!("🛑 MicroVM exited with status: {}", status);
                    status.code().unwrap_or(0)
                }
                _ = tokio::signal::ctrl_c() => {
                    eprintln!("\n⚠️ Received interrupt (Ctrl+C). Gracefully stopping microVM {}...", vm.id());
                    let _ = vm.stop().await;
                    130 // Standard exit code: 128 + SIGINT
                }
            };

            if exit_code != 0 {
                std::process::exit(exit_code);
            }
        }

        Commands::Unikernel {
            kernel,
            cpus,
            memory,
            cmdline,
            disks,
            data_dir,
        } => {
            let mut builder = MicroVmBuilder::new("")
                .cpus(cpus)
                .memory_mb(memory)
                .unikernel(kernel, cmdline)
                .interactive(true)
                .tty(true);

            if let Some(dd) = data_dir {
                builder = builder.data_dir(dd);
            }

            for (i, d) in disks.iter().enumerate() {
                let (id, path, ro) = parse_disk_arg(d, i);
                builder = builder.disk(id, path, ro);
            }

            println!(
                "🚀 Launching unikernel in microVM ({} vCPUs, {} MiB RAM)...",
                cpus, memory
            );
            let mut vm = builder.run().await.context("Failed to boot unikernel")?;
            let status = vm.wait().await?;
            println!("🛑 Unikernel exited with status: {}", status);
            std::process::exit(status.code().unwrap_or(0));
        }

        Commands::Sandbox {
            agent,
            workspace,
            repo,
            cherry_pick,
            apply_to_host,
            image,
            cpus,
            memory,
            secrets,
            data_dir,
            cmd,
        } => {
            let abs_ws = std::fs::canonicalize(&workspace).unwrap_or(workspace);
            println!(
                "🛡️  Initializing microVM Sandbox for AI Agent '{}'...",
                agent
            );
            println!("📁 Isolated Workspace: {}", abs_ws.display());

            let mut builder = MicroVmBuilder::new(&image)
                .cpus(cpus)
                .memory_mb(memory)
                .interactive(true)
                .tty(true)
                .workspace_cow(&abs_ws, "workspace")
                .workdir("/workspace");

            if let Some(ref dd) = data_dir {
                builder = builder.data_dir(dd.clone());
            }

            // Inject host git credentials so git operations (cherry-pick, commit) succeed
            let host_git_name = std::process::Command::new("git")
                .args(["config", "user.name"])
                .current_dir(&abs_ws)
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Sandbox Developer".to_string());

            let host_git_email = std::process::Command::new("git")
                .args(["config", "user.email"])
                .current_dir(&abs_ws)
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "developer@libkrun.local".to_string());

            builder = builder
                .env("GIT_AUTHOR_NAME", &host_git_name)
                .env("GIT_AUTHOR_EMAIL", &host_git_email)
                .env("GIT_COMMITTER_NAME", &host_git_name)
                .env("GIT_COMMITTER_EMAIL", &host_git_email);

            // Inject any explicit secrets passed via --secret
            for s in &secrets {
                let (k, v) = parse_secret_arg(s)?;
                builder = builder.secret(k, v);
            }

            // Configure AI Agent specific egress rules and environment
            match agent.to_lowercase().as_str() {
                "claude" => {
                    builder = builder
                        .allow_host("api.anthropic.com:443")
                        .allow_host("cdn.anthropic.com:443")
                        .allow_host("github.com:443")
                        .allow_host("api.github.com:443")
                        .allow_host("registry.npmjs.org:443");
                    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
                        builder = builder.secret("ANTHROPIC_API_KEY", &key);
                    } else if !secrets.iter().any(|s| s.starts_with("ANTHROPIC_API_KEY")) {
                        eprintln!("⚠️  ANTHROPIC_API_KEY not found in host environment. Agent may fail to authenticate.");
                    }
                }
                "gemini" => {
                    builder = builder
                        .allow_host("generativelanguage.googleapis.com:443")
                        .allow_host("github.com:443")
                        .allow_host("api.github.com:443");
                    if let Ok(key) = std::env::var("GEMINI_API_KEY") {
                        builder = builder.secret("GEMINI_API_KEY", &key);
                    } else if !secrets.iter().any(|s| s.starts_with("GEMINI_API_KEY")) {
                        eprintln!("⚠️  GEMINI_API_KEY not found in host environment. Agent may fail to authenticate.");
                    }
                }
                "codex" | "openai" => {
                    builder = builder
                        .allow_host("api.openai.com:443")
                        .allow_host("github.com:443")
                        .allow_host("api.github.com:443");
                    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
                        builder = builder.secret("OPENAI_API_KEY", &key);
                    } else if !secrets.iter().any(|s| s.starts_with("OPENAI_API_KEY")) {
                        eprintln!("⚠️  OPENAI_API_KEY not found in host environment. Agent may fail to authenticate.");
                    }
                }
                _ => {
                    builder = builder
                        .allow_host("github.com:443")
                        .allow_host("api.github.com:443");
                }
            }

            let mut script_parts = Vec::new();
            script_parts.push(
                r#"if command -v git >/dev/null 2>&1; then
    git config --global --add safe.directory '*' 2>/dev/null || true
    [ -n "$GIT_AUTHOR_NAME" ] && git config --global user.name "$GIT_AUTHOR_NAME" 2>/dev/null || true
    [ -n "$GIT_AUTHOR_EMAIL" ] && git config --global user.email "$GIT_AUTHOR_EMAIL" 2>/dev/null || true
fi"#.to_string(),
            );

            if let Some(ref git_repo) = repo {
                script_parts.push(format!(
                    r#"if [ ! -d .git ]; then
    echo "📦 Cloning {git_repo} into workspace..."
    git clone "{git_repo}" .
    git config --global --add safe.directory '*' 2>/dev/null || true
fi"#
                ));
            }

            if let Some(ref cp_ref) = cherry_pick {
                script_parts.push(format!(
                    r#"if command -v git >/dev/null 2>&1 && [ -d .git ]; then
    echo "🍒 Cherry-picking commit '{cp_ref}' in isolated sandbox..."
    if git cherry-pick "{cp_ref}"; then
        echo "✅ Successfully cherry-picked '{cp_ref}' in sandbox!"
    else
        echo "⚠️  git cherry-pick encountered conflicts. The sandbox is ready for inspection/resolution."
    fi
else
    echo "⚠️  Cannot cherry-pick: git is not installed or workspace is not a git repository."
fi"#
                ));
            }

            if !cmd.is_empty() {
                let joined_cmd = cmd
                    .iter()
                    .map(|s| format!("'{}'", s.replace('\'', "'\\''")))
                    .collect::<Vec<_>>()
                    .join(" ");
                script_parts.push(format!("exec {joined_cmd}"));
            } else {
                script_parts.push("exec /bin/sh".to_string());
            }

            let startup_script = script_parts.join("\n");
            builder = builder.cmd(vec!["/bin/sh".to_string(), "-c".to_string(), startup_script]);

            let mut vm = builder.run().await.context("Failed to start sandbox")?;
            let status = vm.wait().await?;
            println!("🛑 Sandbox session closed with status: {}", status);

            let base_dir = data_dir.unwrap_or_else(default_data_dir);
            let cow_ws = base_dir
                .join("instances")
                .join(vm.id())
                .join("workspaces")
                .join("workspace");

            if abs_ws.join(".git").exists() && cow_ws.join(".git").exists() {
                let host_head = get_git_head(&abs_ws);
                let sandbox_head = get_git_head(&cow_ws);

                if let (Some(ref h_head), Some(ref s_head)) = (&host_head, &sandbox_head) {
                    if h_head != s_head {
                        let commits = get_git_commits_between(&cow_ws, h_head, s_head);
                        if !commits.is_empty() {
                            println!("\n📦 Sandbox produced {} new git commit(s):", commits.len());
                            for c in &commits {
                                println!("   • {c}");
                            }

                            if apply_to_host {
                                println!("🚀 Cherry-picking sandbox commit(s) to host workspace...");
                                match cherry_pick_sandbox_to_host(&cow_ws, &abs_ws, h_head, s_head) {
                                    Ok(_) => println!("✅ Successfully cherry-picked sandbox changes to host branch!"),
                                    Err(e) => eprintln!("⚠️ Failed to cherry-pick to host: {e}"),
                                }
                            } else {
                                println!("\n💡 Tip: To apply these commits to your host branch, run:");
                                println!("   microvm cherry-pick {}", vm.id());
                            }
                        }
                    }
                }
            }

            std::process::exit(status.code().unwrap_or(0));
        }

        Commands::Ps {
            all,
            no_trunc,
            data_dir,
        } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            let vms = StateManager::list(&base)?;

            let displayed: Vec<_> = vms
                .into_iter()
                .filter(|v| all || v.status == VmStatus::Running || v.status == VmStatus::Paused)
                .collect();

            if displayed.is_empty() {
                println!("No {} microVMs found.", if all { "" } else { "running" });
                return Ok(());
            }

            println!(
                "{:<16} {:<24} {:<8} {:<18} {:<18}",
                "INSTANCE ID", "IMAGE", "PID", "STATUS", "PORTS"
            );
            println!("{:-<90}", "");

            for vm in displayed {
                let id_display = if no_trunc || vm.id.len() <= 12 {
                    vm.id.clone()
                } else {
                    vm.id[..12].to_string()
                };

                let time_rel = format_duration_since(vm.created_at);
                let status_str = match vm.status {
                    VmStatus::Running => format!("● Up ({})", time_rel),
                    VmStatus::Paused => format!("⏸ Paused ({})", time_rel),
                    VmStatus::Stopped => format!("○ Exited ({})", time_rel),
                };

                let port_str = if vm.port_forwards.is_empty() {
                    "-".to_string()
                } else {
                    vm.port_forwards
                        .iter()
                        .map(|p| format!("{}:{}", p.host, p.guest))
                        .collect::<Vec<_>>()
                        .join(", ")
                };

                let image_display = if vm.image.len() > 23 && !no_trunc {
                    format!("{}...", &vm.image[..20])
                } else {
                    vm.image.clone()
                };

                println!(
                    "{:<16} {:<24} {:<8} {:<18} {:<18}",
                    id_display, image_display, vm.pid, status_str, port_str
                );
            }
        }

        Commands::Exec {
            id,
            env,
            workdir,
            tty,
            data_dir,
            cmd,
        } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            let mut req = microvm_core::ExecRequest::new(cmd)
                .with_env(env)
                .with_tty(tty);
            if let Some(wd) = workdir {
                req = req.with_workdir(wd);
            }
            let resp = StateManager::exec(&base, &id, &req).await?;
            if !resp.stdout.is_empty() {
                print!("{}", resp.stdout);
            }
            if !resp.stderr.is_empty() {
                eprint!("{}", resp.stderr);
            }
            if let Some(err) = resp.error {
                eprintln!("Exec error: {}", err);
            }
            if resp.exit_code != 0 {
                std::process::exit(resp.exit_code);
            }
        }

        Commands::Stop { id, data_dir } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            println!("Stopping microVM '{}'...", id);
            StateManager::stop(&base, &id)?;
            println!("✅ MicroVM '{}' stopped and state cleaned up.", id);
        }

        Commands::Pause { id, data_dir } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            match StateManager::pause(&base, &id) {
                Ok(vm) => {
                    println!("⏸️  Paused microVM '{}' (PID {})", vm.id, vm.pid);
                }
                Err(e) => {
                    bail!("Failed to pause microVM '{}': {}", id, e);
                }
            }
        }

        Commands::Resume { id, data_dir } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            match StateManager::resume(&base, &id) {
                Ok(vm) => {
                    println!("▶️  Resumed microVM '{}' (PID {})", vm.id, vm.pid);
                }
                Err(e) => {
                    bail!("Failed to resume microVM '{}': {}", id, e);
                }
            }
        }

        Commands::Rm {
            id,
            force,
            data_dir,
        } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            match StateManager::delete(&base, &id, force) {
                Ok(vm) => {
                    println!(
                        "✅ Removed microVM '{}' (Image: {}, PID: {})",
                        vm.id, vm.image, vm.pid
                    );
                }
                Err(e) => {
                    bail!("Failed to remove microVM '{}': {}", id, e);
                }
            }
        }

        Commands::Inspect { id, json, data_dir } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            let vm = match StateManager::find(&base, &id)? {
                Some(v) => v,
                None => bail!("MicroVM '{}' not found in state", id),
            };

            let is_alive = vm.is_process_alive();
            let telemetry = if is_alive {
                collect_process_stats(vm.pid)
            } else {
                None
            };

            let config_json_path = vm.instance_dir.join("config.json");
            let oci_config: Option<serde_json::Value> = if config_json_path.exists() {
                std::fs::read_to_string(&config_json_path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
            } else {
                None
            };

            let console_log = vm.instance_dir.join("console.log");

            if json {
                let info = json!({
                    "id": vm.id,
                    "pid": vm.pid,
                    "image": vm.image,
                    "status": if is_alive { "Running" } else { "Stopped" },
                    "created_at": vm.created_at,
                    "vcpus": vm.vcpus,
                    "memory_mib": vm.memory_mib,
                    "port_forwards": vm.port_forwards,
                    "instance_dir": vm.instance_dir.display().to_string(),
                    "console_log": console_log.display().to_string(),
                    "telemetry": telemetry,
                    "oci_config": oci_config,
                });
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                println!("📦 MicroVM Inspection: {}", vm.id);
                println!("{:-<60}", "");
                println!(
                    "  Status:          {}",
                    if is_alive {
                        format!("● Running (PID {})", vm.pid)
                    } else {
                        "○ Stopped".to_string()
                    }
                );
                println!("  Image:           {}", vm.image);
                if let Some(c) = vm.vcpus {
                    println!("  Configured CPUs: {}", c);
                }
                if let Some(m) = vm.memory_mib {
                    println!("  Allocated RAM:   {} MiB", m);
                }
                println!(
                    "  Created:         {} ({})",
                    format_duration_since(vm.created_at),
                    vm.created_at
                );
                println!("  Instance Path:   {}", vm.instance_dir.display());
                println!("  Console Log:     {}", console_log.display());
                if !vm.port_forwards.is_empty() {
                    let ports: Vec<String> = vm
                        .port_forwards
                        .iter()
                        .map(|p| format!("{}:{}", p.host, p.guest))
                        .collect();
                    println!("  Port Mappings:   {}", ports.join(", "));
                }

                if let Some(ref oci) = oci_config {
                    if let Some(entrypoint) = oci.get("entrypoint").and_then(|v| v.as_array()) {
                        let ep: Vec<&str> = entrypoint.iter().filter_map(|s| s.as_str()).collect();
                        println!("  Entrypoint:      {:?}", ep);
                    }
                    if let Some(cmd) = oci.get("cmd").and_then(|v| v.as_array()) {
                        let c: Vec<&str> = cmd.iter().filter_map(|s| s.as_str()).collect();
                        println!("  Cmd:             {:?}", c);
                    }
                    if let Some(workdir) = oci.get("working_dir").and_then(|v| v.as_str()) {
                        if !workdir.is_empty() {
                            println!("  Working Dir:     {}", workdir);
                        }
                    }
                }

                if let Some(ref stats) = telemetry {
                    println!("\n  Telemetry (Live):");
                    println!(
                        "    CPU Time (Total):  {:.2} ms (User: {:.2} ms, Sys: {:.2} ms)",
                        stats.total_cpu_ns as f64 / 1_000_000.0,
                        stats.user_cpu_ns as f64 / 1_000_000.0,
                        stats.kernel_cpu_ns as f64 / 1_000_000.0,
                    );
                    println!(
                        "    Memory (RSS):      {}",
                        format_bytes(stats.memory_rss_bytes)
                    );
                    println!(
                        "    Memory (Virtual):  {}",
                        format_bytes(stats.memory_vsize_bytes)
                    );
                    println!("    Active Threads:    {}", stats.threads);
                    println!(
                        "    Page Faults:       {} (Major: {})",
                        stats.page_faults, stats.major_page_faults
                    );
                }
            }
        }

        Commands::Stats {
            id,
            no_stream,
            json,
            data_dir,
        } => {
            let base = data_dir.unwrap_or_else(default_data_dir);

            if json {
                let vms = StateManager::list(&base)?;
                let targets: Vec<_> = match id {
                    Some(ref target_id) => {
                        let vm = StateManager::find(&base, target_id)?
                            .ok_or_else(|| anyhow::anyhow!("MicroVM '{}' not found", target_id))?;
                        vec![vm]
                    }
                    None => vms.into_iter().filter(|v| v.is_process_alive()).collect(),
                };

                let mut list = Vec::new();
                for vm in targets {
                    let stats = if vm.is_process_alive() {
                        collect_process_stats(vm.pid)
                    } else {
                        None
                    };
                    list.push(json!({
                        "id": vm.id,
                        "pid": vm.pid,
                        "image": vm.image,
                        "status": if vm.is_process_alive() { "Running" } else { "Stopped" },
                        "stats": stats,
                    }));
                }
                println!("{}", serde_json::to_string_pretty(&list)?);
                return Ok(());
            }

            let sample_interval =
                std::time::Duration::from_millis(if no_stream { 250 } else { 1000 });

            loop {
                let vms = StateManager::list(&base)?;
                let active_vms: Vec<_> = match id {
                    Some(ref target_id) => {
                        let vm = StateManager::find(&base, target_id)?
                            .ok_or_else(|| anyhow::anyhow!("MicroVM '{}' not found", target_id))?;
                        if !vm.is_process_alive() {
                            bail!("MicroVM '{}' is not running (PID {})", vm.id, vm.pid);
                        }
                        vec![vm]
                    }
                    None => vms.into_iter().filter(|v| v.is_process_alive()).collect(),
                };

                if active_vms.is_empty() {
                    println!("No running microVMs found.");
                    return Ok(());
                }

                // Sample 1
                let mut samples1 = Vec::new();
                for vm in &active_vms {
                    if let Some(s) = collect_process_stats(vm.pid) {
                        samples1.push((vm.clone(), std::time::Instant::now(), s));
                    }
                }

                tokio::select! {
                    _ = tokio::time::sleep(sample_interval) => {}
                    _ = tokio::signal::ctrl_c() => {
                        if !no_stream {
                            println!();
                        }
                        return Ok(());
                    }
                }

                // Sample 2
                let mut rows = Vec::new();
                for (vm, t1, s1) in samples1 {
                    if let Some(s2) = collect_process_stats(vm.pid) {
                        let t2 = std::time::Instant::now();
                        let delta_wall = t2.duration_since(t1).as_nanos() as f64;
                        let delta_cpu = s2.total_cpu_ns.saturating_sub(s1.total_cpu_ns) as f64;
                        let cpu_pct = if delta_wall > 0.0 {
                            (delta_cpu / delta_wall) * 100.0
                        } else {
                            0.0
                        };
                        rows.push((vm, cpu_pct, s2));
                    }
                }

                if !no_stream {
                    // Clear terminal and reset cursor
                    print!("\x1B[2J\x1B[1;1H");
                }

                println!(
                    "{:<14} {:<24} {:<10} {:<14} {:<14} {:<8} {:<12}",
                    "CONTAINER ID",
                    "IMAGE",
                    "CPU %",
                    "MEM USAGE",
                    "VIRTUAL MEM",
                    "PIDS",
                    "PAGE FAULTS"
                );
                println!("{:-<100}", "");

                for (vm, cpu_pct, stats) in rows {
                    let short_id = if vm.id.len() > 12 {
                        &vm.id[..12]
                    } else {
                        &vm.id
                    };
                    let short_img = if vm.image.len() > 23 {
                        format!("{}...", &vm.image[..20])
                    } else {
                        vm.image
                    };
                    println!(
                        "{:<14} {:<24} {:<10} {:<14} {:<14} {:<8} {:<12}",
                        short_id,
                        short_img,
                        format!("{:.2}%", cpu_pct),
                        format_bytes(stats.memory_rss_bytes),
                        format_bytes(stats.memory_vsize_bytes),
                        stats.threads,
                        stats.page_faults,
                    );
                }

                if no_stream {
                    break;
                }
            }
        }

        Commands::Top { id, data_dir } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            let vm = match StateManager::find(&base, &id)? {
                Some(v) => v,
                None => bail!("MicroVM '{}' not found in state", id),
            };

            if !vm.is_process_alive() {
                bail!("MicroVM '{}' is not running (PID {})", vm.id, vm.pid);
            }

            let stats = match collect_process_stats(vm.pid) {
                Some(s) => s,
                None => bail!("Failed to collect process stats for PID {}", vm.pid),
            };

            println!("Top - MicroVM {} (PID {})", vm.id, vm.pid);
            println!("{:-<75}", "");
            println!(
                "{:<10} {:<10} {:<16} {:<16} {:<14}",
                "PID", "THREADS", "USER CPU", "SYS CPU", "RSS MEMORY"
            );
            println!(
                "{:<10} {:<10} {:<16} {:<16} {:<14}",
                vm.pid,
                stats.threads,
                format!("{:.2} ms", stats.user_cpu_ns as f64 / 1_000_000.0),
                format!("{:.2} ms", stats.kernel_cpu_ns as f64 / 1_000_000.0),
                format_bytes(stats.memory_rss_bytes)
            );
        }

        Commands::Cp {
            src,
            dest,
            data_dir,
        } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            if let Some((vm_id, guest_path)) = dest.split_once(':') {
                let src_path = PathBuf::from(&src);
                if !src_path.exists() {
                    bail!("Source path does not exist: {}", src);
                }
                StateManager::copy_into(&base, vm_id, &src_path, guest_path)?;
                println!(
                    "✅ Successfully copied '{}' into '{}:{}'",
                    src, vm_id, guest_path
                );
            } else if let Some((vm_id, guest_path)) = src.split_once(':') {
                let dest_path = PathBuf::from(&dest);
                StateManager::copy_from(&base, vm_id, guest_path, &dest_path)?;
                println!(
                    "✅ Successfully copied '{}:{}' to '{}'",
                    vm_id, guest_path, dest
                );
            } else {
                bail!("Invalid cp syntax. Usage:\n  microvm cp <src_host> <vm_id>:<dest_guest>\n  microvm cp <vm_id>:<src_guest> <dest_host>");
            }
        }

        Commands::Logs {
            id,
            follow,
            data_dir,
        } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            let vms = StateManager::list(&base)?;
            let target = vms
                .iter()
                .find(|v| v.id == id || v.id.starts_with(&id) || v.pid.to_string() == id);

            let vm = match target {
                Some(v) => v,
                None => bail!("MicroVM '{}' not found in state", id),
            };

            let console_log = vm.instance_dir.join("console.log");
            let trace_log = vm.instance_dir.join("rootfs/init.trace.log");
            let log_file = if console_log.exists() {
                console_log
            } else if trace_log.exists() {
                trace_log
            } else if follow {
                console_log
            } else {
                println!("No log output found for VM '{}'", id);
                return Ok(());
            };

            use std::io::{Read, Write};
            let mut file = match std::fs::File::open(&log_file) {
                Ok(f) => f,
                Err(_) if follow => {
                    let start = std::time::Instant::now();
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        if let Ok(f) = std::fs::File::open(&log_file) {
                            break f;
                        }
                        if start.elapsed() > std::time::Duration::from_secs(5) {
                            bail!(
                                "Log file '{}' was not created after 5 seconds",
                                log_file.display()
                            );
                        }
                    }
                }
                Err(e) => bail!("Failed to open log file '{}': {}", log_file.display(), e),
            };

            let mut buffer = Vec::new();
            let _ = file.read_to_end(&mut buffer);
            if !buffer.is_empty() {
                print!("{}", String::from_utf8_lossy(&buffer));
                let _ = std::io::stdout().flush();
            }

            if follow {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    let mut new_bytes = Vec::new();
                    let n = file.read_to_end(&mut new_bytes)?;
                    if n > 0 {
                        print!("{}", String::from_utf8_lossy(&new_bytes));
                        let _ = std::io::stdout().flush();
                    } else {
                        let vms = StateManager::list(&base)?;
                        let is_alive = vms.iter().any(|v| (v.id == vm.id) && v.is_process_alive());
                        if !is_alive {
                            let mut final_bytes = Vec::new();
                            let _ = file.read_to_end(&mut final_bytes);
                            if !final_bytes.is_empty() {
                                print!("{}", String::from_utf8_lossy(&final_bytes));
                                let _ = std::io::stdout().flush();
                            }
                            break;
                        }
                    }
                }
            }
        }

        Commands::Prune { data_dir } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            println!("Pruning stopped microVMs and staging caches...");
            let summary = StateManager::prune(&base)?;
            println!(
                "✅ Pruned {} dead instance directories and {} staging cache directories.",
                summary.pruned_instances, summary.pruned_layers
            );
        }

        Commands::Pull { image, data_dir } => {
            let cache_base = data_dir.unwrap_or_else(default_data_dir);

            println!("📦 Pulling image '{}'...", image);
            let reference = ImageReference::parse(&image)?;
            let (rootfs, config) = if reference.is_local_layout {
                let layout_path = reference.layout_path.as_ref().unwrap();
                println!(
                    "📂 Loading local OCI image layout from '{}'...",
                    layout_path.display()
                );
                let (rootfs, config) =
                    OciLayout::load(layout_path, Some(&reference.tag), &cache_base)?;
                println!("✅ Offline OCI layout loaded and unpacked!");
                (rootfs, config)
            } else {
                let client = OciClient::new();
                let (rootfs, config) = client.pull_and_unpack(&reference, &cache_base).await?;
                println!("✅ Image successfully pulled and cached!");
                (rootfs, config)
            };

            println!("   Rootfs path: {}", rootfs.display());
            println!("   Entrypoint: {:?}", config.entrypoint);
            println!("   Cmd: {:?}", config.cmd);
            println!("   Env count: {}", config.env.len());
        }

        Commands::Preflight { port } => {
            println!("🔍 Running MicroVM preflight checks...\n");
            let workdir = default_data_dir();
            let results = Preflight::run_all(&port, &workdir);

            let mut all_ok = true;
            for r in results {
                let status_icon = if r.passed { " [PASS]" } else { "❌ [FAIL]" };
                println!("{} {}: {}", status_icon, r.name, r.message);
                if !r.passed {
                    all_ok = false;
                }
            }

            if all_ok {
                println!("\n🎉 System is fully ready to run hardware-isolated microVMs!");
            } else {
                println!(
                    "\n⚠️ Some preflight checks failed. Please address them before running VMs."
                );
            }
        }

        Commands::Info => {
            let base = default_data_dir();
            let vms = StateManager::list(&base).unwrap_or_default();
            let running_count = vms.iter().filter(|v| v.is_process_alive()).count();
            let stopped_count = vms.len().saturating_sub(running_count);

            let layers_size = dir_size(&base.join("layers"));
            let artifacts_size = dir_size(&base.join("artifacts"));
            let instances_size = dir_size(&base.join("instances"));
            let total_cache = layers_size + artifacts_size + instances_size;

            let cpus = std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(1);
            let mem_str = host_memory_bytes()
                .map(format_bytes)
                .unwrap_or_else(|| "Unknown".to_string());

            println!("libkrun-microvm SDK (Rust Edition)");
            println!("--------------------------------------------------");
            println!("OS:                      {}", std::env::consts::OS);
            println!("Architecture:            {}", std::env::consts::ARCH);
            println!(
                "Hypervisor:              {}",
                if cfg!(target_os = "macos") {
                    "Apple Silicon Hypervisor.framework"
                } else {
                    "Linux KVM"
                }
            );
            println!("Host CPUs:               {}", cpus);
            println!("Host Memory:             {}", mem_str);
            println!("Data Cache Root:         {}", base.display());
            println!("--------------------------------------------------");
            println!(
                "Active MicroVMs:         {} running ({} total)",
                running_count,
                vms.len()
            );
            println!("  - Running:             {}", running_count);
            println!("  - Stopped:             {}", stopped_count);
            println!("Disk Utilization:");
            println!("  - OCI Layer Cache:     {}", format_bytes(layers_size));
            println!("  - OCI Artifacts:       {}", format_bytes(artifacts_size));
            println!("  - MicroVM Instances:   {}", format_bytes(instances_size));
            println!("  - Total Disk Used:     {}", format_bytes(total_cache));
            println!("libkrun Library:         Dynamic linkage (/opt/homebrew/lib or system lib)");
        }

        Commands::Artifact { command } => match command {
            ArtifactCommands::Pull { artifact, data_dir } => {
                let cache_base = data_dir.unwrap_or_else(default_data_dir);
                println!("📦 Pulling OCI artifact '{}'...", artifact);
                let reference = ImageReference::parse(&artifact)?;
                let client = OciClient::new();
                let dest = client.pull_artifact(&reference, &cache_base).await?;
                println!("✅ OCI artifact pulled and cached successfully!");
                println!("   Artifact directory: {}", dest.display());
            }
            ArtifactCommands::List { data_dir } => {
                let cache_base = data_dir.unwrap_or_else(default_data_dir);
                let artifacts = OciArtifact::list_cached(&cache_base)?;
                if artifacts.is_empty() {
                    println!("No locally cached OCI artifacts found.");
                    return Ok(());
                }

                println!(
                    "{:<40} {:<20} {:<12} {:<20}",
                    "REFERENCE", "DIGEST", "FILES", "PULLED AT"
                );
                println!("{:-<95}", "");
                for a in artifacts {
                    let short_digest = if a.digest.len() > 19 {
                        format!("{}...", &a.digest[..16])
                    } else {
                        a.digest.clone()
                    };
                    let time_str = match std::time::UNIX_EPOCH
                        .checked_add(std::time::Duration::from_secs(a.created_at))
                    {
                        Some(t) => {
                            let dur = std::time::SystemTime::now()
                                .duration_since(t)
                                .unwrap_or_default();
                            if dur.as_secs() < 60 {
                                format!("{}s ago", dur.as_secs())
                            } else if dur.as_secs() < 3600 {
                                format!("{}m ago", dur.as_secs() / 60)
                            } else if dur.as_secs() < 86400 {
                                format!("{}h ago", dur.as_secs() / 3600)
                            } else {
                                format!("{}d ago", dur.as_secs() / 86400)
                            }
                        }
                        None => "Unknown".to_string(),
                    };
                    println!(
                        "{:<40} {:<20} {:<12} {:<20}",
                        a.reference,
                        short_digest,
                        format!("{} file(s)", a.files.len()),
                        time_str,
                    );
                }
            }
        },

        Commands::Snapshot {
            id,
            output,
            data_dir,
        } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            let out_path =
                output.unwrap_or_else(|| base.join("snapshots").join(format!("{}.tar", id)));
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let manifest = StateManager::snapshot(&base, &id, &out_path)?;
            println!("📸 Snapshot created successfully!");
            println!("   Target: {}", out_path.display());
            println!(
                "   Origin: {} (Image: {})",
                manifest.original_id, manifest.image
            );
        }

        Commands::Restore {
            snapshot,
            name,
            data_dir,
        } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            let restored = StateManager::restore(&base, &snapshot, name.as_deref())?;
            println!("⚡ MicroVM restored successfully from snapshot!");
            println!("   ID:     {}", restored.id);
            println!("   Image:  {}", restored.image);
            println!("   Rootfs: {}", restored.rootfs_path().display());
            println!(
                "💡 Run 'microvm run --bundle {}' or use with runner to boot.",
                restored.instance_dir.display()
            );
        }

        Commands::Resize {
            id,
            memory,
            cpus,
            data_dir,
        } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            let updated = StateManager::resize(&base, &id, memory, cpus)?;
            println!("✅ MicroVM '{}' resized successfully:", updated.id);
            if let Some(m) = updated.memory_mib {
                println!("   RAM:   {} MiB", m);
            }
            if let Some(c) = updated.vcpus {
                println!("   vCPUs: {}", c);
            }
        }

        Commands::Metrics {
            listen,
            json,
            data_dir,
        } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            if let Some(addr) = listen {
                start_metrics_server(&base, &addr, json).await?;
            } else {
                let vms = StateManager::list(&base)?;
                if json {
                    let summary = json!({
                        "total": vms.len(),
                        "running": vms.iter().filter(|v| v.status == VmStatus::Running).count(),
                        "paused": vms.iter().filter(|v| v.status == VmStatus::Paused).count(),
                        "stopped": vms.iter().filter(|v| v.status == VmStatus::Stopped).count(),
                        "vms": vms,
                    });
                    println!("{}", serde_json::to_string_pretty(&summary)?);
                } else {
                    let exposition = microvm_core::metrics::export_prometheus_metrics(&vms);
                    print!("{}", exposition);
                }
            }
        }

        Commands::Containerd { command } => match command {
            ContainerdCommands::GenerateConfig => {
                println!("# containerd runtime configuration snippet");
                println!("# Add the following to /etc/containerd/config.toml\n");
                println!("[plugins.\"io.containerd.grpc.v1.cri\".containerd.runtimes.krun]");
                println!("  runtime_type = \"io.containerd.krun.v2\"");
                println!(
                    "  [plugins.\"io.containerd.grpc.v1.cri\".containerd.runtimes.krun.options]"
                );
                println!("    BinaryName = \"containerd-shim-krun-v2\"");
                println!();
                println!("---");
                println!("# Kubernetes RuntimeClass resource\n");
                println!("apiVersion: node.k8s.io/v1");
                println!("kind: RuntimeClass");
                println!("metadata:");
                println!("  name: krun");
                println!("handler: krun");
                println!("overhead:");
                println!("  podFixed:");
                println!("    memory: \"64Mi\"");
                println!("    cpu: \"50m\"");
                println!("scheduling:");
                println!("  nodeSelector:");
                println!("    krun.io/enabled: \"true\"");
            }
            ContainerdCommands::Install { target } => {
                let shim_name = "containerd-shim-krun-v2";
                let current_exe = std::env::current_exe()
                    .context("Failed to determine current executable path")?;
                let exe_dir = current_exe.parent().unwrap_or_else(|| Path::new("."));
                let shim_src = exe_dir.join(shim_name);

                if !shim_src.exists() {
                    // Try target/release as fallback
                    let alt = PathBuf::from("target/release").join(shim_name);
                    if alt.exists() {
                        let dest = target.join(shim_name);
                        println!("Symlinking {} -> {}", alt.display(), dest.display());
                        if dest.exists() {
                            std::fs::remove_file(&dest)?;
                        }
                        std::os::unix::fs::symlink(std::fs::canonicalize(&alt)?, &dest)?;
                        println!("✓ Installed {} to {}", shim_name, dest.display());
                    } else {
                        eprintln!(
                            "Error: {} not found at {} or {}",
                            shim_name,
                            shim_src.display(),
                            alt.display()
                        );
                        eprintln!(
                            "Build first with: cargo build --release -p containerd-shim-krun"
                        );
                        std::process::exit(1);
                    }
                } else {
                    let dest = target.join(shim_name);
                    println!("Symlinking {} -> {}", shim_src.display(), dest.display());
                    if dest.exists() {
                        std::fs::remove_file(&dest)?;
                    }
                    std::os::unix::fs::symlink(std::fs::canonicalize(&shim_src)?, &dest)?;
                    println!("✓ Installed {} to {}", shim_name, dest.display());
                }
            }
            ContainerdCommands::Status => {
                // Check shim binary
                let shim_name = "containerd-shim-krun-v2";
                let shim_in_path = std::process::Command::new("which").arg(shim_name).output();
                match shim_in_path {
                    Ok(out) if out.status.success() => {
                        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        println!("✓ Shim binary found: {}", path);
                        // Try --version
                        if let Ok(ver) = std::process::Command::new(&path).arg("--version").output()
                        {
                            println!("  {}", String::from_utf8_lossy(&ver.stdout).trim());
                        }
                    }
                    _ => {
                        println!("✗ Shim binary '{}' not found in PATH", shim_name);
                        println!("  Run: microvm containerd install");
                    }
                }

                // Check containerd daemon
                let ctr_status = std::process::Command::new("ctr").args(["version"]).output();
                match ctr_status {
                    Ok(out) if out.status.success() => {
                        println!("✓ containerd daemon reachable");
                        for line in String::from_utf8_lossy(&out.stdout).lines() {
                            println!("  {}", line);
                        }
                    }
                    _ => {
                        println!("✗ containerd daemon not reachable (is it running?)");
                    }
                }
            }
        },

        Commands::Compose { command } => match command {
            ComposeCommands::Up {
                file,
                detach,
                services,
                data_dir,
            } => {
                let base_data_dir = data_dir.unwrap_or_else(default_data_dir);
                let compose_file = resolve_compose_file(file)?;
                let project = ComposeProject::load(&compose_file, Some(base_data_dir.clone()))?;
                handle_compose_up(
                    &project,
                    &compose_file,
                    &base_data_dir,
                    detach,
                    &services,
                )
                .await?;
            }
            ComposeCommands::Down {
                file,
                volumes,
                data_dir,
            } => {
                let base_data_dir = data_dir.unwrap_or_else(default_data_dir);
                let compose_file = resolve_compose_file(file)?;
                let project = ComposeProject::load(&compose_file, Some(base_data_dir.clone()))?;
                handle_compose_down(&project, &base_data_dir, volumes)?;
            }
            ComposeCommands::Ps { file, data_dir } => {
                let base_data_dir = data_dir.unwrap_or_else(default_data_dir);
                let compose_file = resolve_compose_file(file)?;
                let project = ComposeProject::load(&compose_file, Some(base_data_dir.clone()))?;
                handle_compose_ps(&project, &base_data_dir)?;
            }
            ComposeCommands::Logs {
                file,
                service,
                follow,
                data_dir,
            } => {
                let base_data_dir = data_dir.unwrap_or_else(default_data_dir);
                let compose_file = resolve_compose_file(file)?;
                let project = ComposeProject::load(&compose_file, Some(base_data_dir.clone()))?;
                handle_compose_logs(&project, &base_data_dir, service, follow).await?;
            }
            ComposeCommands::Stop {
                file,
                services,
                data_dir,
            } => {
                let base_data_dir = data_dir.unwrap_or_else(default_data_dir);
                let compose_file = resolve_compose_file(file)?;
                let project = ComposeProject::load(&compose_file, Some(base_data_dir.clone()))?;
                handle_compose_stop(&project, &base_data_dir, &services)?;
            }
            ComposeCommands::Start {
                file,
                services,
                data_dir,
            } => {
                let base_data_dir = data_dir.unwrap_or_else(default_data_dir);
                let compose_file = resolve_compose_file(file)?;
                let project = ComposeProject::load(&compose_file, Some(base_data_dir.clone()))?;
                handle_compose_start(&project, &base_data_dir, &services).await?;
            }
            ComposeCommands::Restart {
                file,
                services,
                data_dir,
            } => {
                let base_data_dir = data_dir.unwrap_or_else(default_data_dir);
                let compose_file = resolve_compose_file(file)?;
                let project = ComposeProject::load(&compose_file, Some(base_data_dir.clone()))?;
                handle_compose_restart(&project, &base_data_dir, &services).await?;
            }
            ComposeCommands::Config { file, data_dir } => {
                let base_data_dir = data_dir.unwrap_or_else(default_data_dir);
                let compose_file = resolve_compose_file(file)?;
                let project = ComposeProject::load(&compose_file, Some(base_data_dir))?;
                handle_compose_config(&project)?;
            }
        },

        Commands::Apply {
            file,
            detach,
            data_dir,
        } => {
            let base_data_dir = data_dir.unwrap_or_else(default_data_dir);
            if !file.exists() {
                bail!("Manifest file '{}' does not exist", file.display());
            }
            let project = ComposeProject::load(&file, Some(base_data_dir.clone()))?;
            handle_compose_up(&project, &file, &base_data_dir, detach, &[]).await?;
        }

        Commands::CherryPick {
            id,
            workspace,
            data_dir,
        } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            let abs_host_ws = std::fs::canonicalize(&workspace).unwrap_or(workspace);

            let vm = match StateManager::find(&base, &id)? {
                Some(v) => v,
                None => bail!("MicroVM instance '{}' not found in state", id),
            };

            let cow_ws = vm.instance_dir.join("workspaces").join("workspace");
            if !cow_ws.exists() {
                bail!(
                    "No Copy-on-Write workspace found for microVM '{}' at {}",
                    id,
                    cow_ws.display()
                );
            }

            let host_head = get_git_head(&abs_host_ws)
                .context("Host workspace is not a valid git repository (no HEAD found)")?;
            let sandbox_head = get_git_head(&cow_ws)
                .context("Sandbox workspace is not a valid git repository")?;

            if host_head == sandbox_head {
                println!(
                    "No new commits in sandbox '{}' compared to host HEAD ({})",
                    id,
                    &host_head[..8.min(host_head.len())]
                );
                return Ok(());
            }

            let commits = get_git_commits_between(&cow_ws, &host_head, &sandbox_head);
            println!(
                "🍒 Cherry-picking {} commit(s) from sandbox '{}' to host:",
                commits.len(),
                id
            );
            for c in &commits {
                println!("   • {c}");
            }

            cherry_pick_sandbox_to_host(&cow_ws, &abs_host_ws, &host_head, &sandbox_head)?;
            println!("✅ Successfully cherry-picked sandbox commits to host workspace!");
        }

        Commands::Network(net_cmd) => handle_network_command(net_cmd).await?,
    }

    Ok(())
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

async fn handle_network_command(cmd: NetworkCommands) -> Result<()> {
    match cmd {
        NetworkCommands::Ls { json, data_dir } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            let vms = StateManager::list(&base)?;
            let mut inspections = Vec::new();
            for vm in &vms {
                let is_alive = vm.is_process_alive();
                if let Ok(insp) = microvm_core::net::inspect_microvm_network(
                    &vm.instance_dir,
                    &vm.id,
                    Some(vm.pid),
                    is_alive,
                ) {
                    inspections.push(insp);
                }
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&inspections)?);
            } else if inspections.is_empty() {
                println!("No active microVM networks found.");
            } else {
                println!(
                    "{:<14} {:<12} {:<16} {:<16} {:<16} {:<18} {}",
                    "MICROVM ID", "MODE", "GUEST IP", "GATEWAY", "PORTS", "EGRESS RULES", "STATUS"
                );
                println!("{:-<100}", "");
                for item in inspections {
                    let ports_str = if item.port_forwards.is_empty() {
                        "-".to_string()
                    } else {
                        item.port_forwards
                            .iter()
                            .map(|p| format!("{}:{}", p.host, p.guest))
                            .collect::<Vec<_>>()
                            .join(",")
                    };
                    let egress_str = if item.allow_hosts.is_empty() {
                        if item.mode.starts_with("none") {
                            "Blocked (All)".to_string()
                        } else {
                            "Unrestricted".to_string()
                        }
                    } else {
                        item.allow_hosts.join(",")
                    };
                    let mode_short = if item.mode.starts_with("gvproxy") {
                        "gvproxy"
                    } else if item.mode.starts_with("tsi") {
                        "tsi"
                    } else if item.mode.starts_with("none") {
                        "none"
                    } else if item.mode.starts_with("cni") {
                        "cni"
                    } else {
                        "unix"
                    };

                    println!(
                        "{:<14} {:<12} {:<16} {:<16} {:<16} {:<18} {}",
                        truncate_str(&item.id, 13),
                        mode_short,
                        truncate_str(&item.guest_ip, 15),
                        truncate_str(&item.gateway_ip, 15),
                        truncate_str(&ports_str, 15),
                        truncate_str(&egress_str, 17),
                        item.status
                    );
                }
            }
        }

        NetworkCommands::Inspect { id, json, data_dir } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            let vm = match StateManager::find(&base, &id)? {
                Some(v) => v,
                None => bail!("MicroVM '{}' not found in state", id),
            };
            let is_alive = vm.is_process_alive();
            let inspection = microvm_core::net::inspect_microvm_network(
                &vm.instance_dir,
                &vm.id,
                Some(vm.pid),
                is_alive,
            )?;

            if json {
                println!("{}", serde_json::to_string_pretty(&inspection)?);
            } else {
                println!("🌐 MicroVM Network Topology & Security: {}", inspection.id);
                println!("{:-<65}", "");
                println!("  Status:              {}", inspection.status);
                println!("  Network Mode:        {}", inspection.mode);
                println!("  Guest IP Address:    {}", inspection.guest_ip);
                println!("  Virtual Gateway:     {}", inspection.gateway_ip);
                println!("  Virtual MAC:         {}", inspection.mac_address);
                println!("  Interface MTU:       {}", inspection.mtu);
                println!("  Guest Hostname:      {}", inspection.hostname);
                println!("  DNS Resolvers:       {}", inspection.dns_servers.join(", "));
                let pf_str = if inspection.port_forwards.is_empty() {
                    "None".to_string()
                } else {
                    inspection
                        .port_forwards
                        .iter()
                        .map(|pf| {
                            format!(
                                "0.0.0.0:{} -> {}:{}",
                                pf.host, inspection.guest_ip, pf.guest
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                println!("  Port Mappings:       {}", pf_str);
                let egress_str = if inspection.allow_hosts.is_empty() {
                    if inspection.mode.starts_with("none") {
                        "Default-Deny (Air-Gapped Isolation)"
                    } else {
                        "Unrestricted (Standard Egress)"
                    }
                } else {
                    &inspection.allow_hosts.join(", ")
                };
                println!("  Allowed Egress:      {}", egress_str);
                println!(
                    "  Metadata Defense:    {}",
                    if inspection.metadata_blocked {
                        "Active (169.254.169.254 exfiltration blocked)"
                    } else {
                        "Disabled"
                    }
                );
                println!(
                    "  In-Flight Secrets:   {}",
                    if inspection.secret_substitution_active {
                        "Active (Zero-Trust header/body substitution)"
                    } else {
                        "None configured"
                    }
                );
                if let Some(budget) = inspection.token_budget {
                    println!("  LLM Token Ceiling:   {} tokens", budget);
                }
                if let Some(port) = inspection.egress_proxy_port {
                    println!("  Egress Proxy Server: 127.0.0.1:{}", port);
                }
                if let Some(ref sock) = inspection.unix_socket_path {
                    println!("  Virtual Switch Sock: {}", sock);
                }
            }
        }

        NetworkCommands::Ports { id, json, data_dir } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            let vms = if let Some(target_id) = id {
                match StateManager::find(&base, &target_id)? {
                    Some(v) => vec![v],
                    None => bail!("MicroVM '{}' not found in state", target_id),
                }
            } else {
                StateManager::list(&base)?
            };

            #[derive(serde::Serialize)]
            struct PortEntry {
                host_port: u16,
                guest_port: u16,
                protocol: &'static str,
                microvm_id: String,
                status: String,
                endpoint: String,
            }

            let mut port_list = Vec::new();
            for vm in &vms {
                let is_alive = vm.is_process_alive();
                let status_str = if is_alive { "Running" } else { "Stopped" };
                for pf in &vm.port_forwards {
                    port_list.push(PortEntry {
                        host_port: pf.host,
                        guest_port: pf.guest,
                        protocol: "tcp",
                        microvm_id: vm.id.clone(),
                        status: status_str.to_string(),
                        endpoint: format!("http://localhost:{}", pf.host),
                    });
                }
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&port_list)?);
            } else if port_list.is_empty() {
                println!("No published port mappings found.");
            } else {
                println!(
                    "{:<12} {:<12} {:<10} {:<16} {:<12} {}",
                    "HOST PORT", "GUEST PORT", "PROTOCOL", "MICROVM ID", "STATUS", "LOCAL ENDPOINT"
                );
                println!("{:-<80}", "");
                for p in port_list {
                    println!(
                        "{:<12} {:<12} {:<10} {:<16} {:<12} {}",
                        p.host_port,
                        p.guest_port,
                        p.protocol,
                        truncate_str(&p.microvm_id, 15),
                        p.status,
                        p.endpoint
                    );
                }
            }
        }

        NetworkCommands::Test {
            id,
            target,
            data_dir,
        } => {
            let base = data_dir.unwrap_or_else(default_data_dir);
            let vm = match StateManager::find(&base, &id)? {
                Some(v) => v,
                None => bail!("MicroVM '{}' not found in state", id),
            };

            if !vm.is_process_alive() {
                bail!(
                    "Cannot test network on stopped microVM '{}'. Start the microVM first.",
                    id
                );
            }

            println!(
                "🔍 Probing network connectivity and egress security inside microVM '{}'...",
                id
            );
            println!("   Target Destination: {}", target);

            let test_cmd = format!(
                "echo '[DNS Test]' && (getent hosts {target} 2>&1 || nslookup {target} 2>&1 || echo 'DNS resolution not available') && echo '[Ping/TCP Probe]' && (ping -c 2 -W 2 {target} 2>&1 || nc -z -w 2 {target} 80 2>&1 || curl -I -s --connect-timeout 2 {target} 2>&1 || echo 'Target unreachable or filtered')"
            );

            let req = microvm_core::ExecRequest::new(vec![
                "sh".to_string(),
                "-c".to_string(),
                test_cmd,
            ]);
            let resp = StateManager::exec(&base, &id, &req).await?;

            if !resp.stdout.is_empty() {
                println!("\n{}", resp.stdout.trim());
            }
            if !resp.stderr.is_empty() {
                eprintln!("\nDiagnostics Stderr:\n{}", resp.stderr.trim());
            }
            if resp.exit_code == 0 {
                println!("\n✅ Network diagnostic probe completed successfully.");
            } else {
                println!("\n⚠️ Network diagnostic returned exit code {}.", resp.exit_code);
            }
        }
    }
    Ok(())
}

fn resolve_compose_file(file: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(f) = file {
        if !f.exists() {
            bail!("Compose file '{}' does not exist", f.display());
        }
        Ok(f)
    } else {
        let cur = std::env::current_dir().context("Failed to get current working directory")?;
        ComposeProject::discover_compose_file(&cur)
    }
}

async fn handle_compose_up(
    project: &ComposeProject,
    manifest_path: &Path,
    data_dir: &Path,
    detach: bool,
    services_filter: &[String],
) -> Result<()> {
    use std::io::Write;

    let all_order = project.resolve_launch_order()?;
    let to_launch: Vec<String> = if services_filter.is_empty() {
        all_order
    } else {
        let set: std::collections::HashSet<&str> =
            services_filter.iter().map(|s| s.as_str()).collect();
        for s in services_filter {
            if !project.spec.services.contains_key(s) {
                bail!("Service '{}' not found in compose manifest", s);
            }
        }
        all_order
            .into_iter()
            .filter(|s| set.contains(s.as_str()))
            .collect()
    };

    println!(
        "[+] Running {} microVM service(s) for project '{}':",
        to_launch.len(),
        project.name
    );

    let mut state = project
        .load_state()?
        .unwrap_or_else(|| ComposeProjectState {
            name: project.name.clone(),
            compose_file: manifest_path.to_path_buf(),
            working_dir: project.base_dir.clone(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            services: BTreeMap::new(),
        });

    let mut active_vms: Vec<microvm_core::MicroVm> = Vec::new();

    for service_name in &to_launch {
        let svc_spec = &project.spec.services[service_name];

        if let Some(prev) = state.services.get(service_name) {
            if let Ok(Some(existing_vm)) = StateManager::find(data_dir, &prev.instance_id) {
                if existing_vm.is_process_alive() {
                    println!(
                        " ✔ Service '{}' is already running (ID: {}, PID: {})",
                        service_name,
                        prev.instance_id,
                        prev.pid.unwrap_or(0)
                    );
                    continue;
                }
            }
        }

        print!(
            " ⏳ Starting service '{}' ({}) ...",
            service_name, svc_spec.image
        );
        let _ = std::io::stdout().flush();

        let builder = project.build_service_vm(service_name)?;
        let vm = builder
            .run()
            .await
            .with_context(|| format!("Failed to launch service '{service_name}'"))?;

        println!(
            "\r ✔ Service '{}' started  (ID: {}, PID: {:?})",
            service_name,
            vm.id(),
            vm.pid()
        );

        state.services.insert(
            service_name.clone(),
            ComposeServiceState {
                service_name: service_name.clone(),
                instance_id: vm.id().to_string(),
                pid: vm.pid(),
                image: svc_spec.image.clone(),
                status: "running".to_string(),
                ports: svc_spec.ports.clone(),
            },
        );
        project.save_state(&state)?;

        if !detach {
            active_vms.push(vm);
        }
    }

    if detach {
        println!(
            "\n✨ Compose project '{}' started in detached mode.",
            project.name
        );
        print_compose_ps_table(&state, data_dir);
        return Ok(());
    }

    println!("\nAttaching to compose project console output (Press Ctrl+C to stop)...");

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            eprintln!("\n⚠️ Received interrupt (Ctrl+C). Gracefully stopping compose project '{}'...", project.name);
            for mut vm in active_vms {
                let _ = vm.stop().await;
            }
            for (_, svc_state) in state.services.iter_mut() {
                svc_state.status = "stopped".to_string();
            }
            let _ = project.save_state(&state);
            println!("🛑 All microVM services stopped.");
        }
    }

    Ok(())
}

fn handle_compose_down(
    project: &ComposeProject,
    data_dir: &Path,
    remove_volumes: bool,
) -> Result<()> {
    use std::io::Write;

    println!(
        "[+] Stopping and removing microVM services for project '{}':",
        project.name
    );
    if let Some(state) = project.load_state()? {
        let mut rev_services: Vec<_> = state.services.values().cloned().collect();
        rev_services.reverse();
        for svc in rev_services {
            print!(
                " ⏳ Stopping service '{}' (ID: {})...",
                svc.service_name, svc.instance_id
            );
            let _ = std::io::stdout().flush();
            let _ = StateManager::stop(data_dir, &svc.instance_id);
            let _ = StateManager::delete(data_dir, &svc.instance_id, true);
            println!("\r ✔ Service '{}' stopped and removed", svc.service_name);
        }
        let _ = project.remove_state();
    } else {
        println!("No active state found for project '{}'.", project.name);
    }

    if remove_volumes {
        println!(" ✔ Volumes cleaned up");
    }

    println!("✨ Compose project '{}' is down.", project.name);
    Ok(())
}

fn handle_compose_ps(project: &ComposeProject, data_dir: &Path) -> Result<()> {
    let state = project.load_state()?;
    println!("Project: {}", project.name);
    match state {
        Some(ref s) => print_compose_ps_table(s, data_dir),
        None => println!(
            "No active compose state found for project '{}'.",
            project.name
        ),
    }
    Ok(())
}

fn print_compose_ps_table(state: &ComposeProjectState, data_dir: &Path) {
    if state.services.is_empty() {
        println!("No services declared or running.");
        return;
    }

    println!(
        "{:<18} {:<18} {:<18} {:<24} {:<20}",
        "SERVICE", "CONTAINER ID", "STATUS", "IMAGE", "PORTS"
    );
    println!("{:-<100}", "");
    for (name, svc) in &state.services {
        let is_alive = if let Ok(Some(vm)) = StateManager::find(data_dir, &svc.instance_id) {
            vm.is_process_alive()
        } else {
            false
        };
        let status_str = if is_alive {
            format!("Up (PID {})", svc.pid.unwrap_or(0))
        } else {
            "Exited".to_string()
        };
        let ports_str = if svc.ports.is_empty() {
            "-".to_string()
        } else {
            svc.ports.join(", ")
        };
        let trunc_id = if svc.instance_id.len() > 14 {
            &svc.instance_id[..14]
        } else {
            &svc.instance_id
        };
        println!(
            "{:<18} {:<18} {:<18} {:<24} {:<20}",
            name, trunc_id, status_str, svc.image, ports_str
        );
    }
}

async fn handle_compose_logs(
    project: &ComposeProject,
    data_dir: &Path,
    service_filter: Option<String>,
    follow: bool,
) -> Result<()> {
    use std::io::{Read, Write};

    let state = match project.load_state()? {
        Some(s) => s,
        None => bail!(
            "No state found for compose project '{}'. Is it running?",
            project.name
        ),
    };

    if let Some(target_svc) = service_filter {
        let svc_state = state.services.get(&target_svc).with_context(|| {
            format!(
                "Service '{target_svc}' not found in project '{}'",
                project.name
            )
        })?;

        let vm = StateManager::find(data_dir, &svc_state.instance_id)?.with_context(|| {
            format!(
                "Instance '{}' for service '{target_svc}' not found",
                svc_state.instance_id
            )
        })?;

        let log_file = vm.instance_dir.join("console.log");
        if !log_file.exists() && !follow {
            println!("No log output for service '{}'", target_svc);
            return Ok(());
        }

        let mut file = match std::fs::File::open(&log_file) {
            Ok(f) => f,
            Err(_) if follow => {
                let start = std::time::Instant::now();
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if let Ok(f) = std::fs::File::open(&log_file) {
                        break f;
                    }
                    if start.elapsed() > std::time::Duration::from_secs(5) {
                        bail!(
                            "Log file '{}' was not created after 5 seconds",
                            log_file.display()
                        );
                    }
                }
            }
            Err(e) => bail!("Failed to open log file '{}': {}", log_file.display(), e),
        };

        let mut buf = Vec::new();
        let _ = file.read_to_end(&mut buf);
        print!("{}", String::from_utf8_lossy(&buf));
        let _ = std::io::stdout().flush();

        if follow {
            let mut pos = buf.len() as u64;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                if let Ok(meta) = std::fs::metadata(&log_file) {
                    if meta.len() > pos {
                        use std::io::Seek;
                        if let Ok(mut f) = std::fs::File::open(&log_file) {
                            if f.seek(std::io::SeekFrom::Start(pos)).is_ok() {
                                let mut new_buf = Vec::new();
                                if f.read_to_end(&mut new_buf).is_ok() {
                                    pos = meta.len();
                                    print!("{}", String::from_utf8_lossy(&new_buf));
                                    let _ = std::io::stdout().flush();
                                }
                            }
                        }
                    }
                }
            }
        }
    } else {
        for (name, svc_state) in &state.services {
            if let Ok(Some(vm)) = StateManager::find(data_dir, &svc_state.instance_id) {
                let log_file = vm.instance_dir.join("console.log");
                if let Ok(mut f) = std::fs::File::open(&log_file) {
                    let mut content = String::new();
                    let _ = f.read_to_string(&mut content);
                    for line in content.lines() {
                        println!("{:<12} | {}", name, line);
                    }
                }
            }
        }

        if follow {
            println!("Streaming logs (Press Ctrl+C to exit)...");
            let mut file_positions: HashMap<String, u64> = HashMap::new();
            for (name, svc_state) in &state.services {
                if let Ok(Some(vm)) = StateManager::find(data_dir, &svc_state.instance_id) {
                    let log_file = vm.instance_dir.join("console.log");
                    if let Ok(meta) = std::fs::metadata(&log_file) {
                        file_positions.insert(name.clone(), meta.len());
                    }
                }
            }

            loop {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                for (name, svc_state) in &state.services {
                    if let Ok(Some(vm)) = StateManager::find(data_dir, &svc_state.instance_id) {
                        let log_file = vm.instance_dir.join("console.log");
                        let pos = file_positions.get(name).copied().unwrap_or(0);
                        if let Ok(meta) = std::fs::metadata(&log_file) {
                            if meta.len() > pos {
                                use std::io::Seek;
                                if let Ok(mut f) = std::fs::File::open(&log_file) {
                                    if f.seek(std::io::SeekFrom::Start(pos)).is_ok() {
                                        let mut new_content = String::new();
                                        if f.read_to_string(&mut new_content).is_ok() {
                                            file_positions.insert(name.clone(), meta.len());
                                            for line in new_content.lines() {
                                                println!("{:<12} | {}", name, line);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn handle_compose_stop(
    project: &ComposeProject,
    data_dir: &Path,
    services_filter: &[String],
) -> Result<()> {
    use std::io::Write;

    let mut state = match project.load_state()? {
        Some(s) => s,
        None => {
            println!("No active state found for project '{}'.", project.name);
            return Ok(());
        }
    };

    println!("[+] Stopping services for project '{}':", project.name);
    for (name, svc) in state.services.iter_mut() {
        if !services_filter.is_empty() && !services_filter.contains(name) {
            continue;
        }
        print!(
            " ⏳ Stopping service '{}' (ID: {})...",
            name, svc.instance_id
        );
        let _ = std::io::stdout().flush();
        let _ = StateManager::stop(data_dir, &svc.instance_id);
        svc.status = "stopped".to_string();
        println!("\r ✔ Service '{}' stopped", name);
    }
    project.save_state(&state)?;
    Ok(())
}

async fn handle_compose_start(
    project: &ComposeProject,
    data_dir: &Path,
    services_filter: &[String],
) -> Result<()> {
    use std::io::Write;

    let mut state = project
        .load_state()?
        .unwrap_or_else(|| ComposeProjectState {
            name: project.name.clone(),
            compose_file: project.base_dir.join("krun-compose.yaml"),
            working_dir: project.base_dir.clone(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            services: BTreeMap::new(),
        });

    let launch_order = project.resolve_launch_order()?;
    let to_start: Vec<String> = if services_filter.is_empty() {
        launch_order
    } else {
        launch_order
            .into_iter()
            .filter(|s| services_filter.contains(s))
            .collect()
    };

    println!("[+] Starting services for project '{}':", project.name);
    for name in &to_start {
        if let Some(svc) = state.services.get(name) {
            if let Ok(Some(existing_vm)) = StateManager::find(data_dir, &svc.instance_id) {
                if existing_vm.is_process_alive() {
                    println!(" ✔ Service '{}' is already running", name);
                    continue;
                }
            }
        }

        print!(" ⏳ Starting service '{}'...", name);
        let _ = std::io::stdout().flush();
        let builder = project.build_service_vm(name)?;
        let vm = builder
            .run()
            .await
            .with_context(|| format!("Failed to start service '{name}'"))?;
        let svc_spec = &project.spec.services[name];

        state.services.insert(
            name.clone(),
            ComposeServiceState {
                service_name: name.clone(),
                instance_id: vm.id().to_string(),
                pid: vm.pid(),
                image: svc_spec.image.clone(),
                status: "running".to_string(),
                ports: svc_spec.ports.clone(),
            },
        );
        println!(
            "\r ✔ Service '{}' started  (ID: {}, PID: {:?})",
            name,
            vm.id(),
            vm.pid()
        );
    }
    project.save_state(&state)?;
    Ok(())
}

async fn handle_compose_restart(
    project: &ComposeProject,
    data_dir: &Path,
    services_filter: &[String],
) -> Result<()> {
    println!("[+] Restarting services for project '{}':", project.name);
    handle_compose_stop(project, data_dir, services_filter)?;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    handle_compose_start(project, data_dir, services_filter).await?;
    Ok(())
}

fn handle_compose_config(project: &ComposeProject) -> Result<()> {
    let yaml = serde_yaml::to_string(&project.spec)
        .context("Failed to serialize resolved compose configuration to YAML")?;
    println!("{yaml}");
    Ok(())
}

fn get_git_head(repo_path: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

fn get_git_commits_between(repo_path: &Path, base: &str, head: &str) -> Vec<String> {
    let out = std::process::Command::new("git")
        .args(["log", "--oneline", &format!("{base}..{head}")])
        .current_dir(repo_path)
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|s| s.to_string())
            .collect(),
        _ => Vec::new(),
    }
}

fn cherry_pick_sandbox_to_host(
    sandbox_ws: &Path,
    host_ws: &Path,
    base_commit: &str,
    target_commit: &str,
) -> Result<()> {
    let patch_output = std::process::Command::new("git")
        .args([
            "format-patch",
            &format!("{base_commit}..{target_commit}"),
            "--stdout",
        ])
        .current_dir(sandbox_ws)
        .output()
        .context("Failed to format patch from sandbox git repository")?;

    if !patch_output.status.success() {
        bail!(
            "Failed to format patch from sandbox: {}",
            String::from_utf8_lossy(&patch_output.stderr)
        );
    }

    if patch_output.stdout.is_empty() {
        println!("No git commits to cherry-pick.");
        return Ok(());
    }

    use std::io::Write;
    let mut child = std::process::Command::new("git")
        .args(["am", "--3way"])
        .current_dir(host_ws)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Failed to spawn 'git am' on host repository")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&patch_output.stdout)?;
    }

    let result = child.wait_with_output()?;
    if !result.status.success() {
        bail!(
            "git am failed: {}\n(You may run 'git am --abort' to reset the host repository state)",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    Ok(())
}

async fn start_metrics_server(data_dir: &Path, addr: &str, format_json: bool) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("Failed to bind metrics HTTP server to {}", addr))?;
    println!(
        "📊 MicroVM Prometheus Metrics Exporter listening on http://{}/metrics",
        addr
    );
    println!("   Press Ctrl+C to stop.");

    let data_dir = data_dir.to_path_buf();
    loop {
        tokio::select! {
            accept_res = listener.accept() => {
                let (mut socket, peer) = accept_res?;
                tracing::debug!("Metrics scrape connection from {}", peer);
                let vms = StateManager::list(&data_dir).unwrap_or_default();
                let (content_type, body) = if format_json {
                    let summary = json!({
                        "total": vms.len(),
                        "running": vms.iter().filter(|v| v.status == VmStatus::Running).count(),
                        "paused": vms.iter().filter(|v| v.status == VmStatus::Paused).count(),
                        "stopped": vms.iter().filter(|v| v.status == VmStatus::Stopped).count(),
                        "vms": vms,
                    });
                    ("application/json", serde_json::to_string_pretty(&summary).unwrap_or_default())
                } else {
                    ("text/plain; version=0.0.4; charset=utf-8", microvm_core::metrics::export_prometheus_metrics(&vms))
                };

                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    content_type,
                    body.len(),
                    body
                );

                tokio::spawn(async move {
                    use tokio::io::AsyncWriteExt;
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
            _ = tokio::signal::ctrl_c() => {
                println!("\nShutting down metrics exporter...");
                break;
            }
        }
    }
    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_duration_since(timestamp: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now < timestamp {
        return "just now".to_string();
    }
    let elapsed = now - timestamp;
    if elapsed < 60 {
        format!("{}s ago", elapsed)
    } else if elapsed < 3600 {
        format!("{}m ago", elapsed / 60)
    } else if elapsed < 86400 {
        format!("{}h ago", elapsed / 3600)
    } else {
        format!("{}d ago", elapsed / 86400)
    }
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(meta) = p.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

fn host_memory_bytes() -> Option<u64> {
    let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if pages > 0 && page_size > 0 {
        Some((pages as u64).saturating_mul(page_size as u64))
    } else {
        None
    }
}

/// Parses a `--disk` CLI argument into (id, path, read_only).
///
/// Accepted formats:
///   `/path/to/disk.raw`                  → auto-ID, rw
///   `/path/to/disk.raw:ro`               → auto-ID, ro
///   `myid:/path/to/disk.raw`             → explicit ID, rw
///   `myid:/path/to/disk.raw:ro`          → explicit ID, ro
///
/// `disk_index` is used to generate unique auto-IDs (`disk0`, `disk1`, ...).
fn parse_disk_arg(arg: &str, disk_index: usize) -> (String, PathBuf, bool) {
    let parts: Vec<&str> = arg.split(':').collect();
    match parts.len() {
        1 => (format!("disk{disk_index}"), PathBuf::from(parts[0]), false),
        2 => {
            if parts[1] == "ro" {
                (format!("disk{disk_index}"), PathBuf::from(parts[0]), true)
            } else if parts[1] == "rw" {
                (format!("disk{disk_index}"), PathBuf::from(parts[0]), false)
            } else {
                (parts[0].to_string(), PathBuf::from(parts[1]), false)
            }
        }
        _ => (
            parts[0].to_string(),
            PathBuf::from(parts[1]),
            parts[2] == "ro",
        ),
    }
}

fn parse_secret_arg(arg: &str) -> Result<(String, String)> {
    let (key, value_spec) = arg.split_once('=').ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid secret format '{}'. Expected KEY=VALUE, KEY=env:VAR_NAME, or KEY=file:/path",
            arg
        )
    })?;

    let key = key.trim();
    if key.is_empty() {
        bail!("Secret key cannot be empty in '{}'", arg);
    }

    let value = if let Some(env_var) = value_spec.strip_prefix("env:") {
        std::env::var(env_var).map_err(|_| {
            anyhow::anyhow!(
                "Environment variable '{}' for secret '{}' is not set",
                env_var,
                key
            )
        })?
    } else if let Some(file_path) = value_spec.strip_prefix("file:") {
        std::fs::read_to_string(file_path)
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to read secret file '{}' for secret '{}': {}",
                    file_path,
                    key,
                    e
                )
            })?
            .trim_end_matches(['\r', '\n'])
            .to_string()
    } else {
        value_spec.to_string()
    };

    Ok((key.to_string(), value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parse_run_artifact_and_cow() {
        let args = vec![
            "microvm",
            "run",
            "--artifact",
            "ghcr.io/owner/model:v1:weights:ro",
            "--workspace-cow",
            "/tmp/test-project:workspace",
            "alpine:latest",
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Commands::Run(run) => {
                assert_eq!(run.artifacts, vec!["ghcr.io/owner/model:v1:weights:ro"]);
                assert_eq!(run.workspace_cow, vec!["/tmp/test-project:workspace"]);
                assert_eq!(run.image, "alpine:latest");
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_cli_parse_egress_and_secrets() {
        let args = vec![
            "microvm",
            "run",
            "--allow-host",
            "api.openai.com:443",
            "--allow-host",
            "*.anthropic.com:443",
            "--secret",
            "OPENAI_API_KEY=sk-test-12345",
            "--max-tokens",
            "50000",
            "python:3.11-slim",
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Commands::Run(run) => {
                assert_eq!(
                    run.allow_hosts,
                    vec!["api.openai.com:443", "*.anthropic.com:443"]
                );
                assert_eq!(run.secrets, vec!["OPENAI_API_KEY=sk-test-12345"]);
                assert_eq!(run.max_tokens, Some(50000));
                assert_eq!(run.image, "python:3.11-slim");
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_parse_secret_arg() {
        // Direct value
        let (k, v) = parse_secret_arg("FOO=bar").unwrap();
        assert_eq!(k, "FOO");
        assert_eq!(v, "bar");

        // Environment variable
        std::env::set_var("TEST_SECRET_ENV_VAR", "super-secret-token");
        let (k2, v2) = parse_secret_arg("API_KEY=env:TEST_SECRET_ENV_VAR").unwrap();
        assert_eq!(k2, "API_KEY");
        assert_eq!(v2, "super-secret-token");

        // Missing env var
        assert!(parse_secret_arg("API_KEY=env:NON_EXISTENT_VAR_123456").is_err());

        // File-based
        let temp_dir = tempfile::tempdir().unwrap();
        let secret_file = temp_dir.path().join("secret.txt");
        std::fs::write(&secret_file, "file-token-content\n").unwrap();
        let arg = format!("FILE_KEY=file:{}", secret_file.display());
        let (k3, v3) = parse_secret_arg(&arg).unwrap();
        assert_eq!(k3, "FILE_KEY");
        assert_eq!(v3, "file-token-content");

        // Invalid format
        assert!(parse_secret_arg("NO_EQUALS_SIGN").is_err());
        assert!(parse_secret_arg("=NO_KEY").is_err());
    }

    #[test]
    fn test_cli_parse_artifact_commands() {
        let pull_args = vec!["microvm", "artifact", "pull", "ghcr.io/owner/weights:v1"];
        let cli = Cli::try_parse_from(pull_args).unwrap();
        match cli.command {
            Commands::Artifact {
                command: ArtifactCommands::Pull { artifact, .. },
            } => {
                assert_eq!(artifact, "ghcr.io/owner/weights:v1");
            }
            _ => panic!("Expected ArtifactCommands::Pull"),
        }

        let list_args = vec!["microvm", "artifact", "list"];
        let cli = Cli::try_parse_from(list_args).unwrap();
        match cli.command {
            Commands::Artifact {
                command: ArtifactCommands::List { .. },
            } => {}
            _ => panic!("Expected ArtifactCommands::List"),
        }
    }

    #[test]
    fn test_cli_parse_stats_and_inspect() {
        let stats_args = vec!["microvm", "stats", "--no-stream", "--json"];
        let cli = Cli::try_parse_from(stats_args).unwrap();
        match cli.command {
            Commands::Stats {
                id,
                no_stream,
                json,
                ..
            } => {
                assert!(id.is_none());
                assert!(no_stream);
                assert!(json);
            }
            _ => panic!("Expected Commands::Stats"),
        }

        let inspect_args = vec!["microvm", "inspect", "test-vm-123", "--json"];
        let cli = Cli::try_parse_from(inspect_args).unwrap();
        match cli.command {
            Commands::Inspect { id, json, .. } => {
                assert_eq!(id, "test-vm-123");
                assert!(json);
            }
            _ => panic!("Expected Commands::Inspect"),
        }
    }

    #[test]
    fn test_cli_parse_rm_and_top() {
        let rm_args = vec!["microvm", "rm", "-f", "test-vm-123"];
        let cli = Cli::try_parse_from(rm_args).unwrap();
        match cli.command {
            Commands::Rm { id, force, .. } => {
                assert_eq!(id, "test-vm-123");
                assert!(force);
            }
            _ => panic!("Expected Commands::Rm"),
        }

        let top_args = vec!["microvm", "top", "test-vm-123"];
        let cli = Cli::try_parse_from(top_args).unwrap();
        match cli.command {
            Commands::Top { id, .. } => {
                assert_eq!(id, "test-vm-123");
            }
            _ => panic!("Expected Commands::Top"),
        }
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1024 * 1024 * 10), "10.0 MiB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 * 2), "2.00 GiB");
    }

    #[test]
    fn test_cli_parse_pause_and_resume() {
        let pause_args = vec!["microvm", "pause", "vm-1234"];
        let cli = Cli::try_parse_from(pause_args).unwrap();
        match cli.command {
            Commands::Pause { id, .. } => {
                assert_eq!(id, "vm-1234");
            }
            _ => panic!("Expected Commands::Pause"),
        }

        let resume_args = vec!["microvm", "resume", "vm-1234"];
        let cli = Cli::try_parse_from(resume_args).unwrap();
        match cli.command {
            Commands::Resume { id, .. } => {
                assert_eq!(id, "vm-1234");
            }
            _ => panic!("Expected Commands::Resume"),
        }
    }

    #[test]
    fn test_cli_parse_run_dax() {
        let run_args = vec!["microvm", "run", "--dax", "4G", "alpine:latest"];
        let cli = Cli::try_parse_from(run_args).unwrap();
        match cli.command {
            Commands::Run(run) => {
                assert_eq!(run.image, "alpine:latest");
                assert_eq!(run.dax.as_deref(), Some("4G"));
            }
            _ => panic!("Expected Commands::Run with --dax"),
        }
    }

    #[test]
    fn test_cli_parse_run_lazy_load() {
        let run_args = vec![
            "microvm",
            "run",
            "--lazy-load",
            "--nydus-bootstrap",
            "/tmp/bootstrap.rafs",
            "--nydus-cache",
            "/var/cache/nydus",
            "--chunk-size",
            "64M",
            "--dax",
            "2G",
            "alpine:latest",
        ];
        let cli = Cli::try_parse_from(run_args).unwrap();
        match cli.command {
            Commands::Run(run) => {
                assert!(run.lazy_load);
                assert_eq!(
                    run.nydus_bootstrap,
                    Some(PathBuf::from("/tmp/bootstrap.rafs"))
                );
                assert_eq!(run.nydus_cache, Some(PathBuf::from("/var/cache/nydus")));
                assert_eq!(run.chunk_size.as_deref(), Some("64M"));
                assert_eq!(run.dax.as_deref(), Some("2G"));
                assert_eq!(run.image, "alpine:latest");
            }
            _ => panic!("Expected Commands::Run with --lazy-load"),
        }
    }

    #[test]
    fn test_cli_parse_run_gpu() {
        let run_args = vec![
            "microvm",
            "run",
            "--gpu",
            "--gpu-shm-size",
            "4G",
            "ghcr.io/ericlbuehler/mistral.rs:latest",
        ];
        let cli = Cli::try_parse_from(run_args).unwrap();
        match cli.command {
            Commands::Run(run) => {
                assert!(run.gpu);
                assert_eq!(run.gpu_shm_size.as_deref(), Some("4G"));
                assert_eq!(run.image, "ghcr.io/ericlbuehler/mistral.rs:latest");
            }
            _ => panic!("Expected Commands::Run with --gpu"),
        }
    }

    #[test]
    fn test_cli_parse_exec() {
        let exec_args = vec![
            "microvm",
            "exec",
            "-t",
            "-w",
            "/app",
            "-e",
            "PORT=9000",
            "vm-12345",
            "uname",
            "-a",
        ];
        let cli = Cli::try_parse_from(exec_args).unwrap();
        match cli.command {
            Commands::Exec {
                id,
                env,
                workdir,
                tty,
                cmd,
                ..
            } => {
                assert_eq!(id, "vm-12345");
                assert_eq!(env, vec!["PORT=9000"]);
                assert_eq!(workdir.as_deref(), Some("/app"));
                assert!(tty);
                assert_eq!(cmd, vec!["uname", "-a"]);
            }
            _ => panic!("Expected Commands::Exec"),
        }
    }

    #[test]
    fn test_cli_parse_snapshot_and_restore() {
        let snap_args = vec![
            "microvm",
            "snapshot",
            "vm-12345",
            "--output",
            "/tmp/snap.tar",
        ];
        let cli = Cli::try_parse_from(snap_args).unwrap();
        match cli.command {
            Commands::Snapshot { id, output, .. } => {
                assert_eq!(id, "vm-12345");
                assert_eq!(output, Some(PathBuf::from("/tmp/snap.tar")));
            }
            _ => panic!("Expected Commands::Snapshot"),
        }

        let restore_args = vec![
            "microvm",
            "restore",
            "/tmp/snap.tar",
            "--name",
            "vm-restored-99",
        ];
        let cli = Cli::try_parse_from(restore_args).unwrap();
        match cli.command {
            Commands::Restore { snapshot, name, .. } => {
                assert_eq!(snapshot, PathBuf::from("/tmp/snap.tar"));
                assert_eq!(name.as_deref(), Some("vm-restored-99"));
            }
            _ => panic!("Expected Commands::Restore"),
        }
    }

    #[test]
    fn test_cli_parse_run_no_sandbox() {
        let args = vec!["microvm", "run", "--no-sandbox", "alpine:latest"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Commands::Run(run) => {
                assert!(run.no_sandbox);
                assert_eq!(run.image, "alpine:latest");
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_cli_parse_resize() {
        let args = vec![
            "microvm",
            "resize",
            "vm-test-1",
            "--memory",
            "1024",
            "--cpus",
            "4",
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Commands::Resize {
                id, memory, cpus, ..
            } => {
                assert_eq!(id, "vm-test-1");
                assert_eq!(memory, Some(1024));
                assert_eq!(cpus, Some(4));
            }
            _ => panic!("Expected Commands::Resize"),
        }
    }

    #[test]
    fn test_cli_parse_metrics() {
        let args = vec!["microvm", "metrics", "--listen", "127.0.0.1:9090"];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Commands::Metrics { listen, json, .. } => {
                assert_eq!(listen.as_deref(), Some("127.0.0.1:9090"));
                assert!(!json);
            }
            _ => panic!("Expected Commands::Metrics"),
        }

        let json_args = vec!["microvm", "metrics", "--json"];
        let cli = Cli::try_parse_from(json_args).unwrap();
        match cli.command {
            Commands::Metrics { listen, json, .. } => {
                assert!(listen.is_none());
                assert!(json);
            }
            _ => panic!("Expected Commands::Metrics"),
        }
    }

    #[test]
    fn test_cli_parse_multi_boot_run() {
        let args = vec![
            "microvm",
            "run",
            "--kernel",
            "/boot/vmlinuz",
            "--kernel-format",
            "gz",
            "--initrd",
            "/boot/initrd.img",
            "--cmdline",
            "console=ttyS0 root=/dev/vda",
            "--disk",
            "rootfs.raw",
            "--disk",
            "data:data.raw:ro",
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Commands::Run(run) => {
                assert_eq!(run.kernel, Some(PathBuf::from("/boot/vmlinuz")));
                assert_eq!(run.kernel_format, Some("gz".to_string()));
                assert_eq!(run.initrd, Some(PathBuf::from("/boot/initrd.img")));
                assert_eq!(run.cmdline, Some("console=ttyS0 root=/dev/vda".to_string()));
                assert_eq!(
                    run.disks,
                    vec!["rootfs.raw".to_string(), "data:data.raw:ro".to_string()]
                );
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_cli_parse_asahi_m1n1_kernel_run() {
        let args = vec![
            "microvm",
            "run",
            "--kernel",
            "/opt/asahi/m1n1.bin",
            "--kformat",
            "raw",
            "--cmdline",
            "console=ttyAMA0 earlycon",
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Commands::Run(run) => {
                assert_eq!(run.kernel, Some(PathBuf::from("/opt/asahi/m1n1.bin")));
                assert_eq!(run.kernel_format, Some("raw".to_string()));
                assert_eq!(run.cmdline, Some("console=ttyAMA0 earlycon".to_string()));
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_cli_parse_unikernel_and_sandbox() {
        let unikernel_args = vec![
            "microvm",
            "unikernel",
            "app.unikraft",
            "-c",
            "1",
            "-m",
            "256",
            "--cmdline",
            "netdev.ipv4_addr=192.168.1.2",
            "--disk",
            "data.img",
        ];
        let cli = Cli::try_parse_from(unikernel_args).unwrap();
        match cli.command {
            Commands::Unikernel {
                kernel,
                cpus,
                memory,
                cmdline,
                disks,
                ..
            } => {
                assert_eq!(kernel, PathBuf::from("app.unikraft"));
                assert_eq!(cpus, 1);
                assert_eq!(memory, 256);
                assert_eq!(cmdline, Some("netdev.ipv4_addr=192.168.1.2".to_string()));
                assert_eq!(disks, vec!["data.img".to_string()]);
            }
            _ => panic!("Expected Commands::Unikernel"),
        }

        // Test with --params alias
        let unikernel_params_args = vec![
            "microvm",
            "unikernel",
            "app.unikraft",
            "--params",
            "console=ttyS0",
        ];
        let cli_unik = Cli::try_parse_from(unikernel_params_args).unwrap();
        match cli_unik.command {
            Commands::Unikernel { cmdline, .. } => {
                assert_eq!(cmdline, Some("console=ttyS0".to_string()));
            }
            _ => panic!("Expected Commands::Unikernel"),
        }

        let sandbox_args = vec![
            "microvm",
            "sandbox",
            "claude",
            "--workspace",
            ".",
            "-c",
            "4",
            "-m",
            "2048",
            "--secret",
            "CUSTOM_KEY=secretval",
            "--repo",
            "https://github.com/example/project.git",
        ];
        let cli2 = Cli::try_parse_from(sandbox_args).unwrap();
        match cli2.command {
            Commands::Sandbox {
                agent,
                workspace,
                repo,
                cpus,
                memory,
                secrets,
                ..
            } => {
                assert_eq!(agent, "claude");
                assert_eq!(workspace, PathBuf::from("."));
                assert_eq!(
                    repo,
                    Some("https://github.com/example/project.git".to_string())
                );
                assert_eq!(cpus, 4);
                assert_eq!(memory, 2048);
                assert_eq!(secrets, vec!["CUSTOM_KEY=secretval".to_string()]);
            }
            _ => panic!("Expected Commands::Sandbox"),
        }
    }

    #[test]
    fn test_cli_parse_compose_up_and_down() {
        let up_args = vec![
            "microvm",
            "compose",
            "up",
            "-f",
            "krun-compose.yaml",
            "-d",
            "web",
            "redis",
        ];
        let cli = Cli::try_parse_from(up_args).unwrap();
        match cli.command {
            Commands::Compose {
                command:
                    ComposeCommands::Up {
                        file,
                        detach,
                        services,
                        ..
                    },
            } => {
                assert_eq!(file, Some(PathBuf::from("krun-compose.yaml")));
                assert!(detach);
                assert_eq!(services, vec!["web".to_string(), "redis".to_string()]);
            }
            _ => panic!("Expected ComposeCommands::Up"),
        }

        let down_args = vec!["microvm", "compose", "down", "-v"];
        let cli_down = Cli::try_parse_from(down_args).unwrap();
        match cli_down.command {
            Commands::Compose {
                command: ComposeCommands::Down { file, volumes, .. },
            } => {
                assert!(file.is_none());
                assert!(volumes);
            }
            _ => panic!("Expected ComposeCommands::Down"),
        }
    }

    #[test]
    fn test_cli_parse_compose_ps_and_logs() {
        let ps_args = vec!["microvm", "compose", "ps", "-f", "compose.yaml"];
        let cli_ps = Cli::try_parse_from(ps_args).unwrap();
        match cli_ps.command {
            Commands::Compose {
                command: ComposeCommands::Ps { file, .. },
            } => {
                assert_eq!(file, Some(PathBuf::from("compose.yaml")));
            }
            _ => panic!("Expected ComposeCommands::Ps"),
        }

        let logs_args = vec![
            "microvm",
            "compose",
            "logs",
            "--file",
            "compose.yaml",
            "-f",
            "api",
        ];
        let cli_logs = Cli::try_parse_from(logs_args).unwrap();
        match cli_logs.command {
            Commands::Compose {
                command:
                    ComposeCommands::Logs {
                        file,
                        service,
                        follow,
                        ..
                    },
            } => {
                assert_eq!(file, Some(PathBuf::from("compose.yaml")));
                assert_eq!(service, Some("api".to_string()));
                assert!(follow);
            }
            _ => panic!("Expected ComposeCommands::Logs"),
        }
    }

    #[test]
    fn test_cli_parse_compose_restart_and_config() {
        let restart_args = vec!["microvm", "compose", "restart", "web"];
        let cli_res = Cli::try_parse_from(restart_args).unwrap();
        match cli_res.command {
            Commands::Compose {
                command: ComposeCommands::Restart { services, .. },
            } => {
                assert_eq!(services, vec!["web".to_string()]);
            }
            _ => panic!("Expected ComposeCommands::Restart"),
        }

        let config_args = vec!["microvm", "compose", "config", "-f", "krun-compose.yaml"];
        let cli_cfg = Cli::try_parse_from(config_args).unwrap();
        match cli_cfg.command {
            Commands::Compose {
                command: ComposeCommands::Config { file, .. },
            } => {
                assert_eq!(file, Some(PathBuf::from("krun-compose.yaml")));
            }
            _ => panic!("Expected ComposeCommands::Config"),
        }
    }

    #[test]
    fn test_cli_parse_apply_and_run_file() {
        let apply_args = vec![
            "microvm",
            "apply",
            "-f",
            "examples/microvm.yaml",
            "-d",
        ];
        let cli = Cli::try_parse_from(apply_args).unwrap();
        match cli.command {
            Commands::Apply { file, detach, .. } => {
                assert_eq!(file, PathBuf::from("examples/microvm.yaml"));
                assert!(detach);
            }
            _ => panic!("Expected Commands::Apply"),
        }

        let run_args = vec!["microvm", "run", "-f", "examples/krun-compose.yaml", "-d"];
        let cli_run = Cli::try_parse_from(run_args).unwrap();
        match cli_run.command {
            Commands::Run(run) => {
                assert_eq!(run.file, Some(PathBuf::from("examples/krun-compose.yaml")));
                assert!(run.detach);
            }
            _ => panic!("Expected Commands::Run with -f"),
        }
    }

    #[test]
    fn test_cli_parse_sandbox_cherry_pick() {
        let args = vec![
            "microvm",
            "sandbox",
            "claude",
            "--workspace",
            "/path/to/project",
            "--cherry-pick",
            "a1b2c3d4",
            "--apply-to-host",
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Commands::Sandbox {
                agent,
                workspace,
                cherry_pick,
                apply_to_host,
                ..
            } => {
                assert_eq!(agent, "claude");
                assert_eq!(workspace, PathBuf::from("/path/to/project"));
                assert_eq!(cherry_pick, Some("a1b2c3d4".to_string()));
                assert!(apply_to_host);
            }
            _ => panic!("Expected Commands::Sandbox with cherry-pick"),
        }
    }

    #[test]
    fn test_cli_parse_cherry_pick_command() {
        let args = vec![
            "microvm",
            "cherry-pick",
            "vm-test1234",
            "-w",
            "/host/workspace",
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Commands::CherryPick {
                id,
                workspace,
                data_dir,
            } => {
                assert_eq!(id, "vm-test1234");
                assert_eq!(workspace, PathBuf::from("/host/workspace"));
                assert!(data_dir.is_none());
            }
            _ => panic!("Expected Commands::CherryPick"),
        }
    }

    #[test]
    fn test_cli_parse_run_mac_and_mtu() {
        let args = vec![
            "microvm",
            "run",
            "--mac",
            "5a:94:ef:e4:0c:ee",
            "--mtu",
            "9000",
            "alpine:latest",
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Commands::Run(run) => {
                assert_eq!(run.mac, Some("5a:94:ef:e4:0c:ee".to_string()));
                assert_eq!(run.mtu, Some(9000));
            }
            _ => panic!("Expected Commands::Run"),
        }
    }

    #[test]
    fn test_cli_parse_network_commands() {
        // network ls
        let cli = Cli::try_parse_from(vec!["microvm", "network", "ls", "--json"]).unwrap();
        match cli.command {
            Commands::Network(NetworkCommands::Ls { json, .. }) => assert!(json),
            _ => panic!("Expected NetworkCommands::Ls"),
        }

        // network inspect
        let cli =
            Cli::try_parse_from(vec!["microvm", "network", "inspect", "vm-net-123"]).unwrap();
        match cli.command {
            Commands::Network(NetworkCommands::Inspect { id, json, .. }) => {
                assert_eq!(id, "vm-net-123");
                assert!(!json);
            }
            _ => panic!("Expected NetworkCommands::Inspect"),
        }

        // network ports
        let cli = Cli::try_parse_from(vec!["microvm", "network", "ports"]).unwrap();
        match cli.command {
            Commands::Network(NetworkCommands::Ports { id, .. }) => assert!(id.is_none()),
            _ => panic!("Expected NetworkCommands::Ports"),
        }

        // network test
        let cli = Cli::try_parse_from(vec![
            "microvm",
            "network",
            "test",
            "vm-net-123",
            "api.openai.com",
        ])
        .unwrap();
        match cli.command {
            Commands::Network(NetworkCommands::Test { id, target, .. }) => {
                assert_eq!(id, "vm-net-123");
                assert_eq!(target, "api.openai.com");
            }
            _ => panic!("Expected NetworkCommands::Test"),
        }
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("superlongstringexample", 8), "super...");
    }
}
