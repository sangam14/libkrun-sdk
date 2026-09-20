use crate::PortForward;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VmStatus {
    Running,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmState {
    pub id: String,
    pub pid: u32,
    pub image: String,
    pub created_at: u64,
    pub port_forwards: Vec<PortForward>,
    pub instance_dir: PathBuf,
    pub status: VmStatus,
}

#[cfg(target_os = "macos")]
fn is_microvm_runner_process(pid: u32) -> bool {
    extern "C" {
        fn proc_pidpath(
            pid: libc::c_int,
            buffer: *mut libc::c_void,
            buffersize: u32,
        ) -> libc::c_int;
    }
    let mut buf = [0u8; 1024];
    let ret = unsafe {
        proc_pidpath(
            pid as libc::c_int,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len() as u32,
        )
    };
    if ret <= 0 {
        return false;
    }
    let path_str = String::from_utf8_lossy(&buf[..ret as usize]);
    path_str.contains("microvm-runner")
}

#[cfg(target_os = "linux")]
fn is_microvm_runner_process(pid: u32) -> bool {
    if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
        if comm.trim().contains("microvm-runner") {
            return true;
        }
    }
    if let Ok(cmdline) = std::fs::read_to_string(format!("/proc/{pid}/cmdline")) {
        return cmdline.contains("microvm-runner");
    }
    false
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn is_microvm_runner_process(_pid: u32) -> bool {
    true
}

impl VmState {
    pub fn is_process_alive(&self) -> bool {
        if self.pid == 0 {
            return false;
        }
        let alive = unsafe { libc::kill(self.pid as i32, 0) == 0 };
        if !alive {
            return false;
        }
        // Allow self process in unit tests
        if self.pid == std::process::id() {
            return true;
        }
        is_microvm_runner_process(self.pid)
    }
}

pub struct StateManager;

impl StateManager {
    pub fn state_dir(data_dir: &Path) -> PathBuf {
        data_dir.join("state")
    }

    pub fn save(data_dir: &Path, state: &VmState) -> Result<PathBuf> {
        let dir = Self::state_dir(data_dir);
        fs::create_dir_all(&dir)?;
        let file_path = dir.join(format!("{}.json", state.id));
        let data = serde_json::to_string_pretty(state)?;
        fs::write(&file_path, data)?;
        Ok(file_path)
    }

    pub fn remove(data_dir: &Path, id: &str) -> Result<()> {
        let file_path = Self::state_dir(data_dir).join(format!("{}.json", id));
        if file_path.exists() {
            fs::remove_file(file_path)?;
        }
        Ok(())
    }

    pub fn list(data_dir: &Path) -> Result<Vec<VmState>> {
        let dir = Self::state_dir(data_dir);
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut vms = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(mut vm) = serde_json::from_str::<VmState>(&content) {
                        vm.status = if vm.is_process_alive() {
                            if vm.status == VmStatus::Paused {
                                VmStatus::Paused
                            } else {
                                VmStatus::Running
                            }
                        } else {
                            VmStatus::Stopped
                        };
                        vms.push(vm);
                    }
                }
            }
        }

        vms.sort_by_key(|a| std::cmp::Reverse(a.created_at));
        Ok(vms)
    }

    pub fn find(data_dir: &Path, id_or_pid: &str) -> Result<Option<VmState>> {
        let vms = Self::list(data_dir)?;
        let target = vms.into_iter().find(|vm| {
            vm.id == id_or_pid || vm.id.starts_with(id_or_pid) || vm.pid.to_string() == id_or_pid
        });
        Ok(target)
    }

    pub fn pause(data_dir: &Path, id_or_pid: &str) -> Result<VmState> {
        let mut vm = match Self::find(data_dir, id_or_pid)? {
            Some(v) => v,
            None => bail!("MicroVM with ID or PID '{}' not found", id_or_pid),
        };

        if !vm.is_process_alive() {
            bail!("Cannot pause microVM '{}': process is not running", vm.id);
        }

        if vm.status == VmStatus::Paused {
            return Ok(vm);
        }

        unsafe {
            if libc::kill(vm.pid as i32, libc::SIGSTOP) != 0 {
                bail!(
                    "Failed to send SIGSTOP to process {}: {}",
                    vm.pid,
                    std::io::Error::last_os_error()
                );
            }
        }

        vm.status = VmStatus::Paused;
        Self::save(data_dir, &vm)?;
        Ok(vm)
    }

    pub fn resume(data_dir: &Path, id_or_pid: &str) -> Result<VmState> {
        let mut vm = match Self::find(data_dir, id_or_pid)? {
            Some(v) => v,
            None => bail!("MicroVM with ID or PID '{}' not found", id_or_pid),
        };

        if !vm.is_process_alive() {
            bail!("Cannot resume microVM '{}': process is not running", vm.id);
        }

        unsafe {
            if libc::kill(vm.pid as i32, libc::SIGCONT) != 0 {
                bail!(
                    "Failed to send SIGCONT to process {}: {}",
                    vm.pid,
                    std::io::Error::last_os_error()
                );
            }
        }

        vm.status = VmStatus::Running;
        Self::save(data_dir, &vm)?;
        Ok(vm)
    }

    pub fn delete(data_dir: &Path, id_or_pid: &str, force: bool) -> Result<VmState> {
        let vm = match Self::find(data_dir, id_or_pid)? {
            Some(vm) => vm,
            None => bail!("MicroVM with ID or PID '{}' not found", id_or_pid),
        };

        if vm.is_process_alive() {
            if !force {
                bail!(
                    "MicroVM '{}' (PID {}) is currently running. Stop it first or use --force (-f) to remove.",
                    vm.id,
                    vm.pid
                );
            }
            unsafe {
                libc::kill(vm.pid as i32, libc::SIGKILL);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        Self::remove(data_dir, &vm.id)?;
        if vm.instance_dir.exists() {
            let _ = fs::remove_dir_all(&vm.instance_dir);
        }

        Ok(vm)
    }

    pub fn stop(data_dir: &Path, id_or_pid: &str) -> Result<()> {
        let vm = match Self::find(data_dir, id_or_pid)? {
            Some(v) => v,
            None => bail!("MicroVM with ID or PID '{}' not found", id_or_pid),
        };

        if vm.is_process_alive() {
            unsafe {
                libc::kill(vm.pid as i32, libc::SIGTERM);
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            if vm.is_process_alive() {
                unsafe {
                    libc::kill(vm.pid as i32, libc::SIGKILL);
                }
            }
        }
        Self::remove(data_dir, &vm.id)?;
        if vm.instance_dir.exists() {
            let _ = fs::remove_dir_all(&vm.instance_dir);
        }
        Ok(())
    }

    pub fn prune(data_dir: &Path) -> Result<PruneSummary> {
        let mut pruned_instances = 0;
        let mut pruned_layers = 0;

        // 1. Clean dead state files & dead instance directories
        let vms = Self::list(data_dir)?;
        for vm in vms {
            if !vm.is_process_alive() {
                Self::remove(data_dir, &vm.id)?;
                if vm.instance_dir.exists() {
                    let _ = fs::remove_dir_all(&vm.instance_dir);
                }
                pruned_instances += 1;
            }
        }

        // Clean orphaned directories in instances/
        let instances_dir = data_dir.join("instances");
        if instances_dir.exists() {
            for entry in fs::read_dir(&instances_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    let id = entry.file_name().to_string_lossy().to_string();
                    let state_file = Self::state_dir(data_dir).join(format!("{}.json", id));
                    if !state_file.exists() {
                        let _ = fs::remove_dir_all(&path);
                        pruned_instances += 1;
                    }
                }
            }
        }

        // 2. Clean temporary staging directories
        let staging_dir = data_dir.join("staging");
        if staging_dir.exists() {
            let _ = fs::remove_dir_all(&staging_dir);
            pruned_layers += 1;
        }

        Ok(PruneSummary {
            pruned_instances,
            pruned_layers,
        })
    }

    /// Copies a host file or directory into a microVM's rootfs.
    pub fn copy_into(
        data_dir: &Path,
        id_or_pid: &str,
        src_host: &Path,
        guest_rel_path: &str,
    ) -> Result<()> {
        let vms = Self::list(data_dir)?;
        let vm = vms
            .iter()
            .find(|v| {
                v.id == id_or_pid || v.id.starts_with(id_or_pid) || v.pid.to_string() == id_or_pid
            })
            .ok_or_else(|| anyhow::anyhow!("MicroVM '{}' not found", id_or_pid))?;

        let clean_guest_path = guest_rel_path.trim_start_matches('/');
        let target_guest = vm.instance_dir.join("rootfs").join(clean_guest_path);

        if src_host.is_dir() {
            copy_dir_all(src_host, &target_guest)?;
        } else {
            if let Some(parent) = target_guest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(src_host, &target_guest)?;
        }
        Ok(())
    }

    /// Copies a file or directory from a microVM's rootfs to the host.
    pub fn copy_from(
        data_dir: &Path,
        id_or_pid: &str,
        guest_rel_path: &str,
        dst_host: &Path,
    ) -> Result<()> {
        let vms = Self::list(data_dir)?;
        let vm = vms
            .iter()
            .find(|v| {
                v.id == id_or_pid || v.id.starts_with(id_or_pid) || v.pid.to_string() == id_or_pid
            })
            .ok_or_else(|| anyhow::anyhow!("MicroVM '{}' not found", id_or_pid))?;

        let clean_guest_path = guest_rel_path.trim_start_matches('/');
        let src_guest = vm.instance_dir.join("rootfs").join(clean_guest_path);

        if !src_guest.exists() {
            bail!(
                "Path '{}' not found inside microVM '{}'",
                guest_rel_path,
                id_or_pid
            );
        }

        if src_guest.is_dir() {
            copy_dir_all(&src_guest, dst_host)?;
        } else {
            if let Some(parent) = dst_host.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&src_guest, dst_host)?;
        }
        Ok(())
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_p = entry.path();
        let dst_p = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_all(&src_p, &dst_p)?;
        } else {
            fs::copy(&src_p, &dst_p)?;
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct PruneSummary {
    pub pruned_instances: usize,
    pub pruned_layers: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_state_manager_save_list_remove() {
        let dir = tempdir().unwrap();
        let vm = VmState {
            id: "vm-test-1".to_string(),
            pid: std::process::id(), // alive current process
            image: "alpine:latest".to_string(),
            created_at: 1000,
            port_forwards: vec![],
            instance_dir: dir.path().join("instances/vm-test-1"),
            status: VmStatus::Running,
        };

        StateManager::save(dir.path(), &vm).unwrap();

        let list = StateManager::list(dir.path()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "vm-test-1");
        assert_eq!(list[0].status, VmStatus::Running);

        StateManager::remove(dir.path(), "vm-test-1").unwrap();
        let list = StateManager::list(dir.path()).unwrap();
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn test_copy_into_and_from() {
        let dir = tempdir().unwrap();
        let instance_dir = dir.path().join("instances/vm-copy-test");
        fs::create_dir_all(instance_dir.join("rootfs/etc")).unwrap();

        let vm = VmState {
            id: "vm-copy-test".to_string(),
            pid: std::process::id(),
            image: "alpine:latest".to_string(),
            created_at: 1000,
            port_forwards: vec![],
            instance_dir: instance_dir.clone(),
            status: VmStatus::Running,
        };
        StateManager::save(dir.path(), &vm).unwrap();

        // 1. Create a host source file
        let host_src = dir.path().join("host_file.txt");
        fs::write(&host_src, "hello microvm").unwrap();

        // 2. Copy host file into guest /etc/app.conf
        StateManager::copy_into(dir.path(), "vm-copy-test", &host_src, "/etc/app.conf").unwrap();
        let guest_file = instance_dir.join("rootfs/etc/app.conf");
        assert!(guest_file.exists());
        assert_eq!(fs::read_to_string(&guest_file).unwrap(), "hello microvm");

        // 3. Copy guest file back to host
        let host_dst = dir.path().join("recovered.txt");
        StateManager::copy_from(dir.path(), "vm-copy-test", "/etc/app.conf", &host_dst).unwrap();
        assert!(host_dst.exists());
        assert_eq!(fs::read_to_string(&host_dst).unwrap(), "hello microvm");
    }

    #[test]
    fn test_pid_reuse_protection() {
        // PID 1 is launchd/init, alive on OS but NOT a microvm-runner process
        let vm = VmState {
            id: "vm-unrelated-test".to_string(),
            pid: 1,
            image: "alpine:latest".to_string(),
            created_at: 1000,
            port_forwards: vec![],
            instance_dir: PathBuf::from("/tmp/instances/vm-unrelated-test"),
            status: VmStatus::Running,
        };
        // Should detect it is not a microvm-runner and return false
        assert!(!vm.is_process_alive());
    }

    #[test]
    fn test_state_manager_find_and_delete() {
        let dir = tempdir().unwrap();
        let instance_dir = dir.path().join("instances/vm-find-del");
        fs::create_dir_all(&instance_dir).unwrap();

        let vm = VmState {
            id: "vm-find-del".to_string(),
            pid: 999_999, // dead PID
            image: "alpine:latest".to_string(),
            created_at: 2000,
            port_forwards: vec![],
            instance_dir: instance_dir.clone(),
            status: VmStatus::Stopped,
        };
        StateManager::save(dir.path(), &vm).unwrap();

        let found = StateManager::find(dir.path(), "vm-find-del").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "vm-find-del");

        // Test delete on stopped VM
        let deleted = StateManager::delete(dir.path(), "vm-find-del", false).unwrap();
        assert_eq!(deleted.id, "vm-find-del");
        assert!(!instance_dir.exists());

        // Verify it is gone
        let not_found = StateManager::find(dir.path(), "vm-find-del").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_state_manager_pause_and_resume_validation() {
        let dir = tempdir().unwrap();
        let instance_dir = dir.path().join("instances/vm-pause-test");
        fs::create_dir_all(&instance_dir).unwrap();

        let vm = VmState {
            id: "vm-pause-test".to_string(),
            pid: 999_998, // dead PID
            image: "alpine:latest".to_string(),
            created_at: 3000,
            port_forwards: vec![],
            instance_dir: instance_dir.clone(),
            status: VmStatus::Stopped,
        };
        StateManager::save(dir.path(), &vm).unwrap();

        // Attempting to pause dead VM fails
        let pause_res = StateManager::pause(dir.path(), "vm-pause-test");
        assert!(pause_res.is_err());
        assert!(pause_res.unwrap_err().to_string().contains("not running"));

        // Attempting to resume dead VM fails
        let resume_res = StateManager::resume(dir.path(), "vm-pause-test");
        assert!(resume_res.is_err());
        assert!(resume_res.unwrap_err().to_string().contains("not running"));
    }
}
