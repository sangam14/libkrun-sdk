//! In-guest interactive and non-interactive command execution protocol.
//!
//! Provides protocol framing, vsock transport coordination, and execution
//! management for running commands inside live microVMs (`microvm exec`).

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Payload sent from the host caller to execute a process inside the microVM.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecRequest {
    /// Command and arguments to execute (e.g. `["/bin/sh", "-c", "uname -a"]`).
    pub cmd: Vec<String>,
    /// Environment variables (e.g. `["FOO=bar", "TERM=xterm-256color"]`).
    #[serde(default)]
    pub env: Vec<String>,
    /// Optional working directory inside the guest.
    #[serde(default)]
    pub workdir: Option<String>,
    /// Whether to allocate a pseudo-terminal (TTY).
    #[serde(default)]
    pub tty: bool,
}

impl ExecRequest {
    pub fn new(cmd: Vec<String>) -> Self {
        Self {
            cmd,
            env: Vec::new(),
            workdir: None,
            tty: false,
        }
    }

    pub fn with_env(mut self, env: Vec<String>) -> Self {
        self.env = env;
        self
    }

    pub fn with_workdir(mut self, workdir: impl Into<String>) -> Self {
        self.workdir = Some(workdir.into());
        self
    }

    pub fn with_tty(mut self, tty: bool) -> Self {
        self.tty = tty;
        self
    }
}

/// Result returned from the in-guest execution agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecResponse {
    /// Exit code of the executed process (0 for success).
    pub exit_code: i32,
    /// Standard output text or byte stream.
    pub stdout: String,
    /// Standard error text or byte stream.
    pub stderr: String,
    /// Optional error description if execution or spawning failed.
    #[serde(default)]
    pub error: Option<String>,
}

impl ExecResponse {
    pub fn success(stdout: String, stderr: String) -> Self {
        Self {
            exit_code: 0,
            stdout,
            stderr,
            error: None,
        }
    }

    pub fn failure(code: i32, stdout: String, stderr: String, error: Option<String>) -> Self {
        Self {
            exit_code: code,
            stdout,
            stderr,
            error,
        }
    }
}

// Frame channels matching the in-guest exec agent
pub const CH_STDIN: u8 = 0;
pub const CH_STDOUT: u8 = 1;
pub const CH_STDERR: u8 = 2;
pub const CH_EXIT: u8 = 3;
pub const CH_WINSZ: u8 = 4;

/// Maximum allowed response size from the in-guest agent (64 MiB).
/// Prevents a malicious or buggy agent from causing OOM via oversized length prefix.
const MAX_RESPONSE_SIZE: usize = 64 * 1024 * 1024;

/// Executes a command in a live microVM given its instance rootfs directory and vsock socket.
pub async fn exec_in_microvm(
    socket_path: &Path,
    rootfs_path: &Path,
    req: &ExecRequest,
) -> Result<ExecResponse> {
    if req.cmd.is_empty() {
        bail!("Exec command cannot be empty");
    }

    // 1. Check if an in-guest agent port file exists (e.g. forwarded by TSI or network manager)
    let instance_dir = socket_path.parent().unwrap_or(rootfs_path);
    let agent_port_file = instance_dir.join("agent.port");
    if agent_port_file.exists() {
        if let Ok(port_str) = std::fs::read_to_string(&agent_port_file) {
            if let Ok(port) = port_str.trim().parse::<u16>() {
                if let Ok(resp) = exec_via_tcp_agent("127.0.0.1", port, req).await {
                    return Ok(resp);
                }
            }
        }
    }

    // 2. If the vsock socket is actively listening on the host, communicate via vsock protocol
    if socket_path.exists() {
        if let Ok(mut stream) = tokio::net::UnixStream::connect(socket_path).await {
            let req_bytes = serde_json::to_vec(req)?;
            let len_prefix = (req_bytes.len() as u32).to_be_bytes();
            stream.write_all(&len_prefix).await?;
            stream.write_all(&req_bytes).await?;
            stream.flush().await?;

            // Read response
            let mut resp_len_buf = [0u8; 4];
            if stream.read_exact(&mut resp_len_buf).await.is_ok() {
                let resp_len = u32::from_be_bytes(resp_len_buf) as usize;
                if resp_len > MAX_RESPONSE_SIZE {
                    bail!("Guest agent response too large ({resp_len} bytes, max {MAX_RESPONSE_SIZE})");
                }
                let mut resp_buf = vec![0u8; resp_len];
                if stream.read_exact(&mut resp_buf).await.is_ok() {
                    if let Ok(resp) = serde_json::from_slice::<ExecResponse>(&resp_buf) {
                        return Ok(resp);
                    }
                }
            }
        }
    }

    // 3. Fail explicitly if no hypervisor guest agent is reachable.
    // Silently falling back to running guest binaries directly on the host OS kernel
    // breaks container isolation and causes ENOEXEC failures on non-Linux hosts.
    bail!(
        "Cannot execute command in microVM: guest execution agent is not reachable via vsock socket '{}' or TCP agent. Ensure the microVM is running with an active guest agent.",
        socket_path.display()
    );
}

/// Executes a command via the in-guest TCP agent over loopback.
pub async fn exec_via_tcp_agent(host: &str, port: u16, req: &ExecRequest) -> Result<ExecResponse> {
    use tokio::net::TcpStream;
    let mut stream = TcpStream::connect((host, port))
        .await
        .with_context(|| format!("Failed to connect to guest agent at {host}:{port}"))?;
    stream.set_nodelay(true)?;

    let req_bytes = serde_json::to_vec(req)?;
    let len_prefix = (req_bytes.len() as u32).to_be_bytes();
    stream.write_all(&len_prefix).await?;
    stream.write_all(&req_bytes).await?;
    stream.flush().await?;

    let mut resp_len_buf = [0u8; 4];
    stream.read_exact(&mut resp_len_buf).await?;
    let resp_len = u32::from_be_bytes(resp_len_buf) as usize;
    if resp_len > MAX_RESPONSE_SIZE {
        bail!("Guest agent response too large ({resp_len} bytes, max {MAX_RESPONSE_SIZE})");
    }
    let mut resp_buf = vec![0u8; resp_len];
    stream.read_exact(&mut resp_buf).await?;

    let resp = serde_json::from_slice::<ExecResponse>(&resp_buf)
        .context("Failed to deserialize agent exec response")?;
    Ok(resp)
}

/// Executes a command directly on the host using rootfs as working directory.
///
/// # Security Notice
/// This function executes commands directly on the host OS kernel. It is intended
/// for offline container provisioning, rootfs setup, or mock testing environments.
/// In-microVM execution in live instances must use [`exec_in_microvm`].
pub async fn exec_in_guest_rootfs(rootfs_path: &Path, req: &ExecRequest) -> Result<ExecResponse> {
    if !rootfs_path.exists() {
        bail!(
            "Guest rootfs path does not exist: {}",
            rootfs_path.display()
        );
    }

    let program = &req.cmd[0];
    let args = &req.cmd[1..];

    // Check if program exists in rootfs /bin, /usr/bin, /sbin, or relative to rootfs
    let candidate_path = if program.starts_with('/') {
        let rel = program.strip_prefix('/').unwrap_or(program);
        rootfs_path.join(rel)
    } else {
        let in_bin = rootfs_path.join("bin").join(program);
        let in_usr_bin = rootfs_path.join("usr/bin").join(program);
        if in_bin.exists() {
            in_bin
        } else if in_usr_bin.exists() {
            in_usr_bin
        } else {
            rootfs_path.join(program)
        }
    };

    let exec_binary = if candidate_path.exists() {
        candidate_path
    } else {
        // Fallback to host PATH if binary not directly resolved inside guest rootfs
        PathBuf::from(program)
    };

    let mut cmd = tokio::process::Command::new(exec_binary);
    cmd.args(args);

    if let Some(ref wd) = req.workdir {
        let resolved_wd = if wd.starts_with('/') {
            let rel = wd.strip_prefix('/').unwrap_or(wd);
            rootfs_path.join(rel)
        } else {
            rootfs_path.join(wd)
        };
        if resolved_wd.exists() {
            cmd.current_dir(resolved_wd);
        } else {
            cmd.current_dir(rootfs_path);
        }
    } else {
        cmd.current_dir(rootfs_path);
    }

    for env_str in &req.env {
        if let Some((k, v)) = env_str.split_once('=') {
            cmd.env(k, v);
        }
    }

    let output = cmd
        .output()
        .await
        .with_context(|| format!("Failed to execute command '{:?}' in guest rootfs", req.cmd))?;

    let exit_code = output.status.code().unwrap_or(1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    Ok(ExecResponse {
        exit_code,
        stdout,
        stderr,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_request_serialization() {
        let req = ExecRequest::new(vec!["ls".to_string(), "-la".to_string()])
            .with_env(vec!["FOO=bar".to_string()])
            .with_workdir("/app")
            .with_tty(true);

        let json = serde_json::to_string(&req).unwrap();
        let decoded: ExecRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, decoded);
    }

    #[test]
    fn test_exec_response_serialization() {
        let resp = ExecResponse::failure(
            127,
            "".to_string(),
            "command not found\n".to_string(),
            Some("Not found".to_string()),
        );
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: ExecResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }

    #[tokio::test]
    async fn test_exec_in_guest_rootfs_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let req = ExecRequest::new(vec!["echo".to_string(), "krun-exec-test".to_string()]);
        let resp = exec_in_guest_rootfs(tmp.path(), &req).await.unwrap();
        assert_eq!(resp.exit_code, 0);
        assert!(resp.stdout.contains("krun-exec-test"));
    }

    #[tokio::test]
    async fn test_exec_in_microvm_unreachable_fails_safely() {
        let tmp = tempfile::tempdir().unwrap();
        let sock = tmp.path().join("nonexistent.sock");
        let rootfs = tmp.path().join("rootfs");
        std::fs::create_dir_all(&rootfs).unwrap();

        let req = ExecRequest::new(vec!["echo".to_string(), "hello".to_string()]);
        let res = exec_in_microvm(&sock, &rootfs, &req).await;
        assert!(res.is_err());
        let err = res.err().unwrap().to_string();
        assert!(err.contains("not reachable"));
    }
}
