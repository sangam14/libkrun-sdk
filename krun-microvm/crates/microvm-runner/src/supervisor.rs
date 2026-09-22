use anyhow::{Context, Result};
use microvm_core::protocol::{
    read_frame_sync, verify_peer_credentials, write_frame_sync, ControlMessage, MessagePayload,
};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

/// Background supervisor server running on a dedicated OS thread.
pub struct SupervisorServer {
    sock_path: PathBuf,
    active_stream: Arc<Mutex<Option<UnixStream>>>,
}

impl SupervisorServer {
    /// Binds the supervisor Unix domain socket, restricts permissions to 0600,
    /// and spawns a background thread to handle parent lifecycle commands.
    pub fn start(sock_path: &Path) -> Result<Self> {
        if let Some(parent) = sock_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if sock_path.exists() {
            let _ = std::fs::remove_file(sock_path);
        }

        let listener = UnixListener::bind(sock_path).with_context(|| {
            format!(
                "Failed to bind supervisor socket at {}",
                sock_path.display()
            )
        })?;

        // Restrict socket file permissions to 0600 (owner read-write only)
        let _ = std::fs::set_permissions(sock_path, std::fs::Permissions::from_mode(0o600));

        let active_stream: Arc<Mutex<Option<UnixStream>>> = Arc::new(Mutex::new(None));
        let active_stream_clone = Arc::clone(&active_stream);

        thread::Builder::new()
            .name("supervisor-ipc".into())
            .spawn(move || {
                run_listener_loop(listener, active_stream_clone);
            })
            .context("Failed to spawn supervisor IPC listener thread")?;

        Ok(Self {
            sock_path: sock_path.to_path_buf(),
            active_stream,
        })
    }

    /// Emits a `Booted` lifecycle event to the connected parent.
    pub fn emit_booted(&self) {
        if let Ok(guard) = self.active_stream.lock() {
            if let Some(ref stream) = *guard {
                if let Ok(mut clone) = stream.try_clone() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let msg = ControlMessage::new(0, MessagePayload::Booted { timestamp: now });
                    let _ = write_frame_sync(&mut clone, &msg);
                }
            }
        }
    }

    /// Emits an `Exited` lifecycle event to the connected parent.
    pub fn emit_exited(&self, exit_code: i32, reason: &str) {
        if let Ok(guard) = self.active_stream.lock() {
            if let Some(ref stream) = *guard {
                if let Ok(mut clone) = stream.try_clone() {
                    let msg = ControlMessage::new(
                        0,
                        MessagePayload::Exited {
                            exit_code,
                            reason: reason.to_string(),
                        },
                    );
                    let _ = write_frame_sync(&mut clone, &msg);
                }
            }
        }
    }

    /// Emits a `Failed` lifecycle event to the connected parent.
    pub fn emit_failed(&self, error: &str, phase: &str) {
        if let Ok(guard) = self.active_stream.lock() {
            if let Some(ref stream) = *guard {
                if let Ok(mut clone) = stream.try_clone() {
                    let msg = ControlMessage::new(
                        0,
                        MessagePayload::Failed {
                            error: error.to_string(),
                            phase: phase.to_string(),
                        },
                    );
                    let _ = write_frame_sync(&mut clone, &msg);
                }
            }
        }
    }
}

impl Drop for SupervisorServer {
    fn drop(&mut self) {
        if self.sock_path.exists() {
            let _ = std::fs::remove_file(&self.sock_path);
        }
    }
}

fn run_listener_loop(listener: UnixListener, shared_stream: Arc<Mutex<Option<UnixStream>>>) {
    for incoming in listener.incoming() {
        match incoming {
            Ok(mut stream) => {
                // 1. Verify peer credentials
                if let Err(e) = verify_peer_credentials(stream.as_raw_fd()) {
                    eprintln!("[microvm-runner:supervisor] Peer rejected: {e}");
                    continue;
                }

                // 2. Send initial Ready message
                let ready_msg = ControlMessage::new(
                    0,
                    MessagePayload::Ready {
                        pid: std::process::id(),
                        version: env!("CARGO_PKG_VERSION").to_string(),
                    },
                );
                if let Err(e) = write_frame_sync(&mut stream, &ready_msg) {
                    eprintln!("[microvm-runner:supervisor] Failed to send Ready: {e}");
                    continue;
                }

                // Store stream for main thread notifications
                if let Ok(mut guard) = shared_stream.lock() {
                    *guard = stream.try_clone().ok();
                }

                // 3. Command loop for this connection
                handle_client_connection(&mut stream);

                // Clear shared stream on disconnect
                if let Ok(mut guard) = shared_stream.lock() {
                    *guard = None;
                }
            }
            Err(_) => {
                break;
            }
        }
    }
}

fn handle_client_connection(stream: &mut UnixStream) {
    loop {
        let msg = match read_frame_sync(stream) {
            Ok(m) => m,
            Err(_) => break, // EOF or disconnected
        };

        match msg.payload {
            MessagePayload::Ping => {
                let reply = ControlMessage::new(msg.id, MessagePayload::Pong { latency_us: None });
                let _ = write_frame_sync(stream, &reply);
            }
            MessagePayload::Stats => {
                let stats = microvm_core::metrics::collect_process_stats(std::process::id());
                let reply = match stats {
                    Some(s) => ControlMessage::new(msg.id, MessagePayload::StatsResponse(s)),
                    None => ControlMessage::new(
                        msg.id,
                        MessagePayload::Error {
                            req_id: Some(msg.id),
                            message: "Failed to collect process telemetry".to_string(),
                        },
                    ),
                };
                let _ = write_frame_sync(stream, &reply);
            }
            MessagePayload::Pause => {
                let reply = ControlMessage::new(
                    msg.id,
                    MessagePayload::Ack {
                        req_id: msg.id,
                        status: "Paused".into(),
                    },
                );
                let _ = write_frame_sync(stream, &reply);
                unsafe {
                    libc::kill(libc::getpid(), libc::SIGSTOP);
                }
            }
            MessagePayload::Resume => {
                let reply = ControlMessage::new(
                    msg.id,
                    MessagePayload::Ack {
                        req_id: msg.id,
                        status: "Resumed".into(),
                    },
                );
                let _ = write_frame_sync(stream, &reply);
            }
            MessagePayload::Stop {
                timeout_secs: _,
                force,
            } => {
                let reply = ControlMessage::new(
                    msg.id,
                    MessagePayload::Ack {
                        req_id: msg.id,
                        status: "Stopping".into(),
                    },
                );
                let _ = write_frame_sync(stream, &reply);
                if force {
                    std::process::exit(130);
                } else {
                    unsafe {
                        libc::kill(libc::getpid(), libc::SIGTERM);
                    }
                }
            }
            MessagePayload::Kill => {
                std::process::exit(137);
            }
            _ => {
                let reply = ControlMessage::new(
                    msg.id,
                    MessagePayload::Error {
                        req_id: Some(msg.id),
                        message: "Unsupported supervisor command".into(),
                    },
                );
                let _ = write_frame_sync(stream, &reply);
            }
        }
    }
}
