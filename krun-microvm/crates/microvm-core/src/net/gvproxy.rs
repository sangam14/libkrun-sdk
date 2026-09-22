//! High-level safe Rust controller for gvproxy (gvisor-tap-vsock).
//!
//! Provides rootless, user-space virtual networking with virtual DNS, DHCP,
//! dynamic port forwarding, and egress domain/CIDR filtering (`allow_net`).

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_longlong};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Configuration for a gvproxy user-space network instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GvproxyConfig {
    pub socket_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_socket_path: Option<String>,
    pub subnet: String,
    pub gateway_ip: String,
    pub gateway_mac: String,
    pub guest_ip: String,
    pub guest_mac: String,
    pub mtu: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns_search_domains: Vec<String>,
    pub debug: bool,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub forwards: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_net: Vec<String>,
}

impl GvproxyConfig {
    /// Create a new gvproxy configuration with standard defaults.
    pub fn new(socket_path: impl AsRef<Path>) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_string_lossy().to_string(),
            control_socket_path: None,
            subnet: "192.168.127.0/24".to_string(),
            gateway_ip: "192.168.127.1".to_string(),
            gateway_mac: "5a:94:ef:e4:0c:dd".to_string(),
            guest_ip: "192.168.127.2".to_string(),
            guest_mac: "5a:94:ef:e4:0c:ee".to_string(),
            mtu: 1500,
            dns_search_domains: Vec::new(),
            debug: false,
            forwards: HashMap::new(),
            allow_net: Vec::new(),
        }
    }

    /// Set subnet CIDR (default: 192.168.127.0/24).
    pub fn subnet(mut self, subnet: impl Into<String>) -> Self {
        self.subnet = subnet.into();
        self
    }

    /// Set guest IP (default: 192.168.127.2).
    pub fn guest_ip(mut self, ip: impl Into<String>) -> Self {
        self.guest_ip = ip.into();
        self
    }

    /// Set gateway IP (default: 192.168.127.1).
    pub fn gateway_ip(mut self, ip: impl Into<String>) -> Self {
        self.gateway_ip = ip.into();
        self
    }

    /// Add a port forward mapping (host port -> guest port).
    pub fn add_forward(mut self, host_port: u16, guest_port: u16) -> Self {
        let local = format!("0.0.0.0:{}", host_port);
        let remote = format!("{}:{}", self.guest_ip, guest_port);
        self.forwards.insert(local, remote);
        self
    }

    /// Add an allowed destination domain or CIDR for egress control.
    pub fn allow_net(mut self, target: impl Into<String>) -> Self {
        self.allow_net.push(target.into());
        self
    }

    /// Add multiple allowed destinations.
    pub fn allow_net_list(mut self, targets: impl IntoIterator<Item = impl Into<String>>) -> Self {
        for t in targets {
            self.allow_net.push(t.into());
        }
        self
    }

    /// Start and spawn the gvproxy instance.
    pub fn start(&self) -> Result<GvproxyInstance> {
        GvproxyInstance::start(self)
    }
}

/// An active, running gvproxy virtual network instance.
///
/// Automatically stops and frees virtual network resources on [`Drop`].
#[derive(Debug)]
pub struct GvproxyInstance {
    id: i64,
    socket_path: PathBuf,
    guest_ip: String,
}

impl GvproxyInstance {
    /// Start a new gvproxy instance from configuration.
    pub fn start(config: &GvproxyConfig) -> Result<Self> {
        let json_str = serde_json::to_string(config)
            .context("Failed to serialize gvproxy configuration to JSON")?;
        let c_json =
            CString::new(json_str).context("Failed to convert gvproxy JSON config to CString")?;

        let mut err_out: *mut c_char = std::ptr::null_mut();

        debug!(
            socket = %config.socket_path,
            guest_ip = %config.guest_ip,
            "Starting gvproxy user-space network stack"
        );

        let id = unsafe { libgvproxy_sys::gvproxy_create(c_json.as_ptr(), &mut err_out) };

        if id < 0 {
            let err_msg = if !err_out.is_null() {
                let c_str = unsafe { CStr::from_ptr(err_out) };
                let msg = c_str.to_string_lossy().to_string();
                unsafe { libgvproxy_sys::gvproxy_free_string(err_out) };
                msg
            } else {
                "Unknown gvproxy_create error".to_string()
            };
            bail!("gvproxy initialization failed: {}", err_msg);
        }

        info!(
            id = id,
            socket = %config.socket_path,
            guest_ip = %config.guest_ip,
            "gvproxy user-mode network stack running"
        );

        Ok(Self {
            id,
            socket_path: PathBuf::from(&config.socket_path),
            guest_ip: config.guest_ip.clone(),
        })
    }

    /// Dynamically expose a port mapping (host port -> guest port) on the running network.
    pub fn expose_port(&self, host_port: u16, guest_port: u16) -> Result<()> {
        let mut err_out: *mut c_char = std::ptr::null_mut();
        let ret = unsafe {
            libgvproxy_sys::gvproxy_expose_port(
                self.id as c_longlong,
                host_port as c_int,
                guest_port as c_int,
                &mut err_out,
            )
        };

        if ret != 0 {
            let msg = if !err_out.is_null() {
                let c_str = unsafe { CStr::from_ptr(err_out) };
                let s = c_str.to_string_lossy().to_string();
                unsafe { libgvproxy_sys::gvproxy_free_string(err_out) };
                s
            } else {
                "failed to expose port".to_string()
            };
            bail!(
                "gvproxy expose_port({} -> {}) failed: {}",
                host_port,
                guest_port,
                msg
            );
        }

        debug!(id = self.id, host_port, guest_port, "gvproxy port exposed");
        Ok(())
    }

    /// Dynamically unexpose an existing host port mapping.
    pub fn unexpose_port(&self, host_port: u16) -> Result<()> {
        let mut err_out: *mut c_char = std::ptr::null_mut();
        let ret = unsafe {
            libgvproxy_sys::gvproxy_unexpose_port(
                self.id as c_longlong,
                host_port as c_int,
                &mut err_out,
            )
        };

        if ret != 0 {
            let msg = if !err_out.is_null() {
                let c_str = unsafe { CStr::from_ptr(err_out) };
                let s = c_str.to_string_lossy().to_string();
                unsafe { libgvproxy_sys::gvproxy_free_string(err_out) };
                s
            } else {
                "failed to unexpose port".to_string()
            };
            bail!("gvproxy unexpose_port({}) failed: {}", host_port, msg);
        }

        debug!(id = self.id, host_port, "gvproxy port unexposed");
        Ok(())
    }

    /// Return the Unix socket path that the microVM should connect to.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Return the assigned guest IP address.
    pub fn guest_ip(&self) -> &str {
        &self.guest_ip
    }

    /// Return the internal instance handle ID.
    pub fn instance_id(&self) -> i64 {
        self.id
    }
}

impl Drop for GvproxyInstance {
    fn drop(&mut self) {
        debug!(id = self.id, "Stopping and destroying gvproxy instance");
        unsafe {
            libgvproxy_sys::gvproxy_destroy(self.id as c_longlong);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gvproxy_config_builder() {
        let config = GvproxyConfig::new("/tmp/test.sock")
            .subnet("192.168.100.0/24")
            .guest_ip("192.168.100.2")
            .gateway_ip("192.168.100.1")
            .add_forward(8080, 80)
            .allow_net("github.com")
            .allow_net("crates.io");

        assert_eq!(config.subnet, "192.168.100.0/24");
        assert_eq!(config.guest_ip, "192.168.100.2");
        assert_eq!(config.gateway_ip, "192.168.100.1");
        assert_eq!(
            config.forwards.get("0.0.0.0:8080").unwrap(),
            "192.168.100.2:80"
        );
        assert_eq!(config.allow_net.len(), 2);
    }

    #[test]
    fn test_gvproxy_instance_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let sock = tmp.path().join("gvproxy.sock");

        let config = GvproxyConfig::new(&sock).add_forward(18088, 80);

        let instance = config.start().expect("Failed to start gvproxy instance");
        assert_eq!(instance.socket_path(), sock.as_path());
        assert!(sock.exists());

        // Test dynamic port expose
        instance
            .expose_port(18089, 443)
            .expect("Failed to expose port");
        instance
            .unexpose_port(18089)
            .expect("Failed to unexpose port");

        drop(instance);
        // Socket should be cleaned up on drop
        assert!(!sock.exists());
    }
}
