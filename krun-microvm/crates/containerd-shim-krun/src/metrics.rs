//! Telemetry and resource metrics serialization for containerd-shim-krun.
//!
//! Reuses cross-platform process accounting from `microvm-core` and converts it
//! into standard `io.containerd.cgroups.v1.Metrics` protobuf format for CRI / `crictl stats`.

use containerd_shim_protos::cgroups::metrics::{
    CPUStat, CPUUsage, MemoryEntry, MemoryStat, Metrics, PidsStat,
};
use containerd_shim_protos::protobuf::{Message, MessageField};
pub use microvm_core::metrics::{collect_process_stats, ProcessStats};

/// Converts native process telemetry into containerd v1 cgroups Metrics protobuf.
pub fn build_cgroups_metrics(stats: &ProcessStats) -> Metrics {
    let mut metrics = Metrics::new();

    // CPU metrics
    let mut cpu = CPUStat::new();
    let mut usage = CPUUsage::new();
    usage.total = stats.total_cpu_ns;
    usage.user = stats.user_cpu_ns;
    usage.kernel = stats.kernel_cpu_ns;
    cpu.usage = MessageField::some(usage);
    metrics.cpu = MessageField::some(cpu);

    // Memory metrics
    let mut memory = MemoryStat::new();
    memory.rss = stats.memory_rss_bytes;
    memory.total_rss = stats.memory_rss_bytes;
    memory.pg_fault = stats.page_faults;
    memory.total_pg_fault = stats.page_faults;
    memory.pg_maj_fault = stats.major_page_faults;
    memory.total_pg_maj_fault = stats.major_page_faults;

    let mut entry = MemoryEntry::new();
    entry.usage = stats.memory_rss_bytes;
    memory.usage = MessageField::some(entry);
    metrics.memory = MessageField::some(memory);

    // PIDs / Threads metrics
    let mut pids = PidsStat::new();
    pids.current = stats.threads;
    metrics.pids = MessageField::some(pids);

    metrics
}

/// Serializes cgroups Metrics into a `google.protobuf.Any` well-known type.
pub fn encode_metrics_any(metrics: &Metrics) -> Result<containerd_shim_protos::protobuf::well_known_types::any::Any, String> {
    let bytes = metrics.write_to_bytes().map_err(|e| e.to_string())?;
    let mut any = containerd_shim_protos::protobuf::well_known_types::any::Any::new();
    any.type_url = "io.containerd.cgroups.v1.Metrics".to_string();
    any.value = bytes;
    Ok(any)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_cgroups_metrics() {
        let stats = ProcessStats {
            user_cpu_ns: 50_000_000,
            kernel_cpu_ns: 25_000_000,
            total_cpu_ns: 75_000_000,
            memory_rss_bytes: 1024 * 1024 * 64, // 64 MB
            memory_vsize_bytes: 1024 * 1024 * 128,
            page_faults: 1200,
            major_page_faults: 15,
            threads: 4,
        };

        let metrics = build_cgroups_metrics(&stats);
        assert!(metrics.cpu.is_some());
        assert!(metrics.memory.is_some());
        assert!(metrics.pids.is_some());

        let cpu = metrics.cpu.as_ref().unwrap();
        let usage = cpu.usage.as_ref().unwrap();
        assert_eq!(usage.total, 75_000_000);
        assert_eq!(usage.user, 50_000_000);
        assert_eq!(usage.kernel, 25_000_000);

        let mem = metrics.memory.as_ref().unwrap();
        assert_eq!(mem.rss, 1024 * 1024 * 64);
        assert_eq!(mem.pg_fault, 1200);

        let pids = metrics.pids.as_ref().unwrap();
        assert_eq!(pids.current, 4);

        // Test protobuf Any encoding
        let any = encode_metrics_any(&metrics).expect("Serialization into Any should succeed");
        assert_eq!(any.type_url, "io.containerd.cgroups.v1.Metrics");
        assert!(!any.value.is_empty());
    }
}
