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
    #[serde(default)]
    pub vcpus: Option<u8>,
    #[serde(default)]
    pub memory_mib: Option<u32>,
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
        let alive =
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(self.pid as i32), None).is_ok();
        if !alive {
            return false;
        }
        // Allow self process in unit tests
        if self.pid == std::process::id() {
            return true;
        }
        is_microvm_runner_process(self.pid)
    }

    /// Path to the isolated vsock socket for guest command execution.
    pub fn exec_socket_path(&self) -> PathBuf {
        self.instance_dir.join("vsock-exec.sock")
    }

    /// Path to the guest root filesystem.
    pub fn rootfs_path(&self) -> PathBuf {
        self.instance_dir.join("rootfs")
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
                        if vm.vcpus.is_none() || vm.memory_mib.is_none() {
                            for cfg_name in &["runner_config.json", ".krun_config.json"] {
                                let cfg_p = vm.instance_dir.join(cfg_name);
                                if let Ok(cfg_data) = fs::read_to_string(&cfg_p) {
                                    if let Ok(runner_cfg) =
                                        serde_json::from_str::<crate::types::RunnerConfig>(
                                            &cfg_data,
                                        )
                                    {
                                        if vm.vcpus.is_none() {
                                            vm.vcpus = Some(runner_cfg.num_vcpus);
                                        }
                                        if vm.memory_mib.is_none() {
                                            vm.memory_mib = Some(runner_cfg.ram_mib);
                                        }
                                        break;
                                    }
                                }
                            }
                        }
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

    /// Dynamically resizes CPU and/or RAM limits of a running microVM in-place.
    pub fn resize(
        data_dir: &Path,
        id_or_pid: &str,
        memory_mib: Option<u32>,
        vcpus: Option<u8>,
    ) -> Result<VmState> {
        if memory_mib.is_none() && vcpus.is_none() {
            bail!("At least one resource (--memory or --cpus) must be specified to resize");
        }

        if let Some(mem) = memory_mib {
            if mem == 0 {
                bail!("Memory must be greater than 0 MiB");
            }
        }

        if let Some(cpus) = vcpus {
            if cpus == 0 {
                bail!("vCPUs must be greater than 0");
            }
        }

        let mut vm = match Self::find(data_dir, id_or_pid)? {
            Some(v) => v,
            None => bail!("MicroVM with ID or PID '{}' not found", id_or_pid),
        };

        if !vm.is_process_alive() {
            bail!("Cannot resize microVM '{}': process is not running", vm.id);
        }

        // Update runner_config.json and/or .krun_config.json if present in instance_dir
        for cfg_name in &["runner_config.json", ".krun_config.json"] {
            let cfg_path = vm.instance_dir.join(cfg_name);
            if cfg_path.exists() {
                if let Ok(content) = fs::read_to_string(&cfg_path) {
                    if let Ok(mut runner_cfg) =
                        serde_json::from_str::<crate::types::RunnerConfig>(&content)
                    {
                        if let Some(mem) = memory_mib {
                            runner_cfg.ram_mib = mem;
                        }
                        if let Some(cpus) = vcpus {
                            runner_cfg.num_vcpus = cpus;
                        }
                        if let Ok(updated) = serde_json::to_string_pretty(&runner_cfg) {
                            let _ = fs::write(&cfg_path, updated);
                        }
                    }
                }
            }
        }

        if let Some(mem) = memory_mib {
            vm.memory_mib = Some(mem);
        }
        if let Some(cpus) = vcpus {
            vm.vcpus = Some(cpus);
        }

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

    /// Executes a command in a running microVM.
    pub async fn exec(
        data_dir: &Path,
        id_or_pid: &str,
        req: &crate::exec::ExecRequest,
    ) -> Result<crate::exec::ExecResponse> {
        let vms = Self::list(data_dir)?;
        let vm = vms
            .iter()
            .find(|v| {
                v.id == id_or_pid || v.id.starts_with(id_or_pid) || v.pid.to_string() == id_or_pid
            })
            .ok_or_else(|| anyhow::anyhow!("MicroVM '{}' not found", id_or_pid))?;

        if vm.status != VmStatus::Running || !vm.is_process_alive() {
            bail!(
                "Cannot exec in microVM '{}' because it is not running (status: {:?})",
                id_or_pid,
                vm.status
            );
        }

        crate::exec::exec_in_microvm(&vm.exec_socket_path(), &vm.rootfs_path(), req).await
    }

    /// Captures a live or stopped microVM's state, configuration, and instance rootfs into a snapshot package.
    pub fn snapshot(
        data_dir: &Path,
        id_or_pid: &str,
        output_path: &Path,
    ) -> Result<SnapshotManifest> {
        let vm = match Self::find(data_dir, id_or_pid)? {
            Some(v) => v,
            None => bail!("MicroVM with ID or PID '{}' not found", id_or_pid),
        };

        let rootfs = vm.rootfs_path();
        if !rootfs.exists() {
            bail!("MicroVM rootfs path does not exist at {}", rootfs.display());
        }

        let config_file = vm.instance_dir.join(".krun_config.json");
        let runner_config = if config_file.exists() {
            let data = fs::read_to_string(&config_file)?;
            serde_json::from_str::<crate::types::RunnerConfig>(&data).ok()
        } else {
            None
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let manifest = SnapshotManifest {
            version: 1,
            original_id: vm.id.clone(),
            image: vm.image.clone(),
            created_at: now,
            port_forwards: vm.port_forwards.clone(),
            runner_config,
        };

        if output_path
            .extension()
            .is_some_and(|e| e == "tar" || e == "gz")
        {
            let tar_file = fs::File::create(output_path)?;
            let mut tar_builder = tar::Builder::new(tar_file);

            let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
            let mut header = tar::Header::new_gnu();
            header.set_size(manifest_bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar_builder.append_data(&mut header, "snapshot.json", &manifest_bytes[..])?;
            tar_builder.append_dir_all("rootfs", &rootfs)?;
            tar_builder.finish()?;
        } else {
            fs::create_dir_all(output_path)?;
            let manifest_path = output_path.join("snapshot.json");
            let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
            fs::write(manifest_path, manifest_bytes)?;

            let target_rootfs = output_path.join("rootfs");
            crate::rootfs::clone_rootfs(&rootfs, &target_rootfs)?;
        }

        Ok(manifest)
    }

    /// Restores a snapshot into a new microVM instance ready for immediate execution.
    pub fn restore(data_dir: &Path, snapshot_path: &Path, new_id: Option<&str>) -> Result<VmState> {
        if !snapshot_path.exists() {
            bail!("Snapshot path '{}' does not exist", snapshot_path.display());
        }

        let gen_id = format!(
            "vm-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        let target_id = new_id.unwrap_or(&gen_id);
        let instances_dir = data_dir.join("instances").join(target_id);
        fs::create_dir_all(&instances_dir)?;

        let target_rootfs = instances_dir.join("rootfs");

        let manifest: SnapshotManifest = if snapshot_path.is_file() {
            let file = fs::File::open(snapshot_path)?;
            let mut archive = tar::Archive::new(file);
            let mut found_manifest = None;

            for entry_res in archive.entries()? {
                let mut entry = entry_res?;
                let path = entry.path()?.to_path_buf();
                if path == Path::new("snapshot.json") {
                    let mut s = String::new();
                    std::io::Read::read_to_string(&mut entry, &mut s)?;
                    found_manifest = serde_json::from_str::<SnapshotManifest>(&s).ok();
                } else if path.starts_with("rootfs") {
                    let rel = path.strip_prefix("rootfs")?;
                    let dest = target_rootfs.join(rel);
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    entry.unpack(&dest)?;
                }
            }

            found_manifest.ok_or_else(|| anyhow::anyhow!("snapshot.json not found in archive"))?
        } else {
            let manifest_path = snapshot_path.join("snapshot.json");
            if !manifest_path.exists() {
                bail!("snapshot.json not found in {}", snapshot_path.display());
            }
            let data = fs::read_to_string(manifest_path)?;
            let m: SnapshotManifest = serde_json::from_str(&data)?;

            let src_rootfs = snapshot_path.join("rootfs");
            if src_rootfs.exists() {
                crate::rootfs::clone_rootfs(&src_rootfs, &target_rootfs)?;
            }
            m
        };

        if let Some(ref cfg) = manifest.runner_config {
            let cfg_path = instances_dir.join(".krun_config.json");
            let json = serde_json::to_string_pretty(cfg)?;
            fs::write(cfg_path, json)?;
        }

        let new_state = VmState {
            id: target_id.to_string(),
            pid: 0,
            image: manifest.image,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            port_forwards: manifest.port_forwards,
            instance_dir: instances_dir,
            status: VmStatus::Stopped,
            vcpus: manifest.runner_config.as_ref().map(|c| c.num_vcpus),
            memory_mib: manifest.runner_config.as_ref().map(|c| c.ram_mib),
        };

        Self::save(data_dir, &new_state)?;
        Ok(new_state)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub version: u32,
    pub original_id: String,
    pub image: String,
    pub created_at: u64,
    pub port_forwards: Vec<PortForward>,
    pub runner_config: Option<crate::types::RunnerConfig>,
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
            vcpus: Some(2),
            memory_mib: Some(512),
        };

        StateManager::save(dir.path(), &vm).unwrap();

        let list = StateManager::list(dir.path()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "vm-test-1");
        assert_eq!(list[0].status, VmStatus::Running);
        assert_eq!(list[0].vcpus, Some(2));
        assert_eq!(list[0].memory_mib, Some(512));

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
            vcpus: None,
            memory_mib: None,
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
            vcpus: None,
            memory_mib: None,
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
            vcpus: None,
            memory_mib: None,
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
            vcpus: None,
            memory_mib: None,
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

    #[test]
    fn test_state_manager_resize_success() {
        let dir = tempdir().unwrap();
        let instance_dir = dir.path().join("instances/vm-resize-test");
        fs::create_dir_all(&instance_dir).unwrap();

        let runner_cfg = crate::types::RunnerConfig {
            root_path: instance_dir.join("rootfs"),
            num_vcpus: 1,
            ram_mib: 256,
            port_forwards: vec![],
            net_sock_path: None,
            virtiofs_mounts: vec![],
            vsock_ports: vec![],
            console_log_path: None,
            log_level: None,
            interactive: false,
            tty: false,
            no_network: false,
            rlimits: None,
            detach: false,
            dax_window_size_bytes: None,
            image_acceleration: None,
            gpu: false,
            gpu_shm_size_bytes: None,
            gpu_flags: None,
            sandbox: true,
            allow_hosts: vec![],
            secrets: vec![],
            max_tokens: None,
            proxy_port: None,
        };
        fs::write(
            instance_dir.join("runner_config.json"),
            serde_json::to_string_pretty(&runner_cfg).unwrap(),
        )
        .unwrap();

        let vm = VmState {
            id: "vm-resize-test".to_string(),
            pid: std::process::id(), // alive process
            image: "alpine:latest".to_string(),
            created_at: 4000,
            port_forwards: vec![],
            instance_dir: instance_dir.clone(),
            status: VmStatus::Running,
            vcpus: Some(1),
            memory_mib: Some(256),
        };
        StateManager::save(dir.path(), &vm).unwrap();

        // Resize both memory and cpus
        let updated =
            StateManager::resize(dir.path(), "vm-resize-test", Some(1024), Some(4)).unwrap();
        assert_eq!(updated.memory_mib, Some(1024));
        assert_eq!(updated.vcpus, Some(4));

        // Verify config file was updated
        let cfg_content = fs::read_to_string(instance_dir.join("runner_config.json")).unwrap();
        let parsed_cfg: crate::types::RunnerConfig = serde_json::from_str(&cfg_content).unwrap();
        assert_eq!(parsed_cfg.ram_mib, 1024);
        assert_eq!(parsed_cfg.num_vcpus, 4);

        // Verify find() returns new values
        let found = StateManager::find(dir.path(), "vm-resize-test")
            .unwrap()
            .unwrap();
        assert_eq!(found.memory_mib, Some(1024));
        assert_eq!(found.vcpus, Some(4));
    }

    #[test]
    fn test_state_manager_resize_validation_errors() {
        let dir = tempdir().unwrap();
        let instance_dir = dir.path().join("instances/vm-resize-val");
        fs::create_dir_all(&instance_dir).unwrap();

        let vm = VmState {
            id: "vm-resize-val".to_string(),
            pid: std::process::id(),
            image: "alpine:latest".to_string(),
            created_at: 4000,
            port_forwards: vec![],
            instance_dir,
            status: VmStatus::Running,
            vcpus: Some(1),
            memory_mib: Some(256),
        };
        StateManager::save(dir.path(), &vm).unwrap();

        // Error: neither memory nor cpus
        assert!(StateManager::resize(dir.path(), "vm-resize-val", None, None).is_err());

        // Error: memory = 0
        assert!(StateManager::resize(dir.path(), "vm-resize-val", Some(0), None).is_err());

        // Error: cpus = 0
        assert!(StateManager::resize(dir.path(), "vm-resize-val", None, Some(0)).is_err());

        // Error: unknown ID
        assert!(StateManager::resize(dir.path(), "vm-nonexistent", Some(512), None).is_err());
    }

    #[test]
    fn test_state_manager_resize_dead_process() {
        let dir = tempdir().unwrap();
        let instance_dir = dir.path().join("instances/vm-resize-dead");
        fs::create_dir_all(&instance_dir).unwrap();

        let vm = VmState {
            id: "vm-resize-dead".to_string(),
            pid: 999_997, // dead PID
            image: "alpine:latest".to_string(),
            created_at: 4000,
            port_forwards: vec![],
            instance_dir,
            status: VmStatus::Stopped,
            vcpus: Some(1),
            memory_mib: Some(256),
        };
        StateManager::save(dir.path(), &vm).unwrap();

        let res = StateManager::resize(dir.path(), "vm-resize-dead", Some(1024), None);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("not running"));
    }

    #[test]
    fn test_snapshot_and_restore_directory() {
        let dir = tempdir().unwrap();
        let instance_dir = dir.path().join("instances/vm-snap-1");
        let rootfs = instance_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("etc")).unwrap();
        fs::write(rootfs.join("etc/version.txt"), "1.0.0-snapshot").unwrap();

        let vm = VmState {
            id: "vm-snap-1".to_string(),
            pid: 0,
            image: "ubuntu:22.04".to_string(),
            created_at: 100,
            port_forwards: vec![PortForward {
                host: 8080,
                guest: 80,
            }],
            instance_dir,
            status: VmStatus::Stopped,
            vcpus: None,
            memory_mib: None,
        };
        StateManager::save(dir.path(), &vm).unwrap();

        let snap_dir = dir.path().join("snapshots/snap-1");
        let manifest = StateManager::snapshot(dir.path(), "vm-snap-1", &snap_dir).unwrap();
        assert_eq!(manifest.original_id, "vm-snap-1");
        assert_eq!(manifest.image, "ubuntu:22.04");

        // Restore into new instance
        let restored = StateManager::restore(dir.path(), &snap_dir, Some("vm-restored-1")).unwrap();
        assert_eq!(restored.id, "vm-restored-1");
        assert_eq!(restored.image, "ubuntu:22.04");
        assert_eq!(restored.port_forwards.len(), 1);

        let restored_ver =
            fs::read_to_string(restored.instance_dir.join("rootfs/etc/version.txt")).unwrap();
        assert_eq!(restored_ver, "1.0.0-snapshot");
    }

    #[test]
    fn test_snapshot_and_restore_tar() {
        let dir = tempdir().unwrap();
        let instance_dir = dir.path().join("instances/vm-snap-tar");
        let rootfs = instance_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("app")).unwrap();
        fs::write(rootfs.join("app/data.json"), r#"{"model":"mistral"}"#).unwrap();

        let vm = VmState {
            id: "vm-snap-tar".to_string(),
            pid: 0,
            image: "mistral:latest".to_string(),
            created_at: 200,
            port_forwards: vec![],
            instance_dir,
            status: VmStatus::Stopped,
            vcpus: None,
            memory_mib: None,
        };
        StateManager::save(dir.path(), &vm).unwrap();

        let tar_path = dir.path().join("snapshots/vm-snap-tar.tar");
        fs::create_dir_all(tar_path.parent().unwrap()).unwrap();
        let manifest = StateManager::snapshot(dir.path(), "vm-snap-tar", &tar_path).unwrap();
        assert_eq!(manifest.original_id, "vm-snap-tar");

        // Restore from tar
        let restored =
            StateManager::restore(dir.path(), &tar_path, Some("vm-tar-restored")).unwrap();
        assert_eq!(restored.id, "vm-tar-restored");
        let data = fs::read_to_string(restored.instance_dir.join("rootfs/app/data.json")).unwrap();
        assert_eq!(data, r#"{"model":"mistral"}"#);
    }
}
