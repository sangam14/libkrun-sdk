use crate::types::VirtioFsMount;
use anyhow::{bail, Context, Result};
use oci_spec::runtime::{LinuxNamespaceType, Spec};
use std::path::{Path, PathBuf};

/// Represents an unpacked OCI runtime bundle (config.json + rootfs) created by containerd, Docker, or runc.
#[derive(Debug, Clone)]
pub struct OciBundle {
    pub bundle_dir: PathBuf,
    pub spec: Spec,
    pub rootfs_path: PathBuf,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
    pub workdir: Option<String>,
    pub rlimits: Option<String>,
    pub netns_path: Option<PathBuf>,
    pub vcpus: Option<u8>,
    pub ram_mib: Option<u32>,
    pub virtiofs_mounts: Vec<VirtioFsMount>,
    pub file_mounts: Vec<(PathBuf, PathBuf)>, // (host_source, guest_destination)
}

impl OciBundle {
    /// Loads and parses an OCI bundle from a directory containing config.json and rootfs/.
    pub fn load(bundle_dir: impl AsRef<Path>) -> Result<Self> {
        let bundle_dir = bundle_dir.as_ref().to_path_buf();
        let config_file = bundle_dir.join("config.json");
        if !config_file.exists() {
            bail!("OCI bundle config.json not found at {}", config_file.display());
        }

        let spec = Spec::load(&config_file)
            .with_context(|| format!("Failed to parse OCI spec at {}", config_file.display()))?;

        // 1. Resolve rootfs path
        let raw_rootfs = spec
            .root()
            .as_ref()
            .map(|r| r.path().clone())
            .unwrap_or_else(|| PathBuf::from("rootfs"));

        let rootfs_path = if raw_rootfs.is_absolute() {
            raw_rootfs
        } else {
            bundle_dir.join(raw_rootfs)
        };

        if !rootfs_path.exists() {
            bail!("OCI bundle rootfs does not exist at {}", rootfs_path.display());
        }

        // 2. Resolve process args, env, cwd, rlimits
        let (cmd, env, workdir, rlimits) = if let Some(ref proc) = spec.process() {
            let cmd = proc.args().clone().unwrap_or_default();
            let env = proc.env().clone().unwrap_or_default();
            let cwd = proc.cwd().to_string_lossy().to_string();
            let workdir = if cwd.is_empty() { None } else { Some(cwd) };

            let rlimits_str = proc.rlimits().as_ref().map(|lims| {
                lims.iter()
                    .map(|l| format!("{:?}={}:{}", l.typ(), l.soft(), l.hard()))
                    .collect::<Vec<_>>()
                    .join(";")
            });

            (cmd, env, workdir, rlimits_str)
        } else {
            (Vec::new(), Vec::new(), None, None)
        };

        // 3. Resolve network namespace path and CPU/memory resources from linux
        let mut netns_path = None;
        let mut vcpus = None;
        let mut ram_mib = None;

        if let Some(ref linux) = spec.linux() {
            if let Some(ref namespaces) = linux.namespaces() {
                for ns in namespaces {
                    if ns.typ() == LinuxNamespaceType::Network {
                        if let Some(p) = ns.path() {
                            netns_path = Some(p.clone());
                            break;
                        }
                    }
                }
            }

            if let Some(ref res) = linux.resources() {
                if let Some(ref mem) = res.memory() {
                    if let Some(limit) = mem.limit() {
                        if limit > 0 {
                            let mb = (limit / (1024 * 1024)) as u32;
                            if mb >= 128 {
                                ram_mib = Some(mb);
                            }
                        }
                    }
                }
                if let Some(ref cpu) = res.cpu() {
                    if let (Some(quota), Some(period)) = (cpu.quota(), cpu.period()) {
                        if quota > 0 && period > 0 {
                            let calculated = ((quota as f64) / (period as f64)).ceil() as u8;
                            if calculated > 0 {
                                vcpus = Some(calculated.min(32));
                            }
                        }
                    }
                }
            }
        }

        // 4. Resolve bind mounts to VirtioFS directory mounts or file mounts
        let mut virtiofs_mounts = Vec::new();
        let mut file_mounts = Vec::new();
        if let Some(ref mounts) = spec.mounts() {
            for (i, m) in mounts.iter().enumerate() {
                if let Some(src) = m.source() {
                    if src.exists() {
                        if src.is_dir() {
                            let tag = format!("mnt{}", i);
                            let ro = m.options().as_ref().map_or(false, |opts| {
                                opts.iter().any(|o| o == "ro" || o == "rbind:ro")
                            });
                            virtiofs_mounts.push(VirtioFsMount::new(tag, src, ro));
                        } else if src.is_file() {
                            // File mount (e.g. ConfigMap / Secret file, resolv.conf, hosts)
                            let dest = m.destination();
                            file_mounts.push((src.to_path_buf(), dest.clone()));
                        }
                    }
                }
            }
        }

        Ok(Self {
            bundle_dir,
            spec,
            rootfs_path,
            cmd,
            env,
            workdir,
            rlimits,
            netns_path,
            vcpus,
            ram_mib,
            virtiofs_mounts,
            file_mounts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_load_oci_bundle() {
        let dir = tempdir().unwrap();
        let rootfs = dir.path().join("rootfs");
        fs::create_dir_all(&rootfs).unwrap();

        let host_file = dir.path().join("secret_token.txt");
        fs::write(&host_file, "supersecret123").unwrap();

        let config_json = format!(
            r#"{{
            "ociVersion": "1.0.2",
            "root": {{
                "path": "rootfs"
            }},
            "process": {{
                "user": {{
                    "uid": 0,
                    "gid": 0
                }},
                "cwd": "/app",
                "args": ["echo", "hello from bundle"],
                "env": ["FOO=BAR"]
            }},
            "linux": {{
                "resources": {{
                    "memory": {{
                        "limit": 2147483648
                    }},
                    "cpu": {{
                        "quota": 400000,
                        "period": 100000
                    }}
                }}
            }},
            "mounts": [
                {{
                    "source": "{}",
                    "destination": "/var/run/secrets/token.txt"
                }}
            ]
        }}"#,
            host_file.display()
        );

        fs::write(dir.path().join("config.json"), config_json).unwrap();

        let bundle = OciBundle::load(dir.path()).unwrap();
        assert_eq!(bundle.cmd, vec!["echo", "hello from bundle"]);
        assert_eq!(bundle.env, vec!["FOO=BAR"]);
        assert_eq!(bundle.workdir, Some("/app".to_string()));
        assert_eq!(bundle.rootfs_path, rootfs);
        assert_eq!(bundle.vcpus, Some(4));
        assert_eq!(bundle.ram_mib, Some(2048));
        assert_eq!(bundle.file_mounts.len(), 1);
        assert_eq!(
            bundle.file_mounts[0],
            (host_file, PathBuf::from("/var/run/secrets/token.txt"))
        );
    }
}
