//! Cross-platform telemetry and resource accounting for microVM supervisor processes.
//!
//! Provides nanosecond CPU user/system times, resident set size (RSS), virtual memory,
//! page fault counters, and active thread counts.

use crate::state::{VmState, VmStatus};
use serde::{Deserialize, Serialize};

/// Snapshot of resource consumption for a process.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProcessStats {
    /// Total CPU time spent in user mode in nanoseconds.
    pub user_cpu_ns: u64,
    /// Total CPU time spent in kernel/system mode in nanoseconds.
    pub kernel_cpu_ns: u64,
    /// Total CPU time spent (user + kernel) in nanoseconds.
    pub total_cpu_ns: u64,
    /// Resident Set Size (RSS) memory in bytes.
    pub memory_rss_bytes: u64,
    /// Virtual memory size in bytes.
    pub memory_vsize_bytes: u64,
    /// Total page faults count (minor + major).
    pub page_faults: u64,
    /// Major page faults requiring disk I/O.
    pub major_page_faults: u64,
    /// Number of active threads in the process.
    pub threads: u64,
}

/// Collects resource consumption metrics for a specific process ID.
///
/// On macOS (Darwin), queries the Mach task info via `proc_pidinfo(PROC_PIDTASKINFO)`.
/// On Linux, reads `/proc/[pid]/stat` and `/proc/[pid]/statm`.
#[cfg(target_os = "macos")]
pub fn collect_process_stats(pid: u32) -> Option<ProcessStats> {
    if pid == 0 {
        return None;
    }

    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as i32;
    let res = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };

    if res <= 0 || res < size {
        return None;
    }

    let user_cpu_ns = info.pti_total_user;
    let kernel_cpu_ns = info.pti_total_system;
    let total_cpu_ns = user_cpu_ns.saturating_add(kernel_cpu_ns);
    let memory_rss_bytes = info.pti_resident_size;
    let memory_vsize_bytes = info.pti_virtual_size;
    let page_faults = info.pti_faults.max(0) as u64;
    let major_page_faults = info.pti_pageins.max(0) as u64;
    let threads = info.pti_threadnum.max(0) as u64;

    Some(ProcessStats {
        user_cpu_ns,
        kernel_cpu_ns,
        total_cpu_ns,
        memory_rss_bytes,
        memory_vsize_bytes,
        page_faults,
        major_page_faults,
        threads,
    })
}

/// Linux implementation reading from `/proc/[pid]/stat` and `/proc/[pid]/statm`.
#[cfg(target_os = "linux")]
pub fn collect_process_stats(pid: u32) -> Option<ProcessStats> {
    if pid == 0 {
        return None;
    }

    let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let clk_tck = if clk_tck > 0 { clk_tck as u64 } else { 100 };

    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page_size = if page_size > 0 {
        page_size as u64
    } else {
        4096
    };

    // 1. Read /proc/[pid]/stat
    let stat_content = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    let closing_paren = stat_content.rfind(')')?;
    let rest = stat_content.get(closing_paren + 2..)?;
    let fields: Vec<&str> = rest.split_whitespace().collect();

    let minflt: u64 = fields.get(7).and_then(|s| s.parse().ok()).unwrap_or(0);
    let majflt: u64 = fields.get(9).and_then(|s| s.parse().ok()).unwrap_or(0);
    let utime_ticks: u64 = fields.get(11).and_then(|s| s.parse().ok()).unwrap_or(0);
    let stime_ticks: u64 = fields.get(12).and_then(|s| s.parse().ok()).unwrap_or(0);
    let threads: u64 = fields.get(17).and_then(|s| s.parse().ok()).unwrap_or(1);
    let vsize: u64 = fields.get(20).and_then(|s| s.parse().ok()).unwrap_or(0);

    let user_cpu_ns = utime_ticks.saturating_mul(1_000_000_000) / clk_tck;
    let kernel_cpu_ns = stime_ticks.saturating_mul(1_000_000_000) / clk_tck;
    let total_cpu_ns = user_cpu_ns.saturating_add(kernel_cpu_ns);

    // 2. Read /proc/[pid]/statm
    let statm_content = std::fs::read_to_string(format!("/proc/{}/statm", pid)).ok();
    let resident_pages: u64 = statm_content
        .as_deref()
        .and_then(|s| s.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let memory_rss_bytes = resident_pages.saturating_mul(page_size);

    Some(ProcessStats {
        user_cpu_ns,
        kernel_cpu_ns,
        total_cpu_ns,
        memory_rss_bytes,
        memory_vsize_bytes: vsize,
        page_faults: minflt.saturating_add(majflt),
        major_page_faults: majflt,
        threads,
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn collect_process_stats(_pid: u32) -> Option<ProcessStats> {
    None
}

/// Escapes special characters in label values according to the Prometheus exposition format.
fn escape_prometheus_label(val: &str) -> String {
    let mut out = String::with_capacity(val.len());
    for c in val.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}

/// Exports microVM inventory, configuration, and runtime telemetry in standard
/// Prometheus text exposition format (version 0.0.4).
pub fn export_prometheus_metrics(vms: &[VmState]) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    let total = vms.len();
    let running = vms.iter().filter(|v| v.status == VmStatus::Running).count();
    let paused = vms.iter().filter(|v| v.status == VmStatus::Paused).count();
    let stopped = vms.iter().filter(|v| v.status == VmStatus::Stopped).count();

    let _ = writeln!(
        out,
        "# HELP microvm_vms_total Total number of tracked microVMs"
    );
    let _ = writeln!(out, "# TYPE microvm_vms_total gauge");
    let _ = writeln!(out, "microvm_vms_total {}", total);

    let _ = writeln!(
        out,
        "# HELP microvm_vms_running_total Number of currently running microVMs"
    );
    let _ = writeln!(out, "# TYPE microvm_vms_running_total gauge");
    let _ = writeln!(out, "microvm_vms_running_total {}", running);

    let _ = writeln!(
        out,
        "# HELP microvm_vms_paused_total Number of currently paused microVMs"
    );
    let _ = writeln!(out, "# TYPE microvm_vms_paused_total gauge");
    let _ = writeln!(out, "microvm_vms_paused_total {}", paused);

    let _ = writeln!(
        out,
        "# HELP microvm_vms_stopped_total Number of stopped microVMs"
    );
    let _ = writeln!(out, "# TYPE microvm_vms_stopped_total gauge");
    let _ = writeln!(out, "microvm_vms_stopped_total {}", stopped);

    if !vms.is_empty() {
        let _ = writeln!(
            out,
            "# HELP microvm_info Informational gauge for microVM metadata"
        );
        let _ = writeln!(out, "# TYPE microvm_info gauge");
        for vm in vms {
            let status_str = match vm.status {
                VmStatus::Running => "running",
                VmStatus::Paused => "paused",
                VmStatus::Stopped => "stopped",
            };
            let _ = writeln!(
                out,
                "microvm_info{{vm_id=\"{}\",image=\"{}\",status=\"{}\",pid=\"{}\"}} 1",
                escape_prometheus_label(&vm.id),
                escape_prometheus_label(&vm.image),
                status_str,
                vm.pid
            );
        }

        let _ = writeln!(
            out,
            "# HELP microvm_configured_vcpus Configured virtual CPUs allocated to the microVM"
        );
        let _ = writeln!(out, "# TYPE microvm_configured_vcpus gauge");
        for vm in vms {
            if let Some(vcpus) = vm.vcpus {
                let _ = writeln!(
                    out,
                    "microvm_configured_vcpus{{vm_id=\"{}\"}} {}",
                    escape_prometheus_label(&vm.id),
                    vcpus
                );
            }
        }

        let _ = writeln!(
            out,
            "# HELP microvm_configured_memory_mib Configured RAM allocation in MiB"
        );
        let _ = writeln!(out, "# TYPE microvm_configured_memory_mib gauge");
        for vm in vms {
            if let Some(mem) = vm.memory_mib {
                let _ = writeln!(
                    out,
                    "microvm_configured_memory_mib{{vm_id=\"{}\"}} {}",
                    escape_prometheus_label(&vm.id),
                    mem
                );
            }
        }

        let _ = writeln!(
            out,
            "# HELP microvm_uptime_seconds Seconds elapsed since microVM creation"
        );
        let _ = writeln!(out, "# TYPE microvm_uptime_seconds gauge");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        for vm in vms {
            let uptime = now.saturating_sub(vm.created_at);
            let _ = writeln!(
                out,
                "microvm_uptime_seconds{{vm_id=\"{}\"}} {}",
                escape_prometheus_label(&vm.id),
                uptime
            );
        }

        // Live runtime telemetry for running VMs
        let mut telemetry_entries = Vec::new();
        for vm in vms {
            if vm.status == VmStatus::Running && vm.pid > 0 {
                if let Some(stats) = collect_process_stats(vm.pid) {
                    telemetry_entries.push((vm.id.clone(), stats));
                }
            }
        }

        if !telemetry_entries.is_empty() {
            let _ = writeln!(
                out,
                "# HELP microvm_cpu_user_seconds Total CPU time spent in user mode in seconds"
            );
            let _ = writeln!(out, "# TYPE microvm_cpu_user_seconds counter");
            for (id, stats) in &telemetry_entries {
                let _ = writeln!(
                    out,
                    "microvm_cpu_user_seconds{{vm_id=\"{}\"}} {:.6}",
                    escape_prometheus_label(id),
                    stats.user_cpu_ns as f64 / 1_000_000_000.0
                );
            }

            let _ = writeln!(
                out,
                "# HELP microvm_cpu_system_seconds Total CPU time spent in kernel/system mode in seconds"
            );
            let _ = writeln!(out, "# TYPE microvm_cpu_system_seconds counter");
            for (id, stats) in &telemetry_entries {
                let _ = writeln!(
                    out,
                    "microvm_cpu_system_seconds{{vm_id=\"{}\"}} {:.6}",
                    escape_prometheus_label(id),
                    stats.kernel_cpu_ns as f64 / 1_000_000_000.0
                );
            }

            let _ = writeln!(
                out,
                "# HELP microvm_memory_rss_bytes Resident Set Size (RSS) memory consumption in bytes"
            );
            let _ = writeln!(out, "# TYPE microvm_memory_rss_bytes gauge");
            for (id, stats) in &telemetry_entries {
                let _ = writeln!(
                    out,
                    "microvm_memory_rss_bytes{{vm_id=\"{}\"}} {}",
                    escape_prometheus_label(id),
                    stats.memory_rss_bytes
                );
            }

            let _ = writeln!(
                out,
                "# HELP microvm_memory_vsize_bytes Virtual memory allocation in bytes"
            );
            let _ = writeln!(out, "# TYPE microvm_memory_vsize_bytes gauge");
            for (id, stats) in &telemetry_entries {
                let _ = writeln!(
                    out,
                    "microvm_memory_vsize_bytes{{vm_id=\"{}\"}} {}",
                    escape_prometheus_label(id),
                    stats.memory_vsize_bytes
                );
            }

            let _ = writeln!(
                out,
                "# HELP microvm_threads_total Active supervisor thread count"
            );
            let _ = writeln!(out, "# TYPE microvm_threads_total gauge");
            for (id, stats) in &telemetry_entries {
                let _ = writeln!(
                    out,
                    "microvm_threads_total{{vm_id=\"{}\"}} {}",
                    escape_prometheus_label(id),
                    stats.threads
                );
            }

            let _ = writeln!(
                out,
                "# HELP microvm_page_faults_total Page faults incurred by supervisor"
            );
            let _ = writeln!(out, "# TYPE microvm_page_faults_total counter");
            for (id, stats) in &telemetry_entries {
                let _ = writeln!(
                    out,
                    "microvm_page_faults_total{{vm_id=\"{}\"}} {}",
                    escape_prometheus_label(id),
                    stats.page_faults
                );
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_current_process_telemetry_collection() {
        let current_pid = std::process::id();
        let stats = collect_process_stats(current_pid);
        assert!(
            stats.is_some(),
            "Should be able to collect stats for current process"
        );

        let s = stats.unwrap();
        assert!(s.memory_rss_bytes > 0, "RSS memory should be > 0");
        assert!(s.threads >= 1, "Threads should be >= 1");
    }

    #[test]
    fn test_invalid_pid_handling() {
        assert!(collect_process_stats(0).is_none());
        assert!(collect_process_stats(999_999_999).is_none());
    }

    #[test]
    fn test_escape_prometheus_label() {
        assert_eq!(escape_prometheus_label("clean_str"), "clean_str");
        assert_eq!(
            escape_prometheus_label("quoted \"val\" with \\slash and \nnewline"),
            "quoted \\\"val\\\" with \\\\slash and \\nnewline"
        );
    }

    #[test]
    fn test_export_prometheus_metrics_empty() {
        let metrics = export_prometheus_metrics(&[]);
        assert!(metrics.contains("microvm_vms_total 0"));
        assert!(metrics.contains("microvm_vms_running_total 0"));
        assert!(metrics.contains("microvm_vms_paused_total 0"));
        assert!(metrics.contains("microvm_vms_stopped_total 0"));
    }

    #[test]
    fn test_export_prometheus_metrics_populated() {
        let vms = vec![
            VmState {
                id: "vm-prom-1".to_string(),
                pid: std::process::id(),
                image: "alpine:latest".to_string(),
                created_at: 1000,
                port_forwards: vec![],
                instance_dir: PathBuf::from("/tmp/prom-1"),
                status: VmStatus::Running,
                vcpus: Some(4),
                memory_mib: Some(1024),
            },
            VmState {
                id: "vm-prom-2".to_string(),
                pid: 0,
                image: "ubuntu:22.04".to_string(),
                created_at: 2000,
                port_forwards: vec![],
                instance_dir: PathBuf::from("/tmp/prom-2"),
                status: VmStatus::Stopped,
                vcpus: Some(2),
                memory_mib: Some(512),
            },
        ];

        let metrics = export_prometheus_metrics(&vms);
        assert!(metrics.contains("microvm_vms_total 2"));
        assert!(metrics.contains("microvm_vms_running_total 1"));
        assert!(metrics.contains("microvm_vms_stopped_total 1"));
        assert!(metrics.contains(
            "microvm_info{vm_id=\"vm-prom-1\",image=\"alpine:latest\",status=\"running\",pid="
        ));
        assert!(metrics.contains("microvm_info{vm_id=\"vm-prom-2\",image=\"ubuntu:22.04\",status=\"stopped\",pid=\"0\"} 1"));
        assert!(metrics.contains("microvm_configured_vcpus{vm_id=\"vm-prom-1\"} 4"));
        assert!(metrics.contains("microvm_configured_memory_mib{vm_id=\"vm-prom-1\"} 1024"));
        assert!(metrics.contains("microvm_configured_vcpus{vm_id=\"vm-prom-2\"} 2"));
        assert!(metrics.contains("microvm_configured_memory_mib{vm_id=\"vm-prom-2\"} 512"));
        assert!(metrics.contains("microvm_uptime_seconds{vm_id=\"vm-prom-1\"}"));
        // Check live telemetry collected for current process
        assert!(metrics.contains("microvm_memory_rss_bytes{vm_id=\"vm-prom-1\"}"));
        assert!(metrics.contains("microvm_threads_total{vm_id=\"vm-prom-1\"}"));
    }
}
