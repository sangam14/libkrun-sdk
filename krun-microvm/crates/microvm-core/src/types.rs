use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Port forwarding rule (host:guest).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortForward {
    pub host: u16,
    pub guest: u16,
}

impl PortForward {
    pub fn new(host: u16, guest: u16) -> Self {
        Self { host, guest }
    }
}

/// VirtioFS directory mount rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtioFsMount {
    pub tag: String,
    pub path: PathBuf,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub dax_window_size_bytes: Option<u64>,
}

impl VirtioFsMount {
    pub fn new(tag: impl Into<String>, path: impl Into<PathBuf>, read_only: bool) -> Self {
        Self {
            tag: tag.into(),
            path: path.into(),
            read_only,
            dax_window_size_bytes: None,
        }
    }

    pub fn with_dax(mut self, bytes: u64) -> Self {
        self.dax_window_size_bytes = Some(bytes);
        self
    }
}

/// Vsock port mapping to a host UNIX socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VsockPort {
    pub port: u32,
    pub socket_path: PathBuf,
}

impl VsockPort {
    pub fn new(port: u32, socket_path: impl Into<PathBuf>) -> Self {
        Self {
            port,
            socket_path: socket_path.into(),
        }
    }
}

/// Configuration passed to microvm-runner as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerConfig {
    pub root_path: PathBuf,
    pub num_vcpus: u8,
    pub ram_mib: u32,
    #[serde(default)]
    pub port_forwards: Vec<PortForward>,
    #[serde(default)]
    pub net_sock_path: Option<String>,
    #[serde(default)]
    pub virtiofs_mounts: Vec<VirtioFsMount>,
    #[serde(default)]
    pub vsock_ports: Vec<VsockPort>,
    #[serde(default)]
    pub console_log_path: Option<PathBuf>,
    #[serde(default)]
    pub log_level: Option<u32>,
    #[serde(default)]
    pub interactive: bool,
    #[serde(default)]
    pub tty: bool,
    #[serde(default)]
    pub no_network: bool,
    #[serde(default)]
    pub rlimits: Option<String>,
    #[serde(default)]
    pub detach: bool,
    #[serde(default)]
    pub dax_window_size_bytes: Option<u64>,
    #[serde(default)]
    pub image_acceleration: Option<crate::acceleration::ImageAcceleration>,
    #[serde(default)]
    pub gpu: bool,
    #[serde(default)]
    pub gpu_shm_size_bytes: Option<u64>,
    #[serde(default)]
    pub gpu_flags: Option<u32>,
    #[serde(default = "default_true")]
    pub sandbox: bool,
    #[serde(default)]
    pub allow_hosts: Vec<String>,
    #[serde(default)]
    pub secrets: Vec<(String, String)>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub proxy_port: Option<u16>,
}

fn default_true() -> bool {
    true
}
