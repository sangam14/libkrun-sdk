//! Low-level CGO FFI bindings to gvproxy (gvisor-tap-vsock).
//!
//! Provides raw `extern "C"` declarations exported by the CGO archive in `gvproxy-bridge`.

use std::os::raw::{c_char, c_int, c_longlong, c_void};

pub type LogCallbackFn = extern "C" fn(level: c_int, message: *const c_char);

extern "C" {
    /// Set a global callback to receive structured log messages from Go (logrus).
    pub fn gvproxy_set_log_callback(callback: *const c_void);

    /// Create and start a new gvproxy virtual network instance.
    ///
    /// # Arguments
    /// * `config_json` - Null-terminated JSON string specifying network configuration.
    /// * `err_out` - Pointer to receive error message C-string on failure (caller must free with `gvproxy_free_string`).
    ///
    /// # Returns
    /// Non-negative instance ID on success, or -1 on failure.
    pub fn gvproxy_create(config_json: *const c_char, err_out: *mut *mut c_char) -> c_longlong;

    /// Dynamically expose a port forwarding mapping (host port -> guest port).
    ///
    /// # Returns
    /// 0 on success, non-zero on failure with `err_out` populated.
    pub fn gvproxy_expose_port(
        id: c_longlong,
        local_port: c_int,
        remote_port: c_int,
        err_out: *mut *mut c_char,
    ) -> c_int;

    /// Dynamically unexpose an existing host port forwarding mapping.
    ///
    /// # Returns
    /// 0 on success, non-zero on failure with `err_out` populated.
    pub fn gvproxy_unexpose_port(
        id: c_longlong,
        local_port: c_int,
        err_out: *mut *mut c_char,
    ) -> c_int;

    /// Terminate and destroy a gvproxy instance, freeing all listeners and resources.
    pub fn gvproxy_destroy(id: c_longlong) -> c_int;

    /// Free a C-string allocated by Go runtime.
    pub fn gvproxy_free_string(str: *mut c_char);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};

    #[test]
    fn test_gvproxy_create_and_destroy() {
        let tmp_dir = tempfile::tempdir().expect("Failed to create tempdir");
        let sock_path = tmp_dir.path().join("qemu.sock");

        let config = serde_json::json!({
            "socket_path": sock_path.to_str().unwrap(),
            "subnet": "192.168.127.0/24",
            "gateway_ip": "192.168.127.1",
            "guest_ip": "192.168.127.2",
            "mtu": 1500,
            "debug": false,
            "forwards": {
                "0.0.0.0:18080": "192.168.127.2:80"
            }
        });

        let json_str = CString::new(config.to_string()).unwrap();
        let mut err_out: *mut c_char = std::ptr::null_mut();

        let id = unsafe { gvproxy_create(json_str.as_ptr(), &mut err_out) };
        if id < 0 {
            let msg = if !err_out.is_null() {
                let err_cstr = unsafe { CStr::from_ptr(err_out) };
                let s = err_cstr.to_string_lossy().to_string();
                unsafe { gvproxy_free_string(err_out) };
                s
            } else {
                "unknown error".to_string()
            };
            panic!("gvproxy_create failed: {msg}");
        }

        assert!(id >= 1, "Expected valid instance ID");
        assert!(sock_path.exists(), "Expected unix socket to be created");

        // Test dynamic port expose
        let mut expose_err: *mut c_char = std::ptr::null_mut();
        let res = unsafe { gvproxy_expose_port(id, 18081, 8080, &mut expose_err) };
        assert_eq!(res, 0, "Expected gvproxy_expose_port to succeed");

        // Test dynamic port unexpose
        let mut unexpose_err: *mut c_char = std::ptr::null_mut();
        let res = unsafe { gvproxy_unexpose_port(id, 18081, &mut unexpose_err) };
        assert_eq!(res, 0, "Expected gvproxy_unexpose_port to succeed");

        // Clean up
        let destroy_res = unsafe { gvproxy_destroy(id) };
        assert_eq!(destroy_res, 0);
    }

    #[test]
    fn test_gvproxy_invalid_json_reports_error() {
        let invalid = CString::new("not valid json").unwrap();
        let mut err_out: *mut c_char = std::ptr::null_mut();

        let id = unsafe { gvproxy_create(invalid.as_ptr(), &mut err_out) };
        assert_eq!(id, -1);
        assert!(!err_out.is_null());

        let err_cstr = unsafe { CStr::from_ptr(err_out) };
        let msg = err_cstr.to_string_lossy().to_string();
        assert!(msg.contains("invalid json"));

        unsafe { gvproxy_free_string(err_out) };
    }
}
