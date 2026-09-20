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
    /// Lifecycle phase: Pending, Running, Succeeded, Failed
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
}
