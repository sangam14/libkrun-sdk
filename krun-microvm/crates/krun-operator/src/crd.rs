use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Custom Resource Definition for running hardware-isolated microVMs via libkrun
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "krun.io",
    version = "v1alpha1",
    kind = "MicroVm",
    plural = "microvms",
    singular = "microvm",
    namespaced,
    status = "MicroVmStatus",
    printcolumn = r#"{"name":"Phase", "type":"string", "description":"Status phase of the microVM", "jsonPath":".status.phase"}"#,
    printcolumn = r#"{"name":"Pod", "type":"string", "description":"Name of backing pod", "jsonPath":".status.podName"}"#,
    printcolumn = r#"{"name":"IP", "type":"string", "description":"IP address of microVM", "jsonPath":".status.podIp"}"#,
    printcolumn = r#"{"name":"Age", "type":"date", "jsonPath":".metadata.creationTimestamp"}"#
)]
pub struct MicroVmSpec {
    /// OCI container image to run as a microVM (e.g. alpine:latest, ghcr.io/ericlbuehler/mistral.rs:cpu-latest)
    pub image: String,

    /// Number of allocated virtual CPUs
    #[serde(default = "default_vcpus")]
    pub vcpus: u8,

    /// Memory limit (e.g. "512Mi", "4Gi", "8Gi")
    #[serde(default = "default_memory")]
    pub memory: String,

    /// Optional command arguments override
    pub cmd: Option<Vec<String>>,

    /// Optional environment variable key-value pairs
    pub env: Option<Vec<EnvVar>>,

    /// Container port to expose
    pub port: Option<u16>,

    /// Optional decoupled OCI model artifact reference (e.g. ghcr.io/mistralai/mistral-7b:v0.3)
    #[serde(rename = "modelArtifact")]
    pub model_artifact: Option<String>,

    /// Host directory path to mount as an isolated CoW workspace
    #[serde(rename = "workspaceCow")]
    pub workspace_cow: Option<String>,

    /// Declarative pause state: if true, pauses/freezes all microVM vCPUs
    #[serde(default)]
    pub paused: Option<bool>,

    /// VirtioFS DAX shared memory window size (e.g. "4Gi", "512Mi")
    #[serde(rename = "daxWindowSize", default)]
    pub dax_window_size: Option<String>,

    /// Dragonfly Nydus RAFSv6 image acceleration and lazy loading configuration
    #[serde(rename = "imageAcceleration", default)]
    pub image_acceleration: Option<ImageAccelerationSpec>,

    /// Enable hardware-accelerated virtio-gpu (Metal on Apple Silicon, DRM on Linux)
    #[serde(default)]
    pub gpu: Option<bool>,

    /// Shared memory vRAM window size for virtio-gpu (e.g. "4Gi", "8Gi")
    #[serde(rename = "gpuShmSize", default)]
    pub gpu_shm_size: Option<String>,

    /// Multi-port mappings exposed by the microVM
    #[serde(default)]
    pub ports: Option<Vec<PortMapping>>,

    /// Network mode: "cni", "gvproxy", "tsi", or "none" (default: "cni" in Kubernetes, "gvproxy" for rootless)
    #[serde(rename = "networkMode", default)]
    pub network_mode: Option<String>,

    /// Allowed outbound egress destinations for zero-trust egress control (e.g. ["api.openai.com:443", "*.github.com:443"])
    #[serde(rename = "allowEgress", default)]
    pub allow_egress: Option<Vec<String>>,

    /// Custom DNS nameservers for in-guest name resolution
    #[serde(rename = "dnsServers", default)]
    pub dns_servers: Option<Vec<String>>,

    /// Cumulative LLM token budget ceiling; blocks outbound LLM requests once breached
    #[serde(rename = "tokenBudget", default)]
    pub token_budget: Option<u64>,

    /// Host filesystem and capability sandboxing (default: true)
    #[serde(default)]
    pub sandbox: Option<bool>,

    /// Host volume mounts to pass into the microVM
    #[serde(rename = "volumeMounts", default)]
    pub volume_mounts: Option<Vec<VolumeMountSpec>>,

    /// Optional custom annotations to propagate to the backing Pod
    #[serde(rename = "podAnnotations", default)]
    pub pod_annotations: Option<std::collections::BTreeMap<String, String>>,

    /// Optional custom labels to propagate to the backing Pod
    #[serde(rename = "podLabels", default)]
    pub pod_labels: Option<std::collections::BTreeMap<String, String>>,
}

/// Port mapping definition for MicroVM
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub struct PortMapping {
    /// Host port (optional; containerPort will be used if omitted)
    #[serde(rename = "hostPort", default)]
    pub host_port: Option<u16>,

    /// Container port inside the microVM
    #[serde(rename = "containerPort")]
    pub container_port: u16,

    /// Protocol (default: "TCP")
    #[serde(default = "default_protocol")]
    pub protocol: Option<String>,

    /// Optional name for this port
    #[serde(default)]
    pub name: Option<String>,
}

fn default_protocol() -> Option<String> {
    Some("TCP".to_string())
}

/// Volume mount definition for passing host storage into a microVM
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub struct VolumeMountSpec {
    /// Volume name
    pub name: String,

    /// Destination mount path inside the microVM
    #[serde(rename = "mountPath")]
    pub mount_path: String,

    /// Source host directory path
    #[serde(rename = "hostPath")]
    pub host_path: String,

    /// Mount as read-only
    #[serde(rename = "readOnly", default)]
    pub read_only: Option<bool>,
}

/// Specification for Dragonfly Nydus RAFSv6 / EROFS image acceleration and lazy loading
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq, Default)]
pub struct ImageAccelerationSpec {
    /// Accelerated format: "rafsv6", "erofs", or "zran"
    #[serde(default)]
    pub format: Option<String>,

    /// Enable instant on-demand lazy loading of chunks
    #[serde(rename = "lazyLoad", default)]
    pub lazy_load: Option<bool>,

    /// Host directory for caching downloaded chunk blobs
    #[serde(rename = "chunkCacheDir", default)]
    pub chunk_cache_dir: Option<String>,

    /// Chunk or macro-chunk size (e.g. "4Mi", "64Mi")
    #[serde(rename = "chunkSize", default)]
    pub chunk_size: Option<String>,

    /// List of paths to prefetch ahead of execution
    #[serde(default)]
    pub prefetch: Option<Vec<String>>,
}

fn default_vcpus() -> u8 {
    2
}

fn default_memory() -> String {
    "512Mi".to_string()
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
pub struct MicroVmStatus {
    /// Lifecycle phase: Pending, Running, Paused, Succeeded, Failed
    pub phase: String,

    /// Backing Pod name managed by the operator
    #[serde(rename = "podName")]
    pub pod_name: Option<String>,

    /// Assigned Pod IP address
    #[serde(rename = "podIp")]
    pub pod_ip: Option<String>,

    /// Host node running the microVM
    #[serde(rename = "nodeName")]
    pub node_name: Option<String>,

    /// True if the microVM is healthy and accepting requests
    pub ready: bool,

    /// Human-readable message or error description
    pub message: Option<String>,

    /// Status conditions representing lifecycle progression
    #[serde(default)]
    pub conditions: Option<Vec<MicroVmCondition>>,
}

/// Standard Kubernetes-style condition object for MicroVM lifecycle tracking
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub struct MicroVmCondition {
    /// Type of condition: "Ready", "PodScheduled", "Initialized"
    #[serde(rename = "type")]
    pub type_: String,

    /// Status: "True", "False", "Unknown"
    pub status: String,

    /// Last time the condition transitioned from one status to another
    #[serde(rename = "lastTransitionTime", default)]
    pub last_transition_time: Option<String>,

    /// Machine-readable reason for condition's last transition
    #[serde(default)]
    pub reason: Option<String>,

    /// Human-readable message explaining the condition status
    #[serde(default)]
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_microvm_spec_serialization_with_acceleration() {
        let spec_json = r#"{
            "image": "alpine:latest",
            "vcpus": 4,
            "memory": "4Gi",
            "daxWindowSize": "2Gi",
            "imageAcceleration": {
                "format": "rafsv6",
                "lazyLoad": true,
                "chunkCacheDir": "/var/cache/nydus",
                "chunkSize": "64Mi",
                "prefetch": ["/bin", "/lib"]
            }
        }"#;

        let spec: MicroVmSpec = serde_json::from_str(spec_json).unwrap();
        assert_eq!(spec.image, "alpine:latest");
        assert_eq!(spec.vcpus, 4);
        assert_eq!(spec.dax_window_size.as_deref(), Some("2Gi"));

        let acc = spec.image_acceleration.expect("Expected imageAcceleration");
        assert_eq!(acc.format.as_deref(), Some("rafsv6"));
        assert_eq!(acc.lazy_load, Some(true));
        assert_eq!(acc.chunk_size.as_deref(), Some("64Mi"));
        assert_eq!(acc.prefetch.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_microvm_spec_with_networking_and_ports() {
        let spec_json = r#"{
            "image": "nginx:alpine",
            "vcpus": 2,
            "memory": "1Gi",
            "networkMode": "gvproxy",
            "ports": [
                { "hostPort": 8080, "containerPort": 80 },
                { "containerPort": 443, "protocol": "TCP", "name": "https" }
            ],
            "allowEgress": ["api.anthropic.com:443", "*.crates.io:443"],
            "tokenBudget": 50000,
            "sandbox": true,
            "volumeMounts": [
                { "name": "data", "mountPath": "/var/data", "hostPath": "/mnt/host-data", "readOnly": false }
            ]
        }"#;

        let spec: MicroVmSpec = serde_json::from_str(spec_json).unwrap();
        assert_eq!(spec.network_mode.as_deref(), Some("gvproxy"));
        assert_eq!(spec.token_budget, Some(50000));
        assert_eq!(spec.sandbox, Some(true));
        let ports = spec.ports.expect("Expected ports");
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].host_port, Some(8080));
        assert_eq!(ports[0].container_port, 80);
        assert_eq!(ports[1].name.as_deref(), Some("https"));

        let egress = spec.allow_egress.expect("Expected allowEgress");
        assert_eq!(egress.len(), 2);

        let vols = spec.volume_mounts.expect("Expected volumeMounts");
        assert_eq!(vols[0].name, "data");
        assert_eq!(vols[0].read_only, Some(false));
    }
}
