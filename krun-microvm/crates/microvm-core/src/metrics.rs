//! Cross-platform telemetry and resource accounting for microVM supervisor processes.
//!
//! Provides nanosecond CPU user/system times, resident set size (RSS), virtual memory,
//! page fault counters, and active thread counts.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
