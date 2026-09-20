use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum KrunError {
    #[error("Failed to create libkrun context, error code: {0}")]
    CreateContext(i32),
    #[error("Failed to configure VM (vCPUs/RAM), error code: {0}")]
    SetVmConfig(i32),
    #[error("Failed to set root filesystem, error code: {0}")]
    SetRoot(i32),
    #[error("Failed to set exec, error code: {0}")]
    SetExec(i32),
    #[error("Failed to set working directory, error code: {0}")]
    SetWorkdir(i32),
    #[error("Failed to set environment, error code: {0}")]
    SetEnv(i32),
    #[error("Failed to set port map, error code: {0}")]
    SetPortMap(i32),
    #[error("Failed to set console output, error code: {0}")]
    SetConsoleOutput(i32),
    #[error("Failed to set log level, error code: {0}")]
    SetLogLevel(i32),
    #[error("Failed to set resource limits (rlimits), error code: {0}")]
    SetRlimits(i32),
    #[error("Failed to add virtio-fs mount, error code: {0}")]
    AddVirtioFs(i32),
    #[error("Failed to add net unixstream, error code: {0}")]
    AddNetUnixStream(i32),
    #[error("Failed to add vsock port, error code: {0}")]
    AddVsockPort(i32),
    #[error("Failed to start VM via krun_start_enter, error code: {0}")]
    StartEnter(i32),
    #[error("Nul byte in string argument: {0}")]
    NulError(#[from] std::ffi::NulError),
}

pub mod ffi {
    use super::*;

    pub const COMPAT_NET_FEATURES: u32 =
        (1 << 0) | (1 << 1) | (1 << 7) | (1 << 10) | (1 << 11) | (1 << 14);

    extern "C" {
        pub fn krun_create_ctx() -> i32;
        pub fn krun_free_ctx(ctx_id: u32) -> i32;
        pub fn krun_set_vm_config(ctx_id: u32, num_vcpus: u8, ram_mib: u32) -> i32;
        pub fn krun_set_root(ctx_id: u32, root_path: *const c_char) -> i32;
        pub fn krun_set_exec(
            ctx_id: u32,
            exec_path: *const c_char,
            argv: *const *const c_char,
            envp: *const *const c_char,
        ) -> i32;
        pub fn krun_set_workdir(ctx_id: u32, workdir: *const c_char) -> i32;
        pub fn krun_set_env(ctx_id: u32, env: *const *const c_char) -> i32;
        pub fn krun_set_port_map(ctx_id: u32, port_map: *const *const c_char) -> i32;
        pub fn krun_set_console_output(ctx_id: u32, filepath: *const c_char) -> i32;
        pub fn krun_set_log_level(level: u32) -> i32;
        pub fn krun_set_rlimits(ctx_id: u32, rlimits: *const c_char) -> i32;
        pub fn krun_add_virtiofs(ctx_id: u32, tag: *const c_char, path: *const c_char) -> i32;
        pub fn krun_add_virtiofs2(
            ctx_id: u32,
            tag: *const c_char,
            path: *const c_char,
            flags: u32,
        ) -> i32;
        pub fn krun_add_net_unixstream(
            ctx_id: u32,
            c_path: *const c_char,
            fd: c_int,
            c_mac: *mut u8,
            features: u32,
            flags: u32,
        ) -> i32;
        pub fn krun_add_vsock_port(ctx_id: u32, port: u32, path: *const c_char) -> i32;
        pub fn krun_check_nested_virt() -> bool;
        pub fn krun_start_enter(ctx_id: u32) -> i32;
    }
}

pub fn set_log_level(level: u32) -> Result<(), KrunError> {
    let rc = unsafe { ffi::krun_set_log_level(level) };
    if rc != 0 {
        return Err(KrunError::SetLogLevel(rc));
    }
    Ok(())
}

pub fn check_nested_virt() -> bool {
    unsafe { ffi::krun_check_nested_virt() }
}

pub struct KrunContext {
    ctx_id: u32,
    active: bool,
}

impl KrunContext {
    pub fn create() -> Result<Self, KrunError> {
        let rc = unsafe { ffi::krun_create_ctx() };
        if rc < 0 {
            return Err(KrunError::CreateContext(rc));
        }
        Ok(Self {
            ctx_id: rc as u32,
            active: true,
        })
    }

    pub fn ctx_id(&self) -> u32 {
        self.ctx_id
    }

    pub fn set_vm_config(&mut self, vcpus: u8, ram_mib: u32) -> Result<(), KrunError> {
        let rc = unsafe { ffi::krun_set_vm_config(self.ctx_id, vcpus, ram_mib) };
        if rc != 0 {
            return Err(KrunError::SetVmConfig(rc));
        }
        Ok(())
    }

    pub fn set_root<P: AsRef<Path>>(&mut self, path: P) -> Result<(), KrunError> {
        let path_str = path.as_ref().to_string_lossy();
        let c_path = CString::new(path_str.as_bytes())?;
        let rc = unsafe { ffi::krun_set_root(self.ctx_id, c_path.as_ptr()) };
        if rc != 0 {
            return Err(KrunError::SetRoot(rc));
        }
        Ok(())
    }

    pub fn set_workdir(&mut self, workdir: &str) -> Result<(), KrunError> {
        let c_workdir = CString::new(workdir)?;
        let rc = unsafe { ffi::krun_set_workdir(self.ctx_id, c_workdir.as_ptr()) };
        if rc != 0 {
            return Err(KrunError::SetWorkdir(rc));
        }
        Ok(())
    }

    pub fn set_rlimits(&mut self, rlimits: &str) -> Result<(), KrunError> {
        let c_rlimits = CString::new(rlimits)?;
        let rc = unsafe { ffi::krun_set_rlimits(self.ctx_id, c_rlimits.as_ptr()) };
        if rc != 0 {
            return Err(KrunError::SetRlimits(rc));
        }
        Ok(())
    }

    pub fn set_port_map(&mut self, mappings: &[String]) -> Result<(), KrunError> {
        let mut c_strings = Vec::new();
        for m in mappings {
            c_strings.push(CString::new(m.as_str())?);
        }
        let mut ptrs: Vec<*const c_char> = c_strings.iter().map(|s| s.as_ptr()).collect();
        ptrs.push(std::ptr::null());

        let rc = unsafe { ffi::krun_set_port_map(self.ctx_id, ptrs.as_ptr()) };
        if rc != 0 {
            return Err(KrunError::SetPortMap(rc));
        }
        Ok(())
    }

    pub fn set_console_output<P: AsRef<Path>>(&mut self, path: P) -> Result<(), KrunError> {
        let path_str = path.as_ref().to_string_lossy();
        let c_path = CString::new(path_str.as_bytes())?;
        let rc = unsafe { ffi::krun_set_console_output(self.ctx_id, c_path.as_ptr()) };
        if rc != 0 {
            return Err(KrunError::SetConsoleOutput(rc));
        }
        Ok(())
    }

    pub fn add_virtiofs<P: AsRef<Path>>(
        &mut self,
        tag: &str,
        path: P,
        read_only: bool,
    ) -> Result<(), KrunError> {
        let c_tag = CString::new(tag)?;
        let path_str = path.as_ref().to_string_lossy();
        let c_path = CString::new(path_str.as_bytes())?;
        let rc = if read_only {
            // flags = 1 for read-only
            unsafe { ffi::krun_add_virtiofs2(self.ctx_id, c_tag.as_ptr(), c_path.as_ptr(), 1) }
        } else {
            unsafe { ffi::krun_add_virtiofs(self.ctx_id, c_tag.as_ptr(), c_path.as_ptr()) }
        };
        if rc != 0 {
            return Err(KrunError::AddVirtioFs(rc));
        }
        Ok(())
    }

    pub fn add_net_unixstream(
        &mut self,
        socket_path: Option<&str>,
        fd: Option<i32>,
    ) -> Result<(), KrunError> {
        let c_path = match socket_path {
            Some(s) => Some(CString::new(s)?),
            None => None,
        };
        let p_path = c_path.as_ref().map_or(std::ptr::null(), |p| p.as_ptr());
        let raw_fd = fd.unwrap_or(-1);

        let rc = unsafe {
            ffi::krun_add_net_unixstream(
                self.ctx_id,
                p_path,
                raw_fd,
                std::ptr::null_mut(),
                ffi::COMPAT_NET_FEATURES,
                0,
            )
        };
        if rc != 0 {
            return Err(KrunError::AddNetUnixStream(rc));
        }
        Ok(())
    }

    pub fn add_vsock_port<P: AsRef<Path>>(
        &mut self,
        port: u32,
        path: P,
    ) -> Result<(), KrunError> {
        let path_str = path.as_ref().to_string_lossy();
        let c_path = CString::new(path_str.as_bytes())?;
        let rc = unsafe { ffi::krun_add_vsock_port(self.ctx_id, port, c_path.as_ptr()) };
        if rc != 0 {
            return Err(KrunError::AddVsockPort(rc));
        }
        Ok(())
    }

    /// Transitions the current process into the microVM supervisor.
    ///
    /// # Safety / Warning
    /// This call NEVER returns on success. The current process and thread are taken over
    /// by the microVM hypervisor and guest init.
    pub fn start_enter(mut self) -> Result<(), KrunError> {
        self.active = false;
        let rc = unsafe { ffi::krun_start_enter(self.ctx_id) };
        Err(KrunError::StartEnter(rc))
    }
}

impl Drop for KrunContext {
    fn drop(&mut self) {
        if self.active {
            unsafe {
                ffi::krun_free_ctx(self.ctx_id);
            }
        }
    }
}
