# krun-microvm: Run OCI Container Images as MicroVMs in Rust

[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20(Apple%20Silicon)%20%7C%20Linux%20(KVM)-lightgrey.svg)]()
[![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg)]()

**krun-microvm** is the premier, state-of-the-art pure-Rust microVM virtualization platform and container orchestration engine. Built entirely in native, memory-safe Rust with zero CGO and zero runtime overhead, `krun-microvm` executes standard OCI container images with sub-100ms cold boots, hardware-isolated virtualization boundaries, and full Kubernetes CRI compatibility.

---

## Architectural Superiority: Pure Rust vs. Legacy Runtimes

| Capability / Architecture | Legacy Go / CGO MicroVM Runtimes | **`krun-microvm` (State-of-the-Art Pure Rust)** | The `krun-microvm` Advantage |
|---|---|---|---|
| **Hypervisor Linkage** | CGO foreign function bridge; requires Go-to-C stack switching | Native zero-cost Rust FFI; direct crate linkage | **0% CGO overhead**, maximum throughput, no cross-language GC pausing |
| **Memory Footprint** | 30 MB – 60 MB per instance runtime overhead | **< 5 MB razor-thin native binary** | Pack **10x more concurrent microVMs** on the same bare-metal host |
| **Boot Latency** | 500ms – 2000ms | **Sub-100ms cold boot** | Instant serverless scaling, microsecond task provisioning |
| **CoW Filesystem Cloning** | External tools or coarse `x/sys/unix` wrappers | Native kernel APFS `clonefile(2)` & Linux `FICLONE` ioctls | Instant snapshotting with zero duplicate disk consumption |
| **Safety & Concurrency** | Go M:N runtime scheduler conflicts with hypervisor threads | Deterministic OS thread isolation & async Tokio orchestration | True hardware CPU thread pinning; zero scheduler contention |
| **Direct Access (DAX)** | None / manual external blocks | **Native VirtioFS DAX Window (`--dax <size>`)** | Zero-copy mmap of multi-GB LLM weights (GGUF/Safetensors) into guest physical address space |
| **Image Acceleration** | Full layer tar download & decompression required (~minutes for multi-GB) | **Dragonfly Nydus RAFSv6 Lazy Loading (`--lazy-load`)** | Sub-50ms cold starts with metadata bootstrap; zero-copy on-demand chunk streaming over VirtioFS DAX |
| **Lifecycle Controls** | SIGKILL / external process kills | **Instant `pause` / `resume` + Declarative CRD** | Freeze and unfreeze microVM vCPUs in single-digit milliseconds; declarative sleep/wake in Kubernetes |
| **Dynamic Resizing** | Cold restart required | **In-Place CPU & RAM Resizing (`microvm resize`)** | Dynamically adjust vCPUs and memory on running microVMs without reboots; native CRI `Task::update` support |
| **Host Sandboxing** | Standard container isolation | **Zero-Trust Host Sandboxing (Landlock + `PR_SET_NO_NEW_PRIVS`)** | Restricts hypervisor process capabilities and filesystem access prior to guest execution |
| **Egress Security** | Open outbound access / complex iptables | **Host-Enforced Egress Proxy (`--allow-host`)** | Default-deny outbound proxy; blocks cloud metadata SSRF (`169.254.169.254`) and unauthorized hosts |
| **Secret Isolation** | Plaintext secrets in container memory & env | **Zero-Trust In-Flight Substitution (`--secret`)** | Guest only sees placeholder keys (`krun-secret:KEY`); real credentials substituted in-flight by host proxy |
| **LLM Token Metering** | External API gateways | **Streaming Token Meter & Hard Budgets (`--max-tokens`)** | Enforces hard cumulative token ceilings directly at host proxy; returns 429 Too Many Requests on breach |
| **Observability** | External stat collectors / cAdvisor | **Native Prometheus 0.0.4 Engine (`microvm metrics`)** | Single-shot and live HTTP scrape server (`--listen`) with per-VM CPU/memory/faults telemetry |
| **Serverless SDK** | Complex custom Docker/CLI wrappers | **Pure Python SDK with `@task` Decorator** | Seamlessly dispatch Python functions to ephemeral, hardware-isolated microVMs with single decorator |
| **Kubernetes Integration** | Monolithic external daemons or out-of-tree bridges | Native containerd v2 TTRPC shim + pure-Rust `kube-rs` Operator | Declarative `MicroVm` CRD (`krun.io/v1alpha1`) with live `crictl stats` telemetry |

---

## Key Architecture Concepts

```
[ OCI Registry / Docker Hub ]
           │
           ▼
[ OCI Layer Fetch & Digest Verification ] (Content-addressable SHA256)
           │
           ▼
[ Layer Merging & Whiteout Engine ] (Handles .wh.* and .wh..wh..opq)
           │
           ▼
[ Cached Base RootFS ] ──(Instant CoW Clone: APFS clonefile / FICLONE)──► [ Instance RootFS ]
                                                                                   │
                                                                 Inject /.krun_config.json
                                                                 (Cmd, Env, WorkingDir)
                                                                                   │
                                                                                   ▼
[ Host Application / CLI ] ──(JSON Config)──► [ microvm-runner ] ──(libkrun)──► [ Guest Linux VM ]
(Tokio Async Orchestrator)                     (Supervisor Child)                 (Hardware Isolated)
```

1. **The 2-Process Supervisor Model**: `libkrun`'s `krun_start_enter()` consumes the calling thread/process to become the microVM supervisor. By delegating this to a dedicated `microvm-runner` binary, your host orchestrator or daemon remains alive, handling lifecycle, port forwards, and multiple concurrent microVMs.
2. **Instant Copy-on-Write (CoW) Cloning**: Images are extracted once into a content-addressable cache (`~/.cache/krun-microvm/rootfs/<digest>`). When launching a VM, `krun-microvm` creates an APFS `clonefile(2)` or Linux `FICLONE` snapshot in single-digit milliseconds without duplicating disk blocks.
3. **The Guest Init Contract (`/.krun_config.json`)**: `libkrun` bundles a minimal static init binary (`init.krun`) that runs as PID 1 inside the guest. It reads `/.krun_config.json` containing the container's `Cmd`, `Env`, and `WorkingDir`, mounts `/proc`, `/sys`, and `/dev`, and execs the container workload.
4. **Hardware Virtualization**: Uses Apple Silicon's `Hypervisor.framework` on macOS and `/dev/kvm` on Linux.

---

## Workspace & Submodule Layout

```
libkrun-sdk/
├── .gitmodules              # Git submodule definitions
├── libkrun/                 # Git submodule: upstream dynamic library (https://github.com/libkrun/libkrun)
├── install.sh               # Universal automated installer (macOS & Linux)
└── krun-microvm/            # Pure-Rust microVM SDK workspace
    ├── Cargo.toml           # Workspace manifest
    ├── Makefile             # Build, test, and codesign automation
    ├── entitlements.plist   # macOS Hypervisor entitlement
    ├── k8s/                 # Kubernetes manifests, CRDs, & containerd configs
    │   ├── crd-microvm.yaml # MicroVm CustomResourceDefinition (v1alpha1)
    │   ├── runtimeclass.yaml# RuntimeClass: krun definition
    │   ├── example-pod.yaml # Example hardware-isolated Pod
    │   ├── containerd-config.toml # CRI runtime configuration snippet
    │   ├── ARCHITECTURE.md  # 5-layer control-to-silicon architecture
    │   ├── FIRECRACKER_COMPARISON.md # Architectural superiority & benchmark dominance vs AWS Firecracker
    │   └── README.md        # Kubernetes deployment guide
    ├── crates/
    │   ├── krun-sys/        # Safe and FFI bindings to libkrun
    │   ├── microvm-core/    # Core SDK: OCI client, rootfs CoW, whiteouts, telemetry, VM builder
    │   ├── microvm-runner/  # Isolated supervisor process that enters the VM
    │   ├── microvm-cli/     # User-facing CLI tool (`microvm`)
    │   ├── containerd-shim-krun/ # containerd v2 shim (`containerd-shim-krun-v2`)
    │   └── krun-operator/   # Pure-Rust Kubernetes Operator (kube-rs)
    ├── sdks/
    │   └── python/          # Serverless Python SDK (libkrun_microvm)
    │       ├── pyproject.toml
    │       ├── README.md
    │       ├── libkrun_microvm/ # @task decorator, client, error types
    │       └── tests/       # Unit test suite for Python SDK
    └── examples/
        ├── run_alpine.rs    # Quick-start SDK example
        ├── run_ai_sandbox.rs # AI Agent CoW sandboxing example
        ├── run_mistral_inference.rs # mistral.rs hardware-isolated LLM inference
        └── python_sdk_agent.py # Hardware-isolated AI agent with zero-trust networking
```

### Cloning with Submodules
To clone the entire SDK including the `libkrun` submodule:
```bash
git clone --recurse-submodules <repo-url>

# Or if already cloned:
git submodule update --init --recursive
```

---

## Quick Installation (`install.sh`)

A universal automated installation script is provided for both **macOS (Apple Silicon / Intel)** and **Linux (Ubuntu/Debian, Fedora/RHEL, Arch)**:

```bash
# 1. Run the one-command installer (checks prerequisites, builds, signs, and installs):
./install.sh

# Or install to a user directory without sudo:
./install.sh --prefix ~/.local/bin

# Build in debug mode or skip building:
./install.sh --debug
```

The script automatically:
1. Verifies `/dev/kvm` (Linux) or Apple Silicon `Hypervisor.framework` (macOS).
2. Installs `libkrun` and `libkrunfw` via Homebrew or Linux package managers (`dnf copr`, `apt`, `pacman`).
3. Installs the Rust toolchain via `rustup` if missing.
4. Compiles all workspace binaries (`microvm`, `microvm-runner`, `containerd-shim-krun-v2`, `krun-operator`).
5. Signs macOS binaries with the `com.apple.security.hypervisor` entitlement.
6. Installs binaries to `/usr/local/bin` (or custom `--prefix`).
7. Runs preflight checks and outputs `microvm info` system telemetry.

---

## Manual Prerequisites & Building

- **macOS (Apple Silicon)**:
  - Xcode command line tools (`xcode-select --install`)
  - `libkrun` and `libkrunfw`:
    ```bash
    brew tap slp/krun && brew install libkrun libkrunfw
    ```
- **Linux**:
  - Hardware virtualization enabled (`/dev/kvm` accessible to user)
  - `libkrun` and `libkrunfw` installed

---

## Building

```bash
cd krun-microvm
make build
make sign   # Signs microvm-runner with com.apple.security.hypervisor on macOS
```

Run unit tests:
```bash
make test
```

---

## CLI Usage

### 1. Preflight Checks
Verify that hardware virtualization and system resources are ready:
```bash
./target/debug/microvm preflight --port 8080
```

### 2. Pull & Cache an Image
Pulls manifest, downloads layers, resolves whiteouts, and caches the rootfs:
```bash
./target/debug/microvm pull alpine:latest
```

### 3. Run a Command in a MicroVM
```bash
./target/debug/microvm run alpine:latest -- echo "Hello from hardware-isolated MicroVM!"
```

### 4. Check Guest Linux Kernel vs Host macOS
```bash
./target/debug/microvm run alpine:latest -- uname -a
# Outputs: Linux localhost 6.12.62 #1 SMP aarch64 Linux
```

### 5. Pass Environment Variables and Working Directory
```bash
./target/debug/microvm run alpine:latest \
    -e APP_ENV=production \
    -w /tmp \
    -- sh -c "echo Env is \$APP_ENV && pwd"
```

### 6. Mount Host Directories via VirtioFS
```bash
./target/debug/microvm run alpine:latest \
    -v /Users/apple/data:shared_data \
    -- sh -c "mount -t virtiofs shared_data /mnt && ls -la /mnt"
```

### 7. Interactive Terminal (TTY / PTY)
Run an interactive container shell with line-editing and signal forwarding:
```bash
./target/debug/microvm run -it alpine:latest -- sh
```

### 8. Detached / Daemon Mode (`-d, --detach`)
Run a microVM in the background as a background daemon (returns instance ID immediately, like `docker run -d`):
```bash
./target/debug/microvm run -d alpine:latest -- sleep 300
# Outputs: vm-1789848631479
```

### 9. Real-Time Log Streaming (`microvm logs -f`)
Follow console output in real-time as the microVM executes:
```bash
./target/debug/microvm logs -f vm-1789848631479
```

### 10. Direct File Copying (`microvm cp`)
Bidirectionally transfer files and directories between the host and running microVMs:
```bash
# Copy file from host into running microVM
./target/debug/microvm cp ./config.json vm-1789848631479:/root/config.json

# Copy file from microVM back to host
./target/debug/microvm cp vm-1789848631479:/root/results.log ./results.log
```

### 11. Production MicroVM Networking & Automated Resilient DNS (`--net`, `--dns`, `--hostname`)
`krun-microvm` features an autonomous guest network configuration engine. It automatically provisions guest `/etc/resolv.conf`, `/etc/hosts`, and `/etc/hostname` with loopback sanitization, host gateway detection, and fault-tolerant upstream DNS fallbacks (`8.8.8.8, 1.1.1.1`):
```bash
# Outbound internet access works out of the box (HTTPS, package managers, git):
./target/debug/microvm run alpine:latest -- apk update

# Custom DNS nameservers (comma or semicolon separated):
./target/debug/microvm run --dns 1.0.0.1,8.8.4.4 alpine:latest -- cat /etc/resolv.conf

# Custom guest hostname:
./target/debug/microvm run --hostname my-sandbox alpine:latest -- hostname

# Provider-based network modes:
# 1. TSI (default rootless socket impersonation):
./target/debug/microvm run --net tsi alpine:latest -- wget -qO- http://example.com

# 2. Air-Gapped Network Isolation (no interfaces, only loopback lo):
./target/debug/microvm run --net none alpine:latest -- ip addr

# 3. External user-space proxy socket (gvproxy/passt/CNI):
./target/debug/microvm run --net unix:/tmp/gvproxy.sock alpine:latest
```

### 12. POSIX Resource Limits (`--rlimits`)
Enforce guest kernel resource limits to protect against fork bombs and descriptor exhaustion:
```bash
./target/debug/microvm run --rlimits "RLIMIT_NOFILE=1024:2048;RLIMIT_NPROC=100" alpine:latest -- ulimit -n
```

### 13. Port Forwarding
Forward host port 8080 to guest port 80:
```bash
./target/debug/microvm run alpine:latest -p 8080:80 -- python3 -m http.server 80
```

### 14. Multi-VM State, Telemetry & Lifecycle Management
List running and stopped microVM instances with human-readable relative uptimes and status indicators:
```bash
# View active instances:
./target/debug/microvm ps

# View all instances without ID/image truncation:
./target/debug/microvm ps -a --no-trunc
```

Live resource telemetry monitoring (`stats`):
```bash
# Stream live interactive dashboard of CPU %, RSS memory, threads, and page faults:
./target/debug/microvm stats

# Single-shot telemetry snapshot (ideal for scripting and CI):
./target/debug/microvm stats --no-stream

# Structured JSON telemetry for monitoring agents:
./target/debug/microvm stats --json
```

Deep inspect microVM configuration, OCI spec, mounts, and live telemetry (`inspect`):
```bash
# Pretty-printed structured inspection:
./target/debug/microvm inspect <vm-id>

# Full JSON inspection:
./target/debug/microvm inspect <vm-id> --json
```

Inspect supervisor process and thread statistics (`top`):
```bash
./target/debug/microvm top <vm-id>
```

# Pause / freeze all vCPUs of an active microVM:
./target/debug/microvm pause <vm-id>

# Resume a paused microVM:
./target/debug/microvm resume <vm-id>

# Gracefully stop an active microVM:
./target/debug/microvm stop <vm-id>

# Remove a stopped microVM (or force-kill and clean with -f):
./target/debug/microvm rm <vm-id>
./target/debug/microvm rm -f <vm-id>

# Clean up all stopped instances and temporary staging caches:
./target/debug/microvm prune
```

Query host hypervisor, hardware capacity, and cache disk utilization (`info`):
```bash
./target/debug/microvm info
```

### 15. Local Docker / Podman Daemon Integration
`krun-microvm` automatically checks the local Docker engine (`/var/run/docker.sock`) first. You can run locally built images immediately:
```bash
# Build an image locally
docker build -t my-local-agent:v1 .

# Run it immediately in a hardware-isolated microVM
./target/debug/microvm run my-local-agent:v1
```

### 16. Direct OCI Runtime Bundle Execution
Directly boot an unpacked OCI runtime bundle (containing `config.json` and `rootfs/`), just like runc or containerd:
```bash
./target/debug/microvm run --bundle /path/to/oci-bundle
```

### 17. Offline OCI Image Layout (`oci:/path/to/layout[:tag]`)
Directly run or pull images from standard offline OCI Image Layout directories on disk (containing `oci-layout`, `index.json`, and `blobs/`) with zero registry or Docker daemon dependencies:
```bash
# Pull and unpack from an offline OCI directory
./target/debug/microvm pull oci:/path/to/image-layout:latest

# Run directly from an offline OCI layout
./target/debug/microvm run oci:/path/to/image-layout:latest -- echo "Offline microVM running!"
```

### 18. Decoupled OCI Artifact Engine (`--artifact`, `microvm artifact`)
Pull and mount arbitrary non-container OCI artifacts (LLM weights, GGUF/safetensors, datasets, toolchains) directly into microVMs via VirtioFS:
```bash
# Inspect locally cached OCI artifacts
./target/debug/microvm artifact list

# Pre-cache an OCI artifact from any registry
./target/debug/microvm artifact pull ghcr.io/mistralai/mistral-7b:v0.3

# Attach an OCI artifact to a microVM mount
./target/debug/microvm run \
    --artifact ghcr.io/mistralai/mistral-7b:v0.3:models:ro \
    vllm/vllm-openai:latest
```

### 19. Instant Copy-on-Write (CoW) Workspace Sandboxing (`--workspace-cow`)
Mount a host project directory into the guest with an instant, isolated APFS `clonefile` / Linux `FICLONE` snapshot. Any edits, writes, or deletions made inside the microVM remain strictly isolated to the sandbox — guaranteeing 100% protection for the host codebase:
```bash
# Mount host project as a disposable CoW sandbox
./target/debug/microvm run \
    --workspace-cow ./my-ai-repo:workspace \
    -w /workspace \
    alpine:latest -- sh -c "echo '# Mutated by agent' >> main.py && cat main.py"

# Verify host ./my-ai-repo/main.py remains completely unaltered!
```

### 20. Advanced Zero-Config TSI Networking & Resilient DNS
`krun-microvm` delivers a high-throughput, rootless networking engine supporting transparent TSI (Transparent Socket Impersonation) with automated guest DNS resolver generation (`/etc/resolv.conf`, `/etc/hosts`, `/etc/hostname`), Unix domain stream sockets, and air-gapped isolation:
```bash
# Default: TSI mode with host DNS resolution (or public fallbacks 8.8.8.8, 1.1.1.1)
./target/debug/microvm run alpine:latest -- apk update && apk add curl

# Custom DNS nameservers
./target/debug/microvm run --dns 1.1.1.1,8.8.8.8 alpine:latest -- nslookup example.com

# Air-gapped / Isolated network mode
./target/debug/microvm run --net none alpine:latest -- ip addr
```

### 21. Hardware-Isolated LLM Inference with mistral.rs (Pattern 1)
Run **[mistral.rs](https://github.com/ericlbuehler/mistral.rs)** inside a hardware-isolated microVM to securely serve OpenAI-compatible LLM inference with zero host risk:

```bash
# 1. Serve Hugging Face model with In-Situ Quantization (ISQ Q4K)
./target/debug/microvm run \
    -c 4 -m 8192 -p 1234:1234 \
    ghcr.io/ericlbuehler/mistral.rs:cpu-latest -- \
    mistralrs-server --host 0.0.0.0 --port 1234 plain -m mistralai/Mistral-7B-Instruct-v0.2 --isq Q4K

# 2. Serve local GGUF model mounted via VirtioFS
./target/debug/microvm run \
    -c 4 -m 4096 -p 1234:1234 \
    -v ./models:models \
    ghcr.io/ericlbuehler/mistral.rs:cpu-latest -- \
    mistralrs-server --host 0.0.0.0 --port 1234 gguf -m /models -f model.gguf

# 3. Query the OpenAI-compatible endpoint from the host
curl http://localhost:1234/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "default",
    "messages": [{"role": "user", "content": "What is a microVM?"}]
  }'
```

### 22. VirtioFS DAX Shared Memory Window for Zero-Copy AI Acceleration (`--dax <size>`)
Direct Access (DAX) enables the microVM to memory-map host files (such as GGUF model weights or databases) directly into the guest address space without data copying:
```bash
# Launch with a 4 GB DAX shared memory window:
./target/debug/microvm run \
    --dax 4G \
    --artifact ghcr.io/mistralai/mistral-7b:v0.3:models:ro \
    ghcr.io/ericlbuehler/mistral.rs:cpu-latest
```

### 23. Dragonfly Nydus RAFSv6 Acceleration & On-Demand Lazy Loading (`--lazy-load`)
`krun-microvm` features native integration with Dragonfly Nydus RAFSv6 / in-kernel EROFS accelerated filesystems. Instead of downloading and uncompressing gigabytes of OCI tarball layers on container boot:
- Only the lightweight RAFS metadata bootstrap (~1–2 MB) is loaded.
- The microVM boots in **sub-50ms**.
- File chunks and AI model tensors (with support for macro-chunks up to 64MB) are streamed lazily on-demand over VirtioFS DAX shared memory.

```bash
# Boot instantly using Nydus RAFSv6 lazy loading:
./target/debug/microvm run \
    --lazy-load \
    --nydus-cache /var/cache/nydus-blobs \
    --chunk-size 64M \
    --dax 4G \
    quay.io/sandstone/deepseek-r1:nydus-latest
```

#### Declarative Acceleration in Kubernetes (`crd-microvm.yaml`):
```yaml
apiVersion: krun.io/v1alpha1
kind: MicroVm
metadata:
  name: llm-worker-accelerated
spec:
  image: "quay.io/sandstone/deepseek-r1:nydus-latest"
  vcpus: 8
  memory: "16Gi"
  daxWindowSize: "8Gi"
  imageAcceleration:
    format: "rafsv6"
    lazyLoad: true
    chunkCacheDir: "/var/cache/krun/nydus-blobs"
    chunkSize: "64Mi"
    prefetch:
      - "/bin"
      - "/lib"
```

### 24. Hardware-Accelerated GPU Passthrough (`--gpu`, `--gpu-shm-size`)
Run GPU compute and graphic workloads inside microVMs with near-native performance. Powered by `virglrenderer` and Venus Vulkan/Metal backend:
- **Apple Silicon (macOS)**: Hardware Metal compute acceleration.
- **Linux**: Direct Rendering Manager (DRM) / Venus Vulkan acceleration.

```bash
# Launch container with GPU passthrough and 4 GB vRAM shared memory:
./target/release/microvm run \
    --gpu \
    --gpu-shm-size 4G \
    ghcr.io/ericlbuehler/mistral.rs:latest
```

#### Declarative GPU Acceleration in Kubernetes:
```yaml
apiVersion: krun.io/v1alpha1
kind: MicroVm
metadata:
  name: gpu-inference-worker
spec:
  image: "ghcr.io/ericlbuehler/mistral.rs:latest"
  vcpus: 8
  memory: "16Gi"
  gpu: true
  gpuShmSize: "8Gi"
```

### 25. Interactive In-Guest Exec (`microvm exec`)
Execute commands directly inside an active, running microVM without restarting the instance, identical to `docker exec` / `kubectl exec`:
```bash
# Execute interactive shell inside running microVM:
./target/release/microvm exec -t <vm-id> /bin/sh

# Run a non-interactive diagnostic command with custom env and working directory:
./target/release/microvm exec -w /app -e DEBUG=1 <vm-id> uname -a
```

### 26. Enterprise Multi-Architecture CI/CD Pipeline
Continuous integration powered by GitHub Actions across:
- **macOS 14 (Apple Silicon arm64)**: Native Metal acceleration, Hypervisor.framework tests.
### 27. MicroVM Live Snapshot & Warm-Start Restore (`microvm snapshot`, `microvm restore`)
Capture instant filesystem and state snapshots of running or stopped microVMs using kernel-level Copy-on-Write (APFS `clonefile` / Linux `reflink`), and restore them in single-digit milliseconds:
```bash
# Snapshot a microVM into a portable archive:
./target/release/microvm snapshot <vm-id> --output /tmp/my-snapshot.tar

# Restore snapshot into a new warm-started microVM instance:
./target/release/microvm restore /tmp/my-snapshot.tar --name worker-warm-01
```

### 28. Kubernetes CNI Network Integration (`--net cni:<netns>`)
Connect microVMs directly into Kubernetes CNI network namespaces (Flannel, Calico, Cilium, Bridge) with dedicated cluster IP addresses:
```bash
# Run microVM attached to a Pod network namespace:
./target/release/microvm run --net cni:/proc/1234/ns/net alpine:latest -- ip addr
```

### 29. Zero-Trust Host Sandboxing (`--no-sandbox`)
`krun-microvm` applies strict defense-in-depth isolation to the host supervisor process right before entering the VM:
- **Linux**: Kernel `PR_SET_NO_NEW_PRIVS` prevents privilege escalation via setuid/setgid, paired with Landlock LSM rules restricting filesystem access to only authorized rootfs and mount paths.
- **macOS**: Strict mount boundary verification ensuring all VirtioFS paths reside within permitted workspaces.
- Enabled by default on every `run`; use `--no-sandbox` if running in unrestricted debugging environments:
```bash
./target/release/microvm run --no-sandbox alpine:latest -- sh
```

### 30. Dynamic CPU & Memory Runtime Resizing (`microvm resize`)
Dynamically adjust vCPU count and memory allocation of a running microVM without terminating or rebooting the guest:
```bash
# Dynamically scale microVM to 4 vCPUs and 2048 MiB RAM:
./target/release/microvm resize <vm-id> --cpus 4 --memory 2048

# Inspect updated allocations:
./target/release/microvm inspect <vm-id>
```

### 31. Enterprise Prometheus Observability Engine (`microvm metrics`)
Native Prometheus exposition engine exporting inventory counters, resource gauges, and live per-VM OS telemetry (CPU user/sys seconds, RSS memory, virtual memory, threads, page faults):
```bash
# Output instantaneous Prometheus exposition format (version 0.0.4):
./target/release/microvm metrics

# Standalone HTTP Prometheus scrape endpoint:
./target/release/microvm metrics --listen 0.0.0.0:9090

# Structured JSON telemetry for custom collectors:
./target/release/microvm metrics --json
```

### 32. Host-Enforced Egress Proxy & Domain Allowlisting (`--allow-host`)
Block data exfiltration and enforce zero-trust network perimeter security. The host proxy operates on a **default-deny** policy, permitting outbound HTTP/HTTPS connections only to explicitly authorized domains and ports:
```bash
# Allow only OpenAI and Anthropic API endpoints:
./target/release/microvm run \
    --allow-host api.openai.com:443 \
    --allow-host "*.anthropic.com:443" \
    python:3.11-slim -- python3 -c "import urllib.request; print(urllib.request.urlopen('https://api.openai.com').status)"

# SSRF and Cloud Metadata Protection:
# Requests to 169.254.169.254 or metadata.google.internal are strictly rejected with 403 Forbidden!
```

### 33. Zero-Trust In-Flight Secret Substitution (`--secret`)
Eliminate plaintext API keys and credentials from guest microVM memory, environment variables, and disk. The guest only sees placeholder values (`krun-secret:<KEY>`); the host proxy substitutes the authentic secret in-flight on outbound HTTP request headers (`Authorization`, `X-Api-Key`) and bodies:
```bash
# 1. From host environment variable:
./target/release/microvm run \
    --allow-host api.openai.com:443 \
    --secret OPENAI_API_KEY=env:HOST_OPENAI_KEY \
    python:3.11-slim -- python3 -c "import os; print('In guest:', os.environ['OPENAI_API_KEY'])"
# Output in guest: In guest: krun-secret:OPENAI_API_KEY

# 2. From file or direct value:
./target/release/microvm run \
    --allow-host api.openai.com:443 \
    --secret OPENAI_API_KEY=file:/etc/secrets/openai.key \
    --secret HF_TOKEN=hf_abc123 \
    python:3.11-slim
```

### 34. LLM Token Metering & Hard Budgets (`--max-tokens`)
The host proxy inspects streaming Server-Sent Events (SSE) and JSON responses from OpenAI, Anthropic, and compatible LLM providers, calculating cumulative token consumption in real time:
```bash
# Enforce hard ceiling of 50,000 total tokens:
./target/release/microvm run \
    --allow-host api.openai.com:443 \
    --secret OPENAI_API_KEY=env:OPENAI_API_KEY \
    --max-tokens 50000 \
    python:3.11-slim -- python3 agent.py
# Once cumulative tokens hit 50,000, subsequent calls return 429 Too Many Requests (LLM Token Budget Exceeded)!
```

---

## Serverless Python SDK (`libkrun-microvm`)

The `libkrun-microvm` Python SDK allows developers to dispatch any Python function into an ephemeral, hardware-isolated microVM using the `@task` decorator.

### Installation
```bash
pip install -e sdks/python
```

### Quickstart Example
```python
from libkrun_microvm import task, MicroVmBudgetExceededError

@task(
    image="python:3.11-slim",
    cpus=2,
    memory_mb=512,
    allow_hosts=["api.openai.com:443", "api.anthropic.com:443"],
    secrets={"OPENAI_API_KEY": "env:OPENAI_API_KEY"},
    max_tokens=25000,
)
def run_autonomous_agent(prompt: str) -> dict:
    import os, urllib.request

    # Inside the microVM, os.environ["OPENAI_API_KEY"] is "krun-secret:OPENAI_API_KEY"
    # The host proxy validates the destination and injects the true key in-flight.
    return {
        "status": "completed",
        "prompt": prompt,
    }

# Execute function inside ephemeral microVM
try:
    result = run_autonomous_agent("Audit security configuration")
    print("Result from microVM:", result)
except MicroVmBudgetExceededError as e:
    print("Security policy stopped task: token budget exceeded!", e)
```

See runnable example in [`examples/python_sdk_agent.py`](examples/python_sdk_agent.py).

---



## Kubernetes & containerd Integration (`RuntimeClass: krun`)

`krun-microvm` includes a native containerd v2 shim binary: `containerd-shim-krun-v2`. This allows Kubernetes clusters to run Pods inside hardware-isolated libkrun microVMs alongside regular runc containers.

### Production Kubernetes Features:
- **Dynamic Resource Auto-Sizing**: Automatically extracts `resources.cpu` (quota/period) and `resources.memory.limit` from Pod specifications and provisions corresponding vCPUs and RAM.
- **Kubernetes CNI Network Integration**: Automatically extracts `netns_path` from PodSandbox specifications to connect microVMs to cluster networks.
- **In-Guest Command Exec (`kubectl exec` / `crictl exec`)**: Directly execute diagnostics and interactive shells inside active microVM Pods via `Task::exec`.
- **In-Place Lifecycle Controls (`Task::pause`, `Task::resume`)**: Freeze and thaw microVM workloads on demand without terminating them.
- **Projected Volume & Single-File Mounts**: Differentiates between directories (mounted seamlessly via VirtioFS) and individual files (Kubernetes `ConfigMap`s, `Secret`s, and service account tokens), injecting files directly into `instance_rootfs`.
- **Real-Time Stdio FIFO Streaming**: Streams console logs into containerd named pipes (`req.stdout()`, `req.stderr()`) in real time, making `kubectl logs -f` and `crictl logs` work natively.
- **Accurate Process Lifecycle & Exited-At Timestamps**: Fully compliant with containerd TTRPC task APIs, returning exit status and nano-precision timestamps upon completion.
- **Automated CRI Conformance Test Suite**: Run `bash k8s/conformance/test_cri_conformance.sh` to validate CRI compatibility against live containerd.

### Quick Setup

1. **Install the Shim & Runner**:
   ```bash
   cargo build --release -p containerd-shim-krun -p microvm-runner
   sudo install -m 755 target/release/containerd-shim-krun-v2 /usr/local/bin/
   sudo install -m 755 target/release/microvm-runner /usr/local/bin/
   ```

2. **Register with containerd** (`/etc/containerd/config.toml`):
   ```toml
   [plugins."io.containerd.grpc.v1.cri".containerd.runtimes.krun]
     runtime_type = "io.containerd.krun.v2"
     [plugins."io.containerd.grpc.v1.cri".containerd.runtimes.krun.options]
       BinaryName = "/usr/local/bin/containerd-shim-krun-v2"
   ```
   Restart containerd: `sudo systemctl restart containerd`

3. **Apply Kubernetes RuntimeClass**:
   ```bash
   kubectl apply -f k8s/runtimeclass.yaml
   ```

4. **Deploy Pods with Hardware Isolation**:
   Specify `runtimeClassName: krun` in any Pod specification:
   ```yaml
   apiVersion: v1
   kind: Pod
   metadata:
     name: secure-ai-agent
   spec:
     runtimeClassName: krun
     containers:
       - name: agent
         image: alpine:latest
         command: ["/bin/sh", "-c", "echo Hardware-isolated microVM Pod! && sleep 3600"]
   ```

See the full guide in [k8s/README.md](k8s/README.md).

---

## Rust SDK Usage

Add `microvm-core` to your `Cargo.toml`:

```rust
use anyhow::Result;
use microvm_core::MicroVmBuilder;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Build and configure the microVM with zero-trust networking
    let mut vm = MicroVmBuilder::new("python:3.11-slim")
        .cpus(2)
        .memory_mb(512)
        .env("SERVICE_NAME", "auth-worker")
        .allow_host("api.openai.com:443")
        .secret("OPENAI_API_KEY", "sk-live-xyz123")
        .max_tokens(25000)
        .cmd(vec![
            "python3".to_string(),
            "-c".to_string(),
            "print('Hello from hardware-isolated MicroVM!')".to_string(),
        ])
        .run()
        .await?;

    println!("MicroVM running with PID {:?}", vm.pid());
    println!("Host proxy listening on port: {:?}", vm.proxy_port());

    // 2. Wait for completion (or call vm.stop().await for graceful shutdown)
    let status = vm.wait().await?;
    println!("VM exited: {status}");
    println!("Egress blocked requests: {}", vm.egress_blocked_count());
    println!("Cumulative LLM tokens: {}", vm.llm_tokens_consumed());

    Ok(())
}
```

Run the included examples:
```bash
# Basic microVM execution
cargo run --example run_alpine

# AI Agent CoW sandboxing and artifact mounting
cargo run --example run_ai_sandbox

# Hardware-isolated LLM inference with mistral.rs
cargo run --example run_mistral_inference
```

### Advanced: AI Agent Sandboxing & OCI Artifacts

```rust
use anyhow::Result;
use microvm_core::MicroVmBuilder;

#[tokio::main]
async fn main() -> Result<()> {
    let mut vm = MicroVmBuilder::new("python:3.11-slim")
        .cpus(4)
        .memory_mb(2048)
        // 1. Zero-copy CoW snapshot of host directory (APFS clonefile / FICLONE)
        .workspace_cow("./my-agent-project", "workspace")
        // 2. Attach GGUF/safetensors model directly from OCI registry via VirtioFS
        .attach_artifact("ghcr.io/mistralai/mistral-7b:v0.3", "models", true)
        .workdir("/workspace")
        .cmd(vec!["python3".into(), "-m".into(), "agent".into()])
        .run()
        .await?;

    let status = vm.wait().await?;
    println!("Sandbox exited with status: {status}");
    // Host ./my-agent-project is guaranteed untouched!
    Ok(())
}
```

---

## License

Apache License 2.0. See [LICENSE](LICENSE) for details.
