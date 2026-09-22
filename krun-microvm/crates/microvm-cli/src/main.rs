use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use microvm_core::{
    collect_process_stats, ImageReference, MicroVmBuilder, OciArtifact, OciClient, OciLayout,
    Preflight, StateManager, VmStatus,
};
use serde_json::json;
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

    /// Network mode: tsi (default), none (air-gapped), or unix:<socket_path>
    #[arg(long, default_value = "tsi")]
    pub net: String,

    /// Custom DNS nameservers (e.g. 8.8.8.8,1.1.1.1; defaults to autonomous resilient fallback)
    #[arg(long = "dns")]
    pub dns: Vec<String>,

    /// Custom guest hostname (defaults to microVM instance ID)
    #[arg(long)]
    pub hostname: Option<String>,

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
    #[arg(long = "allow-host")]
    pub allow_hosts: Vec<String>,

    /// Inject secret via in-flight substitution (KEY=VALUE, KEY=env:VAR_NAME, or KEY=file:/path)
    #[arg(long = "secret")]
    pub secrets: Vec<String>,

    /// Hard ceiling on cumulative LLM tokens (prompt + completion) consumed by the microVM
    #[arg(long = "max-tokens")]
    pub max_tokens: Option<u64>,

    /// Direct kernel boot: path to kernel binary (ELF, RAW, bzImage)
    #[arg(long = "kernel")]
    pub kernel: Option<PathBuf>,

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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    let default_data_dir = || {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".cache/krun-microvm")
    };

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
                initrd,
                cmdline,
                firmware,
                disks,
                cmd,
            } = *run;
            let mut builder = if let Some(ref b) = bundle {
                if !detach {
                    println!("📦 Loading MicroVM from OCI bundle: {}", b.display());
                }
                MicroVmBuilder::from_bundle(b)?
            } else if let Some(ref kpath) = kernel {
                if !detach {
                    println!("🚀 Direct kernel boot: {}", kpath.display());
                }
                MicroVmBuilder::new("").kernel(kpath.clone(), initrd, cmdline)
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

            if let Some(dd) = data_dir {
                builder = builder.data_dir(dd);
            }

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

            if let Some(git_repo) = repo {
                let clone_script = format!(
                    "if [ ! -d .git ]; then git clone {} .; fi; exec /bin/sh",
                    git_repo
                );
                builder = builder.cmd(vec!["/bin/sh".to_string(), "-c".to_string(), clone_script]);
            } else if !cmd.is_empty() {
                builder = builder.cmd(cmd);
            } else {
                builder = builder.cmd(vec!["/bin/sh".to_string()]);
            }

            let mut vm = builder.run().await.context("Failed to start sandbox")?;
            let status = vm.wait().await?;
            println!("🛑 Sandbox session closed with status: {}", status);
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
}
