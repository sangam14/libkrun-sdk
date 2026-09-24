use anyhow::{bail, Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::config::parse_size_to_bytes;
use crate::vm::MicroVmBuilder;

/// Represents a multi-service declarative specification, compatible with Docker Compose YAML formats.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComposeSpec {
    #[serde(default = "default_version")]
    pub version: String,

    #[serde(default)]
    pub name: Option<String>,

    #[serde(default)]
    pub services: BTreeMap<String, ServiceSpec>,

    #[serde(default)]
    pub volumes: BTreeMap<String, ComposeVolumeSpec>,

    #[serde(default)]
    pub networks: BTreeMap<String, ComposeNetworkSpec>,
}

fn default_version() -> String {
    "krun/v1".to_string()
}

/// Specifications for an individual microVM service within a compose manifest.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ServiceSpec {
    /// OCI image reference (e.g. alpine:latest, nginx:alpine)
    #[serde(default)]
    pub image: String,

    /// Number of virtual CPUs (defaults to 2 if unspecified)
    #[serde(default)]
    pub cpus: Option<u8>,

    /// Memory string (e.g. "512M", "1G", "2GiB", defaults to "512M")
    #[serde(default)]
    pub memory: Option<String>,

    /// Command override (supports array ["sh", "-c", "echo hi"] or string "echo hi")
    #[serde(default, deserialize_with = "deserialize_optional_cmd")]
    pub cmd: Option<Vec<String>>,

    /// Entrypoint override
    #[serde(default, deserialize_with = "deserialize_optional_cmd")]
    pub entrypoint: Option<Vec<String>>,

    /// Environment variables (supports map { "KEY": "VAL" } or list ["KEY=VAL"])
    #[serde(default, deserialize_with = "deserialize_env_map")]
    pub env: BTreeMap<String, String>,

    /// Port forward mappings (e.g. ["8080:80", "9000:9000", "3000"])
    #[serde(default)]
    pub ports: Vec<String>,

    /// Volume mounts (e.g. ["./data:/data", "./workspace:/workspace:rw"])
    #[serde(default)]
    pub volumes: Vec<String>,

    /// Working directory inside guest
    #[serde(default, rename = "working_dir", alias = "workdir")]
    pub working_dir: Option<String>,

    /// Network isolation mode: "gvproxy", "tsi", "none", "cni"
    #[serde(default, rename = "network_mode")]
    pub network_mode: Option<String>,

    /// Services that must be booted prior to this service
    #[serde(default, deserialize_with = "deserialize_depends_on")]
    pub depends_on: Vec<String>,

    /// Restart policy: "no", "always", "on-failure"
    #[serde(default)]
    pub restart: Option<String>,

    /// Hardware-accelerated virtio-gpu (Metal on Apple Silicon, DRM on Linux)
    #[serde(default)]
    pub gpu: Option<bool>,

    /// Shared memory vRAM window size for virtio-gpu (e.g. "2G", "4Gi")
    #[serde(default, rename = "gpu_shm_size")]
    pub gpu_shm_size: Option<String>,

    /// VirtioFS DAX window size (e.g. "4G")
    #[serde(default)]
    pub dax: Option<String>,

    /// Secret injection list (e.g. ["API_KEY=val", "SECRET_KEY"])
    #[serde(default)]
    pub secrets: Vec<String>,

    /// Allowed outbound egress destinations for zero-trust egress control
    #[serde(default, rename = "allow_hosts")]
    pub allow_hosts: Vec<String>,

    /// Host sandboxing (default: true)
    #[serde(default)]
    pub sandbox: Option<bool>,

    /// User metadata labels
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ComposeVolumeSpec {
    #[serde(default)]
    pub driver: Option<String>,
    #[serde(default)]
    pub external: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ComposeNetworkSpec {
    #[serde(default)]
    pub driver: Option<String>,
}

/// Kubernetes-style `MicroVm` Custom Resource YAML definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct K8sMicroVmManifest {
    #[serde(rename = "apiVersion", default = "default_api_version")]
    pub api_version: String,

    pub kind: String,

    pub metadata: K8sMetadata,

    pub spec: K8sMicroVmSpec,
}

fn default_api_version() -> String {
    "krun.io/v1alpha1".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct K8sMetadata {
    pub name: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct K8sMicroVmSpec {
    pub image: String,
    #[serde(default)]
    pub vcpus: Option<u8>,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub cmd: Option<Vec<String>>,
    #[serde(default)]
    pub env: Option<Vec<K8sEnvVar>>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub ports: Option<Vec<K8sPortMapping>>,
    #[serde(default, rename = "workspaceCow")]
    pub workspace_cow: Option<String>,
    #[serde(default, rename = "daxWindowSize")]
    pub dax_window_size: Option<String>,
    #[serde(default)]
    pub gpu: Option<bool>,
    #[serde(default, rename = "gpuShmSize")]
    pub gpu_shm_size: Option<String>,
    #[serde(default, rename = "networkMode")]
    pub network_mode: Option<String>,
    #[serde(default, rename = "allowEgress")]
    pub allow_egress: Option<Vec<String>>,
    #[serde(default)]
    pub sandbox: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct K8sEnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct K8sPortMapping {
    pub host: u16,
    pub guest: u16,
}

/// Unified manifest abstraction: auto-detects Docker Compose or Kubernetes CRD formats.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Manifest {
    K8s(K8sMicroVmManifest),
    Compose(ComposeSpec),
}

impl Manifest {
    /// Parses a YAML manifest from raw string content.
    pub fn parse(yaml_str: &str) -> Result<Self> {
        let manifest: Manifest = serde_yaml::from_str(yaml_str)
            .context("Failed to parse YAML manifest (neither Compose nor MicroVm CRD format)")?;
        Ok(manifest)
    }

    /// Reads and parses a YAML manifest from a filesystem path.
    pub fn load_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("Failed to read YAML file at {}", path.as_ref().display()))?;
        Self::parse(&content)
    }

    /// Converts this manifest into a standardized `ComposeSpec`.
    pub fn into_compose_spec(self) -> ComposeSpec {
        match self {
            Manifest::Compose(spec) => spec,
            Manifest::K8s(k8s) => {
                let mut env_map = BTreeMap::new();
                if let Some(envs) = k8s.spec.env {
                    for e in envs {
                        env_map.insert(e.name, e.value);
                    }
                }

                let mut port_strings = Vec::new();
                if let Some(single_port) = k8s.spec.port {
                    port_strings.push(format!("{single_port}:{single_port}"));
                }
                if let Some(multi_ports) = k8s.spec.ports {
                    for p in multi_ports {
                        port_strings.push(format!("{}:{}", p.host, p.guest));
                    }
                }

                let svc = ServiceSpec {
                    image: k8s.spec.image,
                    cpus: k8s.spec.vcpus,
                    memory: k8s.spec.memory,
                    cmd: k8s.spec.cmd,
                    entrypoint: None,
                    env: env_map,
                    ports: port_strings,
                    volumes: Vec::new(),
                    working_dir: None,
                    network_mode: k8s.spec.network_mode,
                    depends_on: Vec::new(),
                    restart: None,
                    gpu: k8s.spec.gpu,
                    gpu_shm_size: k8s.spec.gpu_shm_size,
                    dax: k8s.spec.dax_window_size,
                    secrets: Vec::new(),
                    allow_hosts: k8s.spec.allow_egress.unwrap_or_default(),
                    sandbox: k8s.spec.sandbox,
                    labels: k8s.metadata.labels,
                };

                let mut services = BTreeMap::new();
                services.insert(k8s.metadata.name.clone(), svc);

                ComposeSpec {
                    version: "krun/v1".to_string(),
                    name: Some(k8s.metadata.name),
                    services,
                    volumes: BTreeMap::new(),
                    networks: BTreeMap::new(),
                }
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Serde helper deserializers for permissive compose parsing
// -----------------------------------------------------------------------------

fn deserialize_optional_cmd<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum CmdValue {
        List(Vec<String>),
        Str(String),
    }

    match Option::<CmdValue>::deserialize(deserializer)? {
        None => Ok(None),
        Some(CmdValue::List(list)) => Ok(Some(list)),
        Some(CmdValue::Str(s)) => {
            // Split shell command string by whitespace respecting simple quotes
            let parts: Vec<String> = s.split_whitespace().map(|s| s.to_string()).collect();
            Ok(Some(parts))
        }
    }
}

fn deserialize_env_map<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum EnvValue {
        Map(BTreeMap<String, serde_yaml::Value>),
        List(Vec<String>),
    }

    let mut res = BTreeMap::new();
    match Option::<EnvValue>::deserialize(deserializer)? {
        None => Ok(res),
        Some(EnvValue::Map(map)) => {
            for (k, v) in map {
                let val_str = match v {
                    serde_yaml::Value::String(s) => s,
                    serde_yaml::Value::Number(n) => n.to_string(),
                    serde_yaml::Value::Bool(b) => b.to_string(),
                    other => serde_json::to_string(&other).unwrap_or_default(),
                };
                res.insert(k, val_str);
            }
            Ok(res)
        }
        Some(EnvValue::List(list)) => {
            for item in list {
                if let Some((k, v)) = item.split_once('=') {
                    res.insert(k.trim().to_string(), v.to_string());
                } else {
                    res.insert(item.trim().to_string(), String::new());
                }
            }
            Ok(res)
        }
    }
}

fn deserialize_depends_on<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum DepValue {
        List(Vec<String>),
        Map(BTreeMap<String, serde_yaml::Value>),
    }

    match Option::<DepValue>::deserialize(deserializer)? {
        None => Ok(Vec::new()),
        Some(DepValue::List(list)) => Ok(list),
        Some(DepValue::Map(map)) => Ok(map.into_keys().collect()),
    }
}

// -----------------------------------------------------------------------------
// Compose Project Orchestrator
// -----------------------------------------------------------------------------

/// State of an active compose project saved to disk for CLI inspection and shutdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeProjectState {
    pub name: String,
    pub compose_file: PathBuf,
    pub working_dir: PathBuf,
    pub created_at: u64,
    pub services: BTreeMap<String, ComposeServiceState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeServiceState {
    pub service_name: String,
    pub instance_id: String,
    pub pid: Option<u32>,
    pub image: String,
    pub status: String,
    pub ports: Vec<String>,
}

/// Orchestrates multi-microVM deployments from a `ComposeSpec`.
pub struct ComposeProject {
    pub name: String,
    pub spec: ComposeSpec,
    pub base_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl ComposeProject {
    pub fn new(
        name: impl Into<String>,
        spec: ComposeSpec,
        base_dir: impl Into<PathBuf>,
        data_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            name: name.into(),
            spec,
            base_dir: base_dir.into(),
            data_dir: data_dir.into(),
        }
    }

    /// Auto-discovers compose file (`krun-compose.yaml`, `compose.yaml`, `docker-compose.yaml`)
    /// in the current or specified directory.
    pub fn discover_compose_file(dir: &Path) -> Result<PathBuf> {
        let candidates = [
            "krun-compose.yaml",
            "krun-compose.yml",
            "microvm-compose.yaml",
            "microvm-compose.yml",
            "compose.yaml",
            "compose.yml",
            "docker-compose.yaml",
            "docker-compose.yml",
            "microvm.yaml",
            "microvm.yml",
        ];

        for c in &candidates {
            let p = dir.join(c);
            if p.exists() {
                return Ok(p);
            }
        }

        bail!(
            "No compose manifest found in '{}'. (Looked for: krun-compose.yaml, compose.yaml, microvm.yaml)",
            dir.display()
        );
    }

    /// Loads project from a file path. Defaults project name to directory name if not specified in spec.
    pub fn load(path: &Path, data_dir: Option<PathBuf>) -> Result<Self> {
        let manifest = Manifest::load_file(path)?;
        let spec = manifest.into_compose_spec();

        let base_dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()
            .unwrap_or_else(|_| path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf());

        let project_name = spec.name.clone().unwrap_or_else(|| {
            base_dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "default".to_string())
        });

        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let final_data_dir = data_dir.unwrap_or_else(|| PathBuf::from(home).join(".cache/krun-microvm"));

        Ok(Self::new(project_name, spec, base_dir, final_data_dir))
    }

    /// Resolves the topological launch order of services based on `depends_on`.
    /// Detects circular dependencies and returns ordered service names.
    pub fn resolve_launch_order(&self) -> Result<Vec<String>> {
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

        for name in self.spec.services.keys() {
            in_degree.insert(name.as_str(), 0);
            adj.insert(name.as_str(), Vec::new());
        }

        for (name, svc) in &self.spec.services {
            for dep in &svc.depends_on {
                if !self.spec.services.contains_key(dep) {
                    bail!(
                        "Service '{}' depends on undefined service '{}'",
                        name,
                        dep
                    );
                }
                adj.get_mut(dep.as_str()).unwrap().push(name.as_str());
                *in_degree.get_mut(name.as_str()).unwrap() += 1;
            }
        }

        // Kahn's algorithm
        let mut queue: Vec<&str> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&name, _)| name)
            .collect();
        queue.sort(); // Stable ordering

        let mut order = Vec::new();
        while let Some(u) = queue.pop() {
            order.push(u.to_string());
            if let Some(neighbors) = adj.get(u) {
                for &v in neighbors {
                    let deg = in_degree.get_mut(v).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(v);
                    }
                }
            }
        }

        if order.len() != self.spec.services.len() {
            bail!("Circular dependency detected in compose services depends_on graph");
        }

        Ok(order)
    }

    /// Builds a `MicroVmBuilder` configured for a specific service in the project.
    pub fn build_service_vm(&self, service_name: &str) -> Result<MicroVmBuilder> {
        let svc = self
            .spec
            .services
            .get(service_name)
            .with_context(|| format!("Service '{service_name}' not found in compose spec"))?;

        if svc.image.is_empty() {
            bail!("Service '{service_name}' has no image defined");
        }

        let mut builder = MicroVmBuilder::new(&svc.image)
            .data_dir(self.data_dir.clone())
            .detach(true) // Compose services run detached
            .sandbox(svc.sandbox.unwrap_or(true));

        if let Some(c) = svc.cpus {
            builder = builder.cpus(c);
        } else {
            builder = builder.cpus(2);
        }

        if let Some(ref mem_str) = svc.memory {
            let bytes = parse_size_to_bytes(mem_str)
                .with_context(|| format!("Invalid memory value '{mem_str}' for service '{service_name}'"))?;
            builder = builder.memory_mb((bytes / (1024 * 1024)) as u32);
        } else {
            builder = builder.memory_mb(512);
        }

        if let Some(ref cmd) = svc.cmd {
            builder = builder.cmd(cmd.clone());
        }

        if let Some(ref workdir) = svc.working_dir {
            builder = builder.workdir(workdir);
        }

        // Environment variables
        for (k, v) in &svc.env {
            builder = builder.env(k, v);
        }

        // Project and service tracking metadata env
        builder = builder.env("KRUN_COMPOSE_PROJECT", &self.name);
        builder = builder.env("KRUN_COMPOSE_SERVICE", service_name);

        // Port forwards
        for p in &svc.ports {
            let (host, guest) = parse_port_forward(p)?;
            builder = builder.port_forward(host, guest);
        }

        // Volumes
        for v in &svc.volumes {
            let (host_path, tag, ro) = self.parse_volume_mount(v)?;
            builder = builder.virtiofs(&tag, host_path, ro);
        }

        // GPU
        if svc.gpu.unwrap_or(false) {
            builder = builder.gpu(true);
            if let Some(ref shm) = svc.gpu_shm_size {
                let bytes = parse_size_to_bytes(shm)?;
                builder = builder.gpu_shm_size(bytes);
            }
        }

        // DAX
        if let Some(ref dax_str) = svc.dax {
            builder = builder.dax_window_size_str(dax_str)?;
        }

        // Secrets
        for s in &svc.secrets {
            let (k, v) = if let Some((k, v)) = s.split_once('=') {
                (k.to_string(), v.to_string())
            } else if let Ok(val) = std::env::var(s) {
                (s.clone(), val)
            } else {
                (s.clone(), String::new())
            };
            builder = builder.secret(k, v);
        }

        // Allow hosts
        for h in &svc.allow_hosts {
            builder = builder.allow_host(h);
        }

        // Network mode
        if let Some(ref net) = svc.network_mode {
            match net.to_lowercase().as_str() {
                "none" => builder = builder.network_mode(crate::net::NetworkMode::None),
                "gvproxy" => builder = builder.network_mode(crate::net::NetworkMode::Gvproxy),
                "tsi" => builder = builder.network_mode(crate::net::NetworkMode::Tsi),
                _ => {}
            }
        }

        Ok(builder)
    }

    fn parse_volume_mount(&self, vol_str: &str) -> Result<(PathBuf, String, bool)> {
        let parts: Vec<&str> = vol_str.split(':').collect();
        if parts.is_empty() {
            bail!("Empty volume string");
        }

        let host_src = parts[0];
        let tag = if parts.len() > 1 {
            parts[1].trim_start_matches('/').replace('/', "_")
        } else {
            "vol".to_string()
        };
        let ro = parts.get(2).is_some_and(|&s| s == "ro");

        let host_path = if host_src.starts_with('.') || !host_src.starts_with('/') {
            self.base_dir.join(host_src)
        } else {
            PathBuf::from(host_src)
        };

        if !host_path.exists() {
            let _ = std::fs::create_dir_all(&host_path);
        }

        Ok((host_path, tag, ro))
    }

    /// Path to project state file.
    pub fn state_file_path(&self) -> PathBuf {
        self.data_dir
            .join("compose")
            .join(&self.name)
            .join("project_state.json")
    }

    /// Loads persisted project state from cache directory.
    pub fn load_state(&self) -> Result<Option<ComposeProjectState>> {
        let p = self.state_file_path();
        if !p.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(&p)?;
        let state: ComposeProjectState = serde_json::from_str(&data)?;
        Ok(Some(state))
    }

    /// Saves project state to cache directory.
    pub fn save_state(&self, state: &ComposeProjectState) -> Result<()> {
        let p = self.state_file_path();
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(state)?;
        std::fs::write(&p, json)?;
        Ok(())
    }

    /// Deletes project state file from disk.
    pub fn remove_state(&self) -> Result<()> {
        let p = self.state_file_path();
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
        Ok(())
    }
}

fn parse_port_forward(p: &str) -> Result<(u16, u16)> {
    let parts: Vec<&str> = p.split(':').collect();
    match parts.len() {
        1 => {
            let port: u16 = parts[0].parse().context("Invalid port number")?;
            Ok((port, port))
        }
        2 => {
            let host: u16 = parts[0].parse().context("Invalid host port")?;
            let guest: u16 = parts[1].parse().context("Invalid guest port")?;
            Ok((host, guest))
        }
        _ => bail!("Invalid port forward specification: '{p}', expected host:guest or port"),
    }
}

// -----------------------------------------------------------------------------
// Unit Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_docker_compose_yaml_standard() {
        let yaml = r#"
version: "3.8"
name: test-app

services:
  web:
    image: nginx:alpine
    cpus: 2
    memory: 512M
    ports:
      - "8080:80"
    volumes:
      - "./html:/usr/share/nginx/html:ro"
    env:
      APP_ENV: production
      PORT: 80
    restart: always

  db:
    image: redis:alpine
    cpus: 1
    memory: 256MiB
    ports:
      - "6379:6379"
    cmd: "redis-server --appendonly yes"
"#;

        let manifest = Manifest::parse(yaml).expect("Failed to parse standard compose YAML");
        let spec = manifest.into_compose_spec();

        assert_eq!(spec.name.as_deref(), Some("test-app"));
        assert_eq!(spec.services.len(), 2);

        let web = &spec.services["web"];
        assert_eq!(web.image, "nginx:alpine");
        assert_eq!(web.cpus, Some(2));
        assert_eq!(web.memory.as_deref(), Some("512M"));
        assert_eq!(web.ports, vec!["8080:80"]);
        assert_eq!(web.volumes, vec!["./html:/usr/share/nginx/html:ro"]);
        assert_eq!(web.env.get("APP_ENV").map(|s| s.as_str()), Some("production"));
        assert_eq!(web.env.get("PORT").map(|s| s.as_str()), Some("80"));

        let db = &spec.services["db"];
        assert_eq!(db.image, "redis:alpine");
        assert_eq!(
            db.cmd,
            Some(vec![
                "redis-server".to_string(),
                "--appendonly".to_string(),
                "yes".to_string()
            ])
        );
    }

    #[test]
    fn test_parse_k8s_microvm_crd_yaml() {
        let yaml = r#"
apiVersion: krun.io/v1alpha1
kind: MicroVm
metadata:
  name: isolated-ai-agent
  labels:
    tier: inference
spec:
  image: ghcr.io/ericlbuehler/mistral.rs:cpu-latest
  vcpus: 4
  memory: 8Gi
  cmd: ["mistralrs-server", "--port", "8000"]
  port: 8000
  gpu: true
  daxWindowSize: 4Gi
  allowEgress:
    - "api.anthropic.com:443"
"#;

        let manifest = Manifest::parse(yaml).expect("Failed to parse Kubernetes MicroVm CRD");
        let spec = manifest.into_compose_spec();

        assert_eq!(spec.name.as_deref(), Some("isolated-ai-agent"));
        let svc = &spec.services["isolated-ai-agent"];
        assert_eq!(svc.image, "ghcr.io/ericlbuehler/mistral.rs:cpu-latest");
        assert_eq!(svc.cpus, Some(4));
        assert_eq!(svc.memory.as_deref(), Some("8Gi"));
        assert_eq!(svc.gpu, Some(true));
        assert_eq!(svc.dax.as_deref(), Some("4Gi"));
        assert_eq!(svc.ports, vec!["8000:8000"]);
        assert_eq!(svc.allow_hosts, vec!["api.anthropic.com:443"]);
    }

    #[test]
    fn test_topological_sort_launch_order() {
        let mut services = BTreeMap::new();

        let mut frontend = ServiceSpec::default();
        frontend.image = "frontend:latest".to_string();
        frontend.depends_on = vec!["api".to_string()];

        let mut api = ServiceSpec::default();
        api.image = "api:latest".to_string();
        api.depends_on = vec!["db".to_string()];

        let mut db = ServiceSpec::default();
        db.image = "db:latest".to_string();

        services.insert("frontend".to_string(), frontend);
        services.insert("api".to_string(), api);
        services.insert("db".to_string(), db);

        let spec = ComposeSpec {
            version: "krun/v1".to_string(),
            name: Some("test-project".to_string()),
            services,
            volumes: BTreeMap::new(),
            networks: BTreeMap::new(),
        };

        let project = ComposeProject::new("test-project", spec, "/tmp", "/tmp");
        let order = project.resolve_launch_order().expect("Dependency sort failed");

        assert_eq!(order, vec!["db", "api", "frontend"]);
    }

    #[test]
    fn test_circular_dependency_detected() {
        let mut services = BTreeMap::new();

        let mut a = ServiceSpec::default();
        a.image = "a:latest".to_string();
        a.depends_on = vec!["b".to_string()];

        let mut b = ServiceSpec::default();
        b.image = "b:latest".to_string();
        b.depends_on = vec!["a".to_string()];

        services.insert("a".to_string(), a);
        services.insert("b".to_string(), b);

        let spec = ComposeSpec {
            version: "krun/v1".to_string(),
            name: Some("cyclic".to_string()),
            services,
            volumes: BTreeMap::new(),
            networks: BTreeMap::new(),
        };

        let project = ComposeProject::new("cyclic", spec, "/tmp", "/tmp");
        assert!(project.resolve_launch_order().is_err());
    }

    #[test]
    fn test_parse_env_list_and_map() {
        let yaml_list = r#"
image: alpine
env:
  - FOO=BAR
  - BAZ=123
"#;
        let svc: ServiceSpec = serde_yaml::from_str(yaml_list).unwrap();
        assert_eq!(svc.env.get("FOO").unwrap(), "BAR");
        assert_eq!(svc.env.get("BAZ").unwrap(), "123");

        let yaml_map = r#"
image: alpine
env:
  FOO: BAR
  BAZ: 123
"#;
        let svc2: ServiceSpec = serde_yaml::from_str(yaml_map).unwrap();
        assert_eq!(svc2.env.get("FOO").unwrap(), "BAR");
        assert_eq!(svc2.env.get("BAZ").unwrap(), "123");
    }
}
