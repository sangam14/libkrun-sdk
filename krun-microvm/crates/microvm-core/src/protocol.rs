//! Dedicated supervisor control protocol for microVM lifecycle management.
//!
//! Provides length-prefixed JSON wire framing, versioned control envelopes,
//! message payloads for commands and events, peer credential authentication,
//! and both asynchronous (Tokio) and synchronous (`std::io`) stream codecs.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Current supported protocol version.
pub const CURRENT_PROTOCOL_VERSION: u32 = 1;

/// Maximum payload size permitted for control frames (1 MiB).
/// Prevents denial-of-service / OOM via oversized length prefixes.
pub const MAX_CONTROL_PAYLOAD_SIZE: usize = 1024 * 1024;

/// Absolute maximum frame payload size (64 MiB), allowing exec stdout/stderr streaming.
pub const MAX_FRAME_PAYLOAD_SIZE: usize = 64 * 1024 * 1024;

/// Default timeout in seconds for graceful VM stop before escalating.
pub const DEFAULT_STOP_TIMEOUT_SECS: u32 = 5;

fn default_stop_timeout() -> u32 {
    DEFAULT_STOP_TIMEOUT_SECS
}

/// Versioned control message envelope exchanged over the supervisor socket.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ControlMessage {
    /// Protocol version.
    pub version: u32,
    /// Unique sequence or correlation ID.
    pub id: u64,
    /// Unix timestamp in seconds when the message was generated.
    pub timestamp: u64,
    /// Typed message payload.
    pub payload: MessagePayload,
}

impl ControlMessage {
    /// Constructs a new `ControlMessage` with the current protocol version and timestamp.
    pub fn new(id: u64, payload: MessagePayload) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            version: CURRENT_PROTOCOL_VERSION,
            id,
            timestamp,
            payload,
        }
    }
}

/// Typed message payload variants for the supervisor protocol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "data")]
pub enum MessagePayload {
    // -------------------------------------------------------------------------
    // Host -> Runner Lifecycle Commands
    // -------------------------------------------------------------------------
    /// Instructs the runner to begin VM configuration and enter hypervisor loop.
    Start,
    /// Liveness heartbeat ping.
    Ping,
    /// Suspends vCPU execution and hypervisor event loops.
    Pause,
    /// Resumes suspended vCPUs and hypervisor event loops.
    Resume,
    /// Requests graceful shutdown of the microVM.
    Stop {
        #[serde(default = "default_stop_timeout")]
        timeout_secs: u32,
        #[serde(default)]
        force: bool,
    },
    /// Forces immediate termination of the supervisor process.
    Kill,
    /// Queries hypervisor and runner process resource statistics.
    Stats,
    /// Dispatches an in-guest command execution request.
    Exec(crate::exec::ExecRequest),

    // -------------------------------------------------------------------------
    // Runner -> Host Events and Responses
    // -------------------------------------------------------------------------
    /// Emitted by the runner once its socket is bound and initialization is complete.
    Ready { pid: u32, version: String },
    /// Emitted by the runner when `krun_start_enter` has been entered and vCPUs are running.
    Booted { timestamp: u64 },
    /// Reply to `Ping`.
    Pong {
        #[serde(default)]
        latency_us: Option<u64>,
    },
    /// Generic positive acknowledgment for a command.
    Ack { req_id: u64, status: String },
    /// Reply to `Stats` containing process metrics and memory usage.
    StatsResponse(crate::metrics::ProcessStats),
    /// Reply to `Exec` containing exit code, stdout, and stderr.
    ExecResponse(crate::exec::ExecResponse),
    /// Emitted when the microVM exits cleanly or is stopped.
    Exited { exit_code: i32, reason: String },
    /// Emitted when initialization or hypervisor execution suffers a fatal failure.
    Failed { error: String, phase: String },
    /// Negative acknowledgment or error condition.
    Error {
        #[serde(default)]
        req_id: Option<u64>,
        message: String,
    },
}

// =============================================================================
// Synchronous Codec (`std::io::{Read, Write}`)
// Used by `microvm-runner` and non-async `StateManager` methods
// =============================================================================

/// Writes a length-prefixed `ControlMessage` to a synchronous writer.
pub fn write_frame_sync<W: std::io::Write>(writer: &mut W, msg: &ControlMessage) -> Result<()> {
    let payload_bytes = serde_json::to_vec(msg).context("Failed to serialize ControlMessage")?;
    let len = payload_bytes.len();
    if len > MAX_FRAME_PAYLOAD_SIZE {
        bail!("Frame payload size {len} exceeds ceiling of {MAX_FRAME_PAYLOAD_SIZE} bytes");
    }

    let len_prefix = (len as u32).to_be_bytes();
    writer.write_all(&len_prefix)?;
    writer.write_all(&payload_bytes)?;
    writer.flush()?;
    Ok(())
}

/// Reads a length-prefixed `ControlMessage` from a synchronous reader.
pub fn read_frame_sync<R: std::io::Read>(reader: &mut R) -> Result<ControlMessage> {
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .context("Failed to read frame length prefix")?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len == 0 {
        bail!("Invalid empty frame received");
    }
    if len > MAX_FRAME_PAYLOAD_SIZE {
        bail!("Frame length {len} exceeds maximum allowed size of {MAX_FRAME_PAYLOAD_SIZE}");
    }

    let mut payload_buf = vec![0u8; len];
    reader
        .read_exact(&mut payload_buf)
        .context("Failed to read frame payload")?;

    let msg = serde_json::from_slice::<ControlMessage>(&payload_buf)
        .context("Failed to deserialize ControlMessage")?;
    Ok(msg)
}

// =============================================================================
// Asynchronous Codec (`tokio::io::{AsyncReadExt, AsyncWriteExt}`)
// Used by `microvm-core::vm::MicroVm` and async workflows
// =============================================================================

/// Writes a length-prefixed `ControlMessage` to an asynchronous writer.
pub async fn write_frame_async<W: tokio::io::AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg: &ControlMessage,
) -> Result<()> {
    let payload_bytes = serde_json::to_vec(msg).context("Failed to serialize ControlMessage")?;
    let len = payload_bytes.len();
    if len > MAX_FRAME_PAYLOAD_SIZE {
        bail!("Frame payload size {len} exceeds ceiling of {MAX_FRAME_PAYLOAD_SIZE} bytes");
    }

    let len_prefix = (len as u32).to_be_bytes();
    writer.write_all(&len_prefix).await?;
    writer.write_all(&payload_bytes).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads a length-prefixed `ControlMessage` from an asynchronous reader.
pub async fn read_frame_async<R: tokio::io::AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<ControlMessage> {
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .await
        .context("Failed to read frame length prefix")?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len == 0 {
        bail!("Invalid empty frame received");
    }
    if len > MAX_FRAME_PAYLOAD_SIZE {
        bail!("Frame length {len} exceeds maximum allowed size of {MAX_FRAME_PAYLOAD_SIZE}");
    }

    let mut payload_buf = vec![0u8; len];
    reader
        .read_exact(&mut payload_buf)
        .await
        .context("Failed to read frame payload")?;

    let msg = serde_json::from_slice::<ControlMessage>(&payload_buf)
        .context("Failed to deserialize ControlMessage")?;
    Ok(msg)
}

// =============================================================================
// Peer Authentication & Access Verification
// =============================================================================

/// Verifies that the connecting peer on a Unix domain socket matches the current EUID
/// or is superuser (root).
#[cfg(unix)]
pub fn verify_peer_credentials(fd: std::os::unix::io::RawFd) -> Result<()> {
    let current_uid = unsafe { libc::geteuid() };

    #[cfg(target_os = "macos")]
    {
        let mut euid: libc::uid_t = 0;
        let mut egid: libc::gid_t = 0;
        if unsafe { libc::getpeereid(fd, &mut euid, &mut egid) } != 0 {
            bail!(
                "Failed to get peer credentials: {}",
                std::io::Error::last_os_error()
            );
        }
        if euid != current_uid && current_uid != 0 && euid != 0 {
            bail!("Unauthorized peer: UID {euid} does not match supervisor UID {current_uid}");
        }
    }

    #[cfg(target_os = "linux")]
    {
        let mut ucred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut ucred as *mut _ as *mut libc::c_void,
                &mut len,
            )
        } != 0
        {
            bail!(
                "Failed to get peer credentials: {}",
                std::io::Error::last_os_error()
            );
        }
        if ucred.uid != current_uid && current_uid != 0 && ucred.uid != 0 {
            bail!(
                "Unauthorized peer: UID {} does not match supervisor UID {current_uid}",
                ucred.uid
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_control_message_roundtrip_sync() {
        let msg = ControlMessage::new(
            42,
            MessagePayload::Ready {
                pid: 1234,
                version: "0.1.0".to_string(),
            },
        );

        let mut buffer = Vec::new();
        write_frame_sync(&mut buffer, &msg).expect("write failed");

        let mut reader = Cursor::new(buffer);
        let decoded = read_frame_sync(&mut reader).expect("read failed");

        assert_eq!(decoded.version, CURRENT_PROTOCOL_VERSION);
        assert_eq!(decoded.id, 42);
        match decoded.payload {
            MessagePayload::Ready { pid, version } => {
                assert_eq!(pid, 1234);
                assert_eq!(version, "0.1.0");
            }
            other => panic!("Unexpected payload: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_control_message_roundtrip_async() {
        let msg = ControlMessage::new(
            100,
            MessagePayload::Stop {
                timeout_secs: 10,
                force: false,
            },
        );

        let mut buffer = Vec::new();
        write_frame_async(&mut buffer, &msg)
            .await
            .expect("async write failed");

        let mut reader = Cursor::new(buffer);
        let decoded = read_frame_async(&mut reader)
            .await
            .expect("async read failed");

        assert_eq!(decoded.id, 100);
        match decoded.payload {
            MessagePayload::Stop {
                timeout_secs,
                force,
            } => {
                assert_eq!(timeout_secs, 10);
                assert!(!force);
            }
            other => panic!("Unexpected payload: {:?}", other),
        }
    }

    #[test]
    fn test_oversized_payload_rejection() {
        let oversized = ((MAX_FRAME_PAYLOAD_SIZE as u32) + 1024).to_be_bytes();
        let mut reader = Cursor::new(oversized);
        let res = read_frame_sync(&mut reader);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("exceeds maximum"));
    }

    #[test]
    fn test_empty_frame_rejection() {
        let empty_len = 0u32.to_be_bytes();
        let mut reader = Cursor::new(empty_len);
        let res = read_frame_sync(&mut reader);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("empty frame"));
    }

    #[test]
    fn test_payload_variants_serialization() {
        let payloads = vec![
            MessagePayload::Start,
            MessagePayload::Ping,
            MessagePayload::Pause,
            MessagePayload::Resume,
            MessagePayload::Kill,
            MessagePayload::Stats,
            MessagePayload::Booted { timestamp: 9999 },
            MessagePayload::Pong {
                latency_us: Some(150),
            },
            MessagePayload::Ack {
                req_id: 1,
                status: "OK".into(),
            },
            MessagePayload::Exited {
                exit_code: 0,
                reason: "clean".into(),
            },
            MessagePayload::Failed {
                error: "failed".into(),
                phase: "init".into(),
            },
            MessagePayload::Error {
                req_id: Some(1),
                message: "err".into(),
            },
        ];

        for p in payloads {
            let msg = ControlMessage::new(1, p.clone());
            let encoded = serde_json::to_string(&msg).unwrap();
            let decoded: ControlMessage = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded.payload, p);
        }
    }

    #[test]
    #[cfg(unix)]
    fn test_peer_credential_verification_socketpair() {
        use std::os::unix::io::AsRawFd;
        use std::os::unix::net::UnixStream;

        let (client, server) = UnixStream::pair().expect("socketpair failed");
        assert!(verify_peer_credentials(client.as_raw_fd()).is_ok());
        assert!(verify_peer_credentials(server.as_raw_fd()).is_ok());
    }

    #[tokio::test]
    async fn test_unix_stream_ipc_handshake() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let sock_path = dir.path().join("test_supervisor.sock");

        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();

        let server_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();

            // Send Ready
            let ready = ControlMessage::new(
                0,
                MessagePayload::Ready {
                    pid: std::process::id(),
                    version: "1.0.0".into(),
                },
            );
            write_frame_async(&mut stream, &ready).await.unwrap();

            // Read Ping -> Send Pong
            let ping = read_frame_async(&mut stream).await.unwrap();
            assert_eq!(ping.payload, MessagePayload::Ping);
            let pong = ControlMessage::new(
                ping.id,
                MessagePayload::Pong {
                    latency_us: Some(42),
                },
            );
            write_frame_async(&mut stream, &pong).await.unwrap();

            // Read Stats -> Send StatsResponse
            let stats_req = read_frame_async(&mut stream).await.unwrap();
            assert_eq!(stats_req.payload, MessagePayload::Stats);
            let dummy_stats = crate::metrics::ProcessStats {
                memory_rss_bytes: 1024 * 1024,
                memory_vsize_bytes: 2048 * 1024,
                user_cpu_ns: 10_000_000,
                kernel_cpu_ns: 5_000_000,
                total_cpu_ns: 15_000_000,
                page_faults: 100,
                major_page_faults: 0,
                threads: 4,
            };
            let stats_resp =
                ControlMessage::new(stats_req.id, MessagePayload::StatsResponse(dummy_stats));
            write_frame_async(&mut stream, &stats_resp).await.unwrap();
        });

        // Client side
        let mut client = tokio::net::UnixStream::connect(&sock_path).await.unwrap();

        // 1. Receive Ready
        let ready_msg = read_frame_async(&mut client).await.unwrap();
        match ready_msg.payload {
            MessagePayload::Ready { pid, version } => {
                assert_eq!(pid, std::process::id());
                assert_eq!(version, "1.0.0");
            }
            other => panic!("Expected Ready, got {:?}", other),
        }

        // 2. Send Ping and receive Pong
        let ping_msg = ControlMessage::new(1, MessagePayload::Ping);
        write_frame_async(&mut client, &ping_msg).await.unwrap();
        let pong_msg = read_frame_async(&mut client).await.unwrap();
        assert_eq!(pong_msg.id, 1);
        match pong_msg.payload {
            MessagePayload::Pong { latency_us } => {
                assert_eq!(latency_us, Some(42));
            }
            other => panic!("Expected Pong, got {:?}", other),
        }

        // 3. Send Stats and receive StatsResponse
        let stats_req = ControlMessage::new(2, MessagePayload::Stats);
        write_frame_async(&mut client, &stats_req).await.unwrap();
        let stats_resp = read_frame_async(&mut client).await.unwrap();
        assert_eq!(stats_resp.id, 2);
        match stats_resp.payload {
            MessagePayload::StatsResponse(s) => {
                assert_eq!(s.memory_rss_bytes, 1024 * 1024);
            }
            other => panic!("Expected StatsResponse, got {:?}", other),
        }

        server_task.await.unwrap();
    }
}
