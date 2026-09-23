# Kubernetes & containerd Integration Guide for libkrun

This directory provides configuration manifests, architecture documents, and deployment instructions for running hardware-isolated microVM Pods in Kubernetes using `krun-microvm`, `containerd`, and `krun-operator`.

> 📘 **Deep Dive Architecture**: For a comprehensive explanation of how `kube-rs`, `containerd-shim-krun`, and `libkrun` work together across all five system layers, see the **[Architecture Guide](ARCHITECTURE.md)**.
>
> ⚡ **Architectural Superiority vs AWS Firecracker**: For a detailed breakdown of why `krun-microvm` outperforms AWS's `firecracker-containerd` in speed, memory efficiency, and cross-platform flexibility, see the **[Firecracker Comparison & Benchmark Guide](FIRECRACKER_COMPARISON.md)**.

## Overview Architecture

```
                      Kubernetes API Server
                                │
                             Kubelet
                                │  (CRI gRPC)
                            containerd
                                │
              ┌─────────────────┴─────────────────┐
              │                                   │
       (Default runtime)                   (RuntimeClass: krun)
      runc / crun                          containerd-shim-krun-v2
              │                                   │
      Linux Namespaces                     libkrun (Hypervisor / KVM / Apple HVF)
      (Shared Host Kernel)                        │
                                           Guest Linux MicroVM
                                           (Hardware Isolated Boundary)
                                                  │
                                       ┌──────────┴──────────┐
                                       │                     │
                                  CNI Bridge         User-Space gvproxy
                                  (Host NetNS)      (Rootless User-Space)
```

## Prerequisites

1. **Host Virtualization**:
   - **Linux**: `/dev/kvm` accessible to the user/containerd.
   - **macOS**: Apple Silicon `Hypervisor.framework` (via containerd or Lima/Colima/OrbStack nodes).
2. **libkrun & libkrunfw**: Installed on the node host (`brew install libkrun libkrunfw` or via Linux distro packages).
3. **containerd**: v1.6+ or v2.x.
4. **Kubernetes**: v1.20+ with CRI support enabled.

---

## Step 1: Fast Automated Setup with `microvm containerd` CLI

The `microvm` CLI includes built-in commands to streamline containerd and shim configuration:

```bash
# 1. Install or symlink containerd-shim-krun-v2 to /usr/local/bin:
microvm containerd install

# 2. Generate the exact containerd CRI configuration snippet:
microvm containerd generate-config

# 3. Check installation, shim PATH detection, version, and containerd connectivity:
microvm containerd status
```

### Manual Installation (Alternative)

If you prefer building and installing manually:

```bash
cargo build --release -p containerd-shim-krun -p microvm-runner

# Install the shim and runner to system binary path
sudo install -m 755 target/release/containerd-shim-krun-v2 /usr/local/bin/containerd-shim-krun-v2
sudo install -m 755 target/release/microvm-runner /usr/local/bin/microvm-runner

# Verify the shim installation
containerd-shim-krun-v2 --version
# Output: containerd-shim-krun-v2 (krun-microvm) version 0.1.0
```

---

## Step 2: Configure containerd

Merge the snippet from [`containerd-config.toml`](./containerd-config.toml) (or the output of `microvm containerd generate-config`) into `/etc/containerd/config.toml`:

```toml
[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.krun]
  runtime_type = "io.containerd.krun.v2"
  runtime_engine = ""
  runtime_root = ""
  privileged_without_host_devices = false

  [plugins."io.containerd.grpc.v1.cri".containerd.runtimes.krun.options]
    BinaryName = "/usr/local/bin/containerd-shim-krun-v2"
```

Restart containerd:
```bash
sudo systemctl restart containerd
```

Verify containerd runtime detection:
```bash
microvm containerd status
```

---

## Step 3: Register Kubernetes RuntimeClass

Apply the RuntimeClass to your Kubernetes cluster:

```bash
kubectl apply -f k8s/runtimeclass.yaml
```

Verify that the RuntimeClass is available:
```bash
kubectl get runtimeclass
# NAME   HANDLER   AGE
# krun   krun      5s
```

---

## Step 4: Deploy a MicroVM Pod

Deploy the sample Pod:

```bash
kubectl apply -f k8s/example-pod.yaml
```

Check the pod status and logs:
```bash
kubectl get pods -w
kubectl logs microvm-demo
```

Notice the output confirming hardware virtualization isolation:
```
=== Hello from inside a libkrun microVM Pod! ===
Linux microvm-demo 6.1.x ...
Hardware virtualization boundary active.
```

---

## Step 5: Declarative Management with `krun-operator` (`kube-rs`)

For declarative Kubernetes management, `krun-operator` is a pure-Rust operator powered by **`kube-rs`** that reconciles `MicroVm` custom resources into hardware-isolated Pods backed by `containerd-shim-krun-v2`.

### Supported Declarative Features in `MicroVmSpec`

| Field | Type | Description |
|---|---|---|
| `image` | `string` | OCI image (e.g. `alpine:latest`, `ghcr.io/ericlbuehler/mistral.rs:cpu-latest`) |
| `vcpus` | `u8` | Allocated virtual CPUs (default: 2) |
| `memory` | `string` | Memory allocation (e.g. `"512Mi"`, `"4Gi"`, `"8Gi"`) |
| `cmd` | `string[]` | Command arguments override |
| `env` | `EnvVar[]` | In-guest environment variables |
| `ports` | `PortMapping[]` | Multi-port mappings with `hostPort`, `containerPort`, and `protocol` |
| `networkMode` | `string` | `"cni"` (Kubernetes default), `"gvproxy"` (rootless), `"tsi"`, or `"none"` |
| `allowEgress` | `string[]` | Outbound zero-trust allowlist (e.g. `["api.openai.com:443", "*.github.com:443"]`) |
| `dnsServers` | `string[]` | Custom DNS nameservers for in-guest resolution |
| `tokenBudget` | `u64` | Hard LLM cumulative token budget ceiling before requests are blocked |
| `sandbox` | `bool` | Host filesystem and capability sandboxing (default: true) |
| `daxWindowSize`| `string` | VirtioFS DAX window size for zero-copy mmap (e.g. `"4Gi"`, `"8Gi"`) |
| `gpu` | `bool` | Hardware-accelerated virtio-gpu (Metal on Apple Silicon, DRM on Linux) |
| `gpuShmSize` | `string` | Shared vRAM memory size for virtio-gpu |
| `imageAcceleration` | `ImageAccelerationSpec` | Dragonfly Nydus RAFSv6 / EROFS chunked lazy loading |
| `volumeMounts` | `VolumeMountSpec[]` | Host path mounts passed into the microVM |
| `paused` | `bool` | Declarative pause state: freezes/unfreezes vCPUs in single-digit milliseconds |

### 1. Install the `MicroVm` CustomResourceDefinition (CRD)

```bash
# Apply the pre-generated CRD:
kubectl apply -f k8s/crd-microvm.yaml

# (Optional) Export/regenerate schema directly from Rust structs:
cargo run --release -p krun-operator -- --export-crd > k8s/crd-microvm.yaml
```

### 2. Start the Operator

```bash
cargo run --release -p krun-operator
```

### 3. Deploy a MicroVM Custom Resource

```bash
kubectl apply -f k8s/example-microvm-crd.yaml
```

Example manifest ([`k8s/example-microvm-crd.yaml`](./example-microvm-crd.yaml)):
```yaml
apiVersion: krun.io/v1alpha1
kind: MicroVm
metadata:
  name: mistral-7b-inference
  namespace: default
spec:
  image: ghcr.io/ericlbuehler/mistral.rs:cpu-latest
  vcpus: 4
  memory: 8Gi
  daxWindowSize: 4Gi
  networkMode: cni
  sandbox: true
  ports:
    - name: http
      hostPort: 1234
      containerPort: 1234
      protocol: TCP
  allowEgress:
    - "huggingface.co:443"
    - "cdn-lfs.huggingface.co:443"
  cmd:
    - "mistralrs-server"
    - "--host"
    - "0.0.0.0"
    - "--port"
    - "1234"
    - "plain"
    - "-m"
    - "mistralai/Mistral-7B-Instruct-v0.2"
    - "--isq"
    - "Q4K"
  env:
    - name: RUST_LOG
      value: "info"
```

### 4. Monitor MicroVMs Declaratively

```bash
kubectl get microvms
# NAME                   PHASE     POD                         IP           AGE
# mistral-7b-inference   Running   microvm-mistral-7b-infer…   10.244.0.5   12s

# Inspect rich lifecycle conditions and status:
kubectl get microvm mistral-7b-inference -o yaml
```

---

## Step 6: Live Telemetry & Resource Monitoring (`crictl stats`)

`containerd-shim-krun-v2` implements the containerd TTRPC `Task::stats` and `Task::pids` interface, returning nanosecond-accurate CPU times (user and kernel), Resident Set Size (RSS) memory, page faults, and active threads encoded as standard `io.containerd.cgroups.v1.Metrics` inside `google.protobuf.Any`.

### 1. Inspect Live Metrics with `crictl stats`

```bash
# Get real-time resource utilization across all running microVM containers:
sudo crictl stats

# Sample Output:
# CONTAINER           CPU %               MEM                 DISK                INODES
# 5b3a88dfb70a1       1.42                68.5MB              0B                  0
# 8c9d12aef41b2       14.80               4.12GB              0B                  0
```

### 2. Inspect Detailed JSON Telemetry

```bash
# Query detailed CPU, memory, and thread metrics for a specific microVM:
sudo crictl stats <CONTAINER_ID> -o json
```

The response includes:
- **CPU Accounting**: User nanoseconds (`cpu.usage.user`), kernel nanoseconds (`cpu.usage.kernel`), total nanoseconds (`cpu.usage.total`).
- **Memory Accounting**: Resident Set Size (`memory.rss`, `memory.usage.usage`), page fault counts (`memory.pg_fault`, `memory.pg_maj_fault`).
- **Thread Count**: Number of active supervisor and vCPU threads (`pids.current`).

### 3. Kubernetes HPA & Prometheus Integration

Because `containerd-shim-krun` serializes metrics into the standard containerd cgroups protobuf format:
- **`kubectl top pods`**: Works natively to display CPU and memory consumption.
- **cAdvisor / Prometheus Node Exporter**: Automatically scrapes microVM container metrics for cluster dashboards and alerting.
- **Horizontal Pod Autoscaler (HPA)**: Can automatically scale inference workloads (e.g. `mistral.rs` microVMs) based on CPU/memory utilization thresholds.
