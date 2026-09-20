# Kubernetes CRI & containerd Conformance Suite

This directory contains the automated end-to-end conformance testing suite for validating `containerd-shim-krun-v2` against containerd and Kubernetes Container Runtime Interface (CRI) standards.

---

## What the Suite Validates

1. **containerd Configuration & Runtime Registration**:
   - Verifies that `/etc/containerd/config.toml` includes `io.containerd.krun.v2`.
   - Confirms binary linkage and hypervisor entitlements for `containerd-shim-krun-v2`.

2. **PodSandbox Creation (`crictl runp`)**:
   - Validates CNI network namespace assignment.
   - Tests DNS configuration (`resolv.conf`, search domains, options).
   - Validates port mapping rules.

3. **Container Lifecycle (`create` & `start`)**:
   - Parses OCI bundle `config.json` and extracts memory limits, vCPU quotas, and volume mounts.
   - Ensures microVM supervisor spawns cleanly in detached mode.

4. **In-Guest Exec (`crictl exec`)**:
   - Tests process creation inside the running microVM rootfs.
   - Verifies standard streams (stdout/stderr) are captured and piped to containerd.

5. **Resource Telemetry (`crictl stats`)**:
   - Collects live CPU, RSS memory, thread counts, and page faults via `Task::stats`.

6. **Pause & Resume (`Task::pause`, `Task::resume`)**:
   - Validates freezing and unfreezing of microVM processes using POSIX signals.

---

## Running the Conformance Suite

### 1. Dry-Run / Local Verification (macOS / Linux without root containerd)
```bash
./k8s/conformance/test_cri_conformance.sh --dry-run
```

### 2. Live containerd Testing (Kubernetes node / Linux VM)
Ensure containerd is running with the krun runtime registered:

```bash
# Register shim in /usr/local/bin
sudo cp ./target/release/containerd-shim-krun-v2 /usr/local/bin/
sudo cp ./target/release/microvm-runner /usr/local/bin/

# Run the test suite against the live containerd socket
sudo ./k8s/conformance/test_cri_conformance.sh
```
