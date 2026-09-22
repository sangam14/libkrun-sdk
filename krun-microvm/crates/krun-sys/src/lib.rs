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
    #[error("Failed to configure virtio-gpu device, error code: {0}")]
    SetGpuOptions(i32),
    #[error("Failed to add disk, error code: {0}")]
    AddDisk(i32),
    #[error("Failed to set kernel, error code: {0}")]
    SetKernel(i32),
    #[error("Failed to set firmware, error code: {0}")]
    SetFirmware(i32),
    #[error("Failed to disable implicit console, error code: {0}")]
    DisableImplicitConsole(i32),
    #[error("Failed to add serial console default, error code: {0}")]
    AddSerialConsoleDefault(i32),
    #[error("Failed to start VM via krun_start_enter, error code: {0}")]
    StartEnter(i32),
    #[error("Nul byte in string argument: {0}")]
    NulError(#[from] std::ffi::NulError),
}

pub mod kernel_formats {
    pub const KRUN_KERNEL_FORMAT_ELF: u32 = 0;
    pub const KRUN_KERNEL_FORMAT_RAW: u32 = 1;
    pub const KRUN_KERNEL_FORMAT_PE_GZ: u32 = 2;
    pub const KRUN_KERNEL_FORMAT_IMAGE_BZ2: u32 = 3;
    pub const KRUN_KERNEL_FORMAT_IMAGE_GZ: u32 = 4;
    pub const KRUN_KERNEL_FORMAT_IMAGE_ZSTD: u32 = 5;
}

pub mod virgl_flags {
    pub const VIRGLRENDERER_USE_EGL: u32 = 1 << 0;
    pub const VIRGLRENDERER_THREAD_SYNC: u32 = 1 << 1;
    pub const VIRGLRENDERER_USE_GLX: u32 = 1 << 2;
    pub const VIRGLRENDERER_USE_SURFACELESS: u32 = 1 << 3;
    pub const VIRGLRENDERER_USE_GLES: u32 = 1 << 4;
    pub const VIRGLRENDERER_USE_EXTERNAL_BLOB: u32 = 1 << 5;
    pub const VIRGLRENDERER_VENUS: u32 = 1 << 6;
    pub const VIRGLRENDERER_NO_VIRGL: u32 = 1 << 7;
    pub const VIRGLRENDERER_USE_ASYNC_FENCE_CB: u32 = 1 << 8;
    pub const VIRGLRENDERER_RENDER_SERVER: u32 = 1 << 9;
    pub const VIRGLRENDERER_DRM: u32 = 1 << 10;
}

pub mod ffi {
    use super::*;

    pub const COMPAT_NET_FEATURES: u32 =
        (1 << 0) | (1 << 1) | (1 << 7) | (1 << 10) | (1 << 11) | (1 << 14);

    extern "C" {
        pub fn krun_create_ctx() -> i32;
        pub fn krun_free_ctx(ctx_id: u32) -> i32;
        pub fn krun_set_vm_config(ctx_id: u32, num_vcpus: u8, ram_mib: u32) -> i32;
        pub fn krun_add_disk(
            ctx_id: u32,
            block_id: *const c_char,
            disk_path: *const c_char,
            read_only: bool,
        ) -> i32;
        pub fn krun_set_kernel(
            ctx_id: u32,
            kernel_path: *const c_char,
            kernel_format: u32,
            initramfs: *const c_char,
            cmdline: *const c_char,
        ) -> i32;
        pub fn krun_set_firmware(ctx_id: u32, firmware_path: *const c_char) -> i32;
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
        pub fn krun_disable_implicit_console(ctx_id: u32) -> i32;
        pub fn krun_add_serial_console_default(ctx_id: u32, input_fd: i32, output_fd: i32) -> i32;
        pub fn krun_set_log_level(level: u32) -> i32;
        pub fn krun_set_rlimits(ctx_id: u32, rlimits: *const c_char) -> i32;
        pub fn krun_set_gpu_options(ctx_id: u32, virgl_flags: u32) -> i32;
        pub fn krun_set_gpu_options2(ctx_id: u32, virgl_flags: u32, shm_size: u64) -> i32;
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

#[cfg(unix)]
fn path_to_cstring(path: &Path) -> Result<CString, KrunError> {
    use std::os::unix::ffi::OsStrExt;
    CString::new(path.as_os_str().as_bytes()).map_err(KrunError::NulError)
}

#[cfg(not(unix))]
fn path_to_cstring(path: &Path) -> Result<CString, KrunError> {
    CString::new(path.to_string_lossy().as_bytes()).map_err(KrunError::NulError)
}

/// A handle to a libkrun microVM context.
///
/// # Thread Safety
/// `KrunContext` implements `Send` so it can be moved between threads, but explicitly
/// opts out of `Sync` (via `PhantomData<*const ()>`) because libkrun's C internal context
/// operations are not thread-safe.
pub struct KrunContext {
    ctx_id: u32,
    active: bool,
    _not_sync: std::marker::PhantomData<*const ()>,
}

unsafe impl Send for KrunContext {}

impl KrunContext {
    pub fn create() -> Result<Self, KrunError> {
        let rc = unsafe { ffi::krun_create_ctx() };
        if rc < 0 {
            return Err(KrunError::CreateContext(rc));
        }
        Ok(Self {
            ctx_id: rc as u32,
            active: true,
            _not_sync: std::marker::PhantomData,
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
        let c_path = path_to_cstring(path.as_ref())?;
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

    pub fn set_exec(
        &mut self,
        exec_path: &str,
        argv: &[String],
        envp: &[String],
    ) -> Result<(), KrunError> {
        let c_exec = CString::new(exec_path)?;

        let mut c_argv = Vec::new();
        for arg in argv {
            c_argv.push(CString::new(arg.as_str())?);
        }
        let mut argv_ptrs: Vec<*const c_char> = c_argv.iter().map(|s| s.as_ptr()).collect();
        argv_ptrs.push(std::ptr::null());

        let mut c_envp = Vec::new();
        for env in envp {
            c_envp.push(CString::new(env.as_str())?);
        }
        let mut envp_ptrs: Vec<*const c_char> = c_envp.iter().map(|s| s.as_ptr()).collect();
        envp_ptrs.push(std::ptr::null());

        let rc = unsafe {
            ffi::krun_set_exec(
                self.ctx_id,
                c_exec.as_ptr(),
                argv_ptrs.as_ptr(),
                envp_ptrs.as_ptr(),
            )
        };
        if rc != 0 {
            return Err(KrunError::SetExec(rc));
        }
        Ok(())
    }

    pub fn set_env(&mut self, envp: &[String]) -> Result<(), KrunError> {
        let mut c_envp = Vec::new();
        for env in envp {
            c_envp.push(CString::new(env.as_str())?);
        }
        let mut envp_ptrs: Vec<*const c_char> = c_envp.iter().map(|s| s.as_ptr()).collect();
        envp_ptrs.push(std::ptr::null());

        let rc = unsafe { ffi::krun_set_env(self.ctx_id, envp_ptrs.as_ptr()) };
        if rc != 0 {
            return Err(KrunError::SetEnv(rc));
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

    /// Enables and configures virtio-gpu device with virglrenderer / Venus flags.
    ///
    /// If `shm_size_bytes` is provided, specifies the shared memory host window (vRAM)
    /// for zero-copy buffer sharing between host GPU and guest workloads.
    pub fn set_gpu_options(
        &mut self,
        virgl_flags: u32,
        shm_size_bytes: Option<u64>,
    ) -> Result<(), KrunError> {
        let rc = if let Some(shm) = shm_size_bytes {
            unsafe { ffi::krun_set_gpu_options2(self.ctx_id, virgl_flags, shm) }
        } else {
            unsafe { ffi::krun_set_gpu_options(self.ctx_id, virgl_flags) }
        };
        if rc != 0 {
            return Err(KrunError::SetGpuOptions(rc));
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
        let c_path = path_to_cstring(path.as_ref())?;
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
        let c_path = path_to_cstring(path.as_ref())?;
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
        let c_path = socket_path.map(CString::new).transpose()?;
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

    pub fn add_vsock_port<P: AsRef<Path>>(&mut self, port: u32, path: P) -> Result<(), KrunError> {
        let c_path = path_to_cstring(path.as_ref())?;
        let rc = unsafe { ffi::krun_add_vsock_port(self.ctx_id, port, c_path.as_ptr()) };
        if rc != 0 {
            return Err(KrunError::AddVsockPort(rc));
        }
        Ok(())
    }

    pub fn add_disk<P: AsRef<Path>>(
        &mut self,
        block_id: &str,
        disk_path: P,
        read_only: bool,
    ) -> Result<(), KrunError> {
        let c_id = CString::new(block_id)?;
        let c_path = path_to_cstring(disk_path.as_ref())?;
        let rc =
            unsafe { ffi::krun_add_disk(self.ctx_id, c_id.as_ptr(), c_path.as_ptr(), read_only) };
        if rc != 0 {
            return Err(KrunError::AddDisk(rc));
        }
        Ok(())
    }

    pub fn set_kernel<P: AsRef<Path>>(
        &mut self,
        kernel_path: P,
        kernel_format: u32,
        initramfs: Option<&Path>,
        cmdline: Option<&str>,
    ) -> Result<(), KrunError> {
        let c_kpath = path_to_cstring(kernel_path.as_ref())?;
        let c_initrd = match initramfs {
            Some(p) => Some(path_to_cstring(p)?),
            None => None,
        };
        let c_cmdline = match cmdline {
            Some(s) => Some(CString::new(s)?),
            None => None,
        };
        let initrd_ptr = c_initrd.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
        let cmdline_ptr = c_cmdline.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());

        let rc = unsafe {
            ffi::krun_set_kernel(
                self.ctx_id,
                c_kpath.as_ptr(),
                kernel_format,
                initrd_ptr,
                cmdline_ptr,
            )
        };
        if rc != 0 {
            return Err(KrunError::SetKernel(rc));
        }
        Ok(())
    }

    pub fn set_firmware<P: AsRef<Path>>(&mut self, firmware_path: P) -> Result<(), KrunError> {
        let c_path = path_to_cstring(firmware_path.as_ref())?;
        let rc = unsafe { ffi::krun_set_firmware(self.ctx_id, c_path.as_ptr()) };
        if rc != 0 {
            return Err(KrunError::SetFirmware(rc));
        }
        Ok(())
    }

    pub fn disable_implicit_console(&mut self) -> Result<(), KrunError> {
        let rc = unsafe { ffi::krun_disable_implicit_console(self.ctx_id) };
        if rc != 0 {
            return Err(KrunError::DisableImplicitConsole(rc));
        }
        Ok(())
    }

    pub fn add_serial_console_default(
        &mut self,
        input_fd: i32,
        output_fd: i32,
    ) -> Result<(), KrunError> {
        let rc = unsafe { ffi::krun_add_serial_console_default(self.ctx_id, input_fd, output_fd) };
        if rc != 0 {
            return Err(KrunError::AddSerialConsoleDefault(rc));
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
        if rc == 0 {
            Ok(())
        } else {
            Err(KrunError::StartEnter(rc))
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_krun_context_send_not_sync() {
        fn assert_send<T: Send>() {}
        assert_send::<KrunContext>();

        // Verify that KrunContext does NOT implement Sync at compile time
        // by testing that an attempt to pass &KrunContext across threads fails if required.
        // The presence of PhantomData<*const ()> removes Sync.
    }

    #[test]
    fn test_path_to_cstring_valid() {
        let p = PathBuf::from("/var/run/krun.sock");
        let c = path_to_cstring(&p).expect("valid path should convert");
        assert_eq!(c.to_bytes_with_nul(), b"/var/run/krun.sock\0");
    }

    #[test]
    fn test_path_to_cstring_rejects_nul() {
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            let bad_path = OsStr::from_bytes(b"/tmp/foo\0bar");
            let p = Path::new(bad_path);
            assert!(path_to_cstring(p).is_err());
        }
    }
}
