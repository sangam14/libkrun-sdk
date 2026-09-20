use std::net::TcpListener;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

pub struct Preflight;

impl Preflight {
    pub fn run_all(ports: &[u16], workdir: &Path) -> Vec<CheckResult> {
        let mut results = Vec::new();
        results.push(Self::check_virtualization());
        results.push(Self::check_workdir(workdir));

        for &port in ports {
            results.push(Self::check_port(port));
        }

        results
    }

    pub fn check_virtualization() -> CheckResult {
        #[cfg(target_os = "macos")]
        {
            // On macOS Apple Silicon, Hypervisor.framework provides virtualization
            let nested = krun_sys::check_nested_virt();
            CheckResult {
                name: "Virtualization Support (macOS HVF)".to_string(),
                passed: true,
                message: format!(
                    "Hypervisor.framework available (nested virt: {})",
                    if nested {
                        "enabled"
                    } else {
                        "disabled/unsupported"
                    }
                ),
            }
        }

        #[cfg(target_os = "linux")]
        {
            let kvm_path = Path::new("/dev/kvm");
            if !kvm_path.exists() {
                CheckResult {
                    name: "Virtualization Support (Linux KVM)".to_string(),
                    passed: false,
                    message: "/dev/kvm device does not exist. Enable KVM in BIOS or kernel."
                        .to_string(),
                }
            } else {
                match std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(kvm_path)
                {
                    Ok(_) => CheckResult {
                        name: "Virtualization Support (Linux KVM)".to_string(),
                        passed: true,
                        message: "/dev/kvm is accessible with read/write permissions.".to_string(),
                    },
                    Err(e) => CheckResult {
                        name: "Virtualization Support (Linux KVM)".to_string(),
                        passed: false,
                        message: format!(
                            "Cannot open /dev/kvm: {e}. Check user groups (e.g. kvm)."
                        ),
                    },
                }
            }
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            CheckResult {
                name: "Virtualization Support".to_string(),
                passed: false,
                message: "Unsupported operating system for libkrun microVMs".to_string(),
            }
        }
    }

    pub fn check_workdir(workdir: &Path) -> CheckResult {
        if let Err(e) = std::fs::create_dir_all(workdir) {
            return CheckResult {
                name: "Workdir Writable".to_string(),
                passed: false,
                message: format!("Cannot create or access workdir {}: {e}", workdir.display()),
            };
        }

        CheckResult {
            name: "Workdir Writable".to_string(),
            passed: true,
            message: format!("Workdir {} is accessible", workdir.display()),
        }
    }

    pub fn check_port(port: u16) -> CheckResult {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(_) => CheckResult {
                name: format!("Port {} Availability", port),
                passed: true,
                message: format!("Port {} is free to bind on 127.0.0.1", port),
            },
            Err(e) => CheckResult {
                name: format!("Port {} Availability", port),
                passed: false,
                message: format!("Port {} is unavailable: {e}", port),
            },
        }
    }
}
