# Kubernetes & containerd Integration Guide for libkrun

This directory provides configuration manifests, architecture documents, and deployment instructions for running hardware-isolated microVM Pods in Kubernetes using `krun-microvm` and `containerd`.

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
      Linux Namespaces                     libkrun (Hypervisor / KVM)
      (Shared Host Kernel)                        │
                                           Guest Linux MicroVM
                                           (Hardware Isolated Boundary)
```

## Prerequisites

1. **Host Virtualization**:
   - **Linux**: `/dev/kvm` accessible to the user/containerd.
   - **macOS**: Apple Silicon `Hypervisor.framework` (via containerd or Lima/Colima/OrbStack nodes).
2. **libkrun & libkrunfw**: Installed on the node host.
3. **containerd**: v1.6+ or v2.x.
4. **Kubernetes**: v1.20+ with CRI support enabled.

---

## Step 1: Build and Install Shim

Build the release binary of `containerd-shim-krun-v2`:

```bash
cargo build --release -p containerd-shim-krun -p microvm-runner

# Install the shim and runner to system binary path
sudo install -m 755 target/release/containerd-shim-krun-v2 /usr/local/bin/containerd-shim-krun-v2
sudo install -m 755 target/release/microvm-runner /usr/local/bin/microvm-runner
```

Verify the shim installation:
```bash
containerd-shim-krun-v2 --version
# Output: containerd-shim-krun-v2 (krun-microvm) version 0.1.0
```

---

## Step 2: Configure containerd

Merge the snippet from [`containerd-config.toml`](./containerd-config.toml) into `/etc/containerd/config.toml`:

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

For declarative Kubernetes management, run our pure-Rust operator powered by **`kube-rs`**:

1. **Install the `MicroVm` CustomResourceDefinition (CRD)**:
   ```bash
   # Apply the pre-generated CRD
   kubectl apply -f k8s/crd-microvm.yaml

   # (Optional) Export/regenerate schema directly from Rust structs:
   cargo run -p krun-operator -- --export-crd > k8s/crd-microvm.yaml
   ```

2. **Start the Operator**:
   ```bash
   cargo run -p krun-operator
   ```

3. **Deploy a MicroVM Custom Resource**:
   ```bash
   kubectl apply -f k8s/example-microvm-crd.yaml
   ```

4. **Monitor MicroVMs Declaratively**:
   ```bash
   kubectl get microvms
   # NAME                   PHASE     POD                         IP           AGE
   # mistral-7b-inference   Running   microvm-mistral-7b-infer…   10.244.0.5   12s
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


