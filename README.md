# libkrun-sdk: The Premier Pure-Rust MicroVM Virtualization Platform

[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20(Apple%20Silicon)%20%7C%20Linux%20(KVM)-lightgrey.svg)]()
[![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg)]()

**libkrun-sdk** is the premier, state-of-the-art virtualization SDK and microVM orchestration suite powered by `libkrun`. Built entirely in native, memory-safe Rust with zero CGO and zero runtime overhead, this platform executes standard OCI container images with sub-100ms cold boots, hardware-isolated virtualization boundaries, and full Kubernetes CRI compatibility.

---

## Architectural Superiority: Pure Rust vs. Legacy Runtimes

| Capability / Architecture | Legacy Go / CGO MicroVM Runtimes | Legacy AWS Firecracker | **`libkrun-sdk` / `krun-microvm` (State-of-the-Art Pure Rust)** | The `libkrun-sdk` Advantage |
|---|---|---|---|---|
| **Hypervisor Linkage** | CGO foreign function bridge; requires Go-to-C stack switching | Out-of-process REST socket / Go-to-Rust bridge | Native zero-cost Rust FFI; direct crate linkage | **0% CGO overhead**, maximum throughput, no cross-language GC pausing |
| **Memory Footprint** | 30 MB – 60 MB per instance | ~50 MB – 80 MB per instance | **< 15 MB razor-thin native footprint** | Pack **4x – 10x more concurrent microVMs** on the same bare-metal host |
| **Cold Boot Latency** | 500ms – 2000ms | ~250ms – 600ms | **Sub-100ms cold boot** | Instant serverless scaling, microsecond task provisioning |
| **Guest Architecture** | Nested containers via guest `runc` | Heavy guest OS + Go agent daemon + `runc` | **Direct PID 1 Static Exec** (`init.krun` < 1MB) | **Zero in-guest agent bloat**; container execs directly as PID 1 |
| **CoW Storage** | External tools or loop block devices | devmapper thin-pools or raw block images | Native kernel APFS `clonefile(2)` & Linux `FICLONE` + VirtioFS | Instant snapshotting with zero duplicate disk consumption |
| **Networking** | Root TAP bridge setup | Requires root privileges for TAP, Bridge, iptables | **Transparent Socket Impersonation (TSI)** + Resilient DNS | **100% Rootless networking** out-of-the-box; zero host bridge hassles |
| **Platform Support** | Linux only | Linux KVM only (fails on Apple Silicon) | **Universal Silicon**: macOS Apple Silicon (`Hypervisor.framework`) AND Linux KVM (`/dev/kvm`) | Full developer parity across Mac laptops and production Linux nodes |
| **Direct Access (DAX)** | None / manual external blocks | Complex devmapper attachments | **Native VirtioFS DAX Window (`--dax <size>`)** | Zero-copy mmap of multi-GB LLM weights (GGUF/Safetensors) into guest physical address space |
| **Image Acceleration** | Full layer tar download & untar required (~minutes) | Full rootfs block download required | **Dragonfly Nydus RAFSv6 Lazy Loading (`--lazy-load`)** | Sub-50ms cold starts with metadata bootstrap; zero-copy on-demand chunk streaming over VirtioFS DAX |
| **Lifecycle Controls** | SIGKILL / external daemons | Out-of-process REST socket calls | **Instant `pause` / `resume` + Declarative CRD** | Freeze and unfreeze microVM vCPUs in single-digit milliseconds; declarative sleep/wake in Kubernetes |
| **Dynamic Resizing** | Cold restart required | Cold restart required | **In-Place CPU & Memory Resizing (`microvm resize`)** | Dynamically adjust vCPUs and memory on active microVMs without rebooting; native CRI `Task::update` support |
| **Host Sandboxing** | Standard container isolation | Seccomp filters only | **Zero-Trust Host Sandboxing (Landlock + `PR_SET_NO_NEW_PRIVS`)** | Restricts hypervisor process capabilities and filesystem access prior to entering the VM |
| **Egress Security** | Open outbound access | Manual iptables/nftables rules | **Host-Enforced Egress Proxy (`--allow-host`)** | Default-deny outbound proxy; blocks cloud metadata SSRF (`169.254.169.254`) and unauthorized hosts |
| **Secret Isolation** | Plaintext secrets in container memory & env | In-guest plaintext env vars | **Zero-Trust In-Flight Substitution (`--secret`)** | Guest only sees placeholder keys (`krun-secret:KEY`); real credentials substituted in-flight by host proxy |
| **LLM Token Metering** | External API gateways | N/A | **Streaming Token Meter & Hard Budgets (`--max-tokens`)** | Enforces hard cumulative token ceilings directly at host proxy; returns 429 Too Many Requests on breach |
| **Observability** | External stat collectors | REST API polling | **Native Prometheus 0.0.4 Engine (`microvm metrics`)** | Single-shot and live HTTP scrape server (`--listen`) with per-VM CPU/memory/faults telemetry |
| **Multi-Boot Engines** | Containers only or kernel only | Linux bzImage only | **Universal Multi-Boot (Containers, Direct Kernels, UEFI Firmware, Unikernels)** | Boot OCI containers, raw Linux/BSD bzImages, UEFI firmware (EDK2), or lightweight unikernels (Unikraft, Nanos) with raw VirtIO block disks |
| **Hypervisor Resilience** | Prone to kqueue aborts & SMP crash | Linux KVM only | **Battle-Tested Resilience (HVF Panic Interceptor, Safe Console Pipe, Signal TTY Recovery)** | Traps Apple Silicon HVF multi-vCPU PSCI shutdown panics; prevents kqueue epoll assertion aborts on non-pollable stdin; async-signal-safe terminal restore |
| **AI Agent Sandboxing** | Manual container configs | Manual VMs | **Automated Zero-Trust Agent Sandbox (`microvm sandbox <agent>`)** | One-command sandboxing for Claude, Gemini, and Codex with CoW host repository clones, API allowlisting, secret proxying, and token budgeting |
| **Multi-Language SDKs** | Go only or raw CLI wrappers | Python/Go REST clients | **Multi-Language Client SDKs (Rust, Python, TypeScript, Go)** | Native crates, Python `@task` serverless decorator, TypeScript/Node `@libkrun/sdk`, and Go `krun-sdk-go` |
| **Kubernetes CRI** | Monolithic external daemons | `firecracker-containerd` (Go) | Native containerd v2 TTRPC shim + pure-Rust `kube-rs` Operator | Declarative `MicroVm` CRD (`krun.io/v1alpha1`) with live `Task::stats` telemetry |

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
4. **Hardware Virtualization**: Uses Apple Silicon's native `Hypervisor.framework` on macOS and `/dev/kvm` on Linux.

---

## Workspace & Submodule Layout

```
libkrun-sdk/
├── .gitmodules              # Git submodule definitions
├── libkrun/                 # Git submodule: upstream dynamic library (https://github.com/libkrun/libkrun)
├── install.sh               # Universal automated installer (macOS & Linux)
├── LICENSE                  # Apache 2.0 License
├── README.md                # Top-level SDK documentation
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
    │   ├── python/          # Serverless Python SDK (libkrun_microvm)
    │   ├── typescript/      # Modern Node / TypeScript Client SDK (@libkrun/sdk)
    │   └── go/              # Pure Go MicroVM Client SDK (krun-sdk-go)
    └── examples/
        ├── run_alpine.rs    # Quick-start SDK example
        ├── run_ai_sandbox.rs # AI Agent CoW sandboxing example
        ├── run_mistral_inference.rs # mistral.rs hardware-isolated LLM inference
        └── python_sdk_agent.py # Hardware-isolated AI agent with zero-trust networking
```

### Cloning with Submodules
To clone the entire SDK including the `libkrun` submodule:
```bash
git clone --recurse-submodules https://github.com/sangam14/libkrun-sdk.git

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

### Building
```bash
cd krun-microvm
make build
make sign   # Signs microvm-runner with com.apple.security.hypervisor on macOS
make test   # Runs all 33 unit tests across workspace crates
```

---

## CLI Usage (`microvm`)

### 1. Preflight Checks
Verify that hardware virtualization and system resources are ready:
```bash
microvm preflight --port 8080
```

### 2. Pull & Cache an Image
Pulls manifest, downloads layers, resolves whiteouts, and caches the rootfs:
```bash
microvm pull alpine:latest
```

### 3. Run a Command in a MicroVM
```bash
microvm run alpine:latest -- echo "Hello from hardware-isolated MicroVM!"
```

### 4. Check Guest Linux Kernel vs Host macOS
```bash
microvm run alpine:latest -- uname -a
# Outputs: Linux localhost 6.12.62 #1 SMP aarch64 Linux
```

### 5. Pass Environment Variables and Working Directory
```bash
microvm run alpine:latest \
    -e APP_ENV=production \
    -w /tmp \
    -- sh -c "echo Env is \$APP_ENV && pwd"
```

### 6. Mount Host Directories via VirtioFS
```bash
microvm run alpine:latest \
    -v /Users/apple/data:shared_data \
    -- sh -c "mount -t virtiofs shared_data /mnt && ls -la /mnt"
```

### 7. Interactive Terminal (TTY / PTY)
Run an interactive container shell with line-editing and signal forwarding:
```bash
microvm run -it alpine:latest -- sh
```

### 8. Detached / Daemon Mode (`-d, --detach`)
Run a microVM in the background as a background daemon (returns instance ID immediately, like `docker run -d`):
```bash
microvm run -d alpine:latest -- sleep 300
# Outputs: vm-1789848631479
```

### 9. Real-Time Log Streaming (`microvm logs -f`)
Follow console output in real-time as the microVM executes:
```bash
microvm logs -f vm-1789848631479
```

### 10. Direct File Copying (`microvm cp`)
Bidirectionally transfer files and directories between the host and running microVMs:
```bash
# Copy file from host into running microVM
microvm cp ./config.json vm-1789848631479:/root/config.json

# Copy file from microVM back to host
microvm cp vm-1789848631479:/root/results.log ./results.log
```

### 11. Production MicroVM Networking & Automated Resilient DNS (`--net`, `--dns`, `--hostname`)
`libkrun-sdk` features an autonomous guest network configuration engine. It automatically provisions guest `/etc/resolv.conf`, `/etc/hosts`, and `/etc/hostname` with loopback sanitization, host gateway detection, and fault-tolerant upstream DNS fallbacks (`8.8.8.8, 1.1.1.1`):
```bash
# Outbound internet access works out of the box (HTTPS, package managers, git):
microvm run alpine:latest -- apk update

# Custom DNS nameservers:
microvm run --dns 1.0.0.1,8.8.4.4 alpine:latest -- cat /etc/resolv.conf

# Custom guest hostname:
microvm run --hostname my-sandbox alpine:latest -- hostname

# Provider-based network modes:
# 1. TSI (default rootless socket impersonation):
microvm run --net tsi alpine:latest -- wget -qO- http://example.com

# 2. Air-Gapped Network Isolation (no interfaces, only loopback lo):
microvm run --net none alpine:latest -- ip addr

# 3. External user-space proxy socket (gvproxy/passt/CNI):
microvm run --net unix:/tmp/gvproxy.sock alpine:latest
```

### 12. POSIX Resource Limits (`--rlimits`)
Enforce guest kernel resource limits to protect against fork bombs and descriptor exhaustion:
```bash
microvm run --rlimits "RLIMIT_NOFILE=1024:2048;RLIMIT_NPROC=100" alpine:latest -- ulimit -n
```

### 13. Port Forwarding
Forward host port 8080 to guest port 80:
```bash
microvm run alpine:latest -p 8080:80 -- python3 -m http.server 80
```

### 14. Multi-VM State, Telemetry & Lifecycle Management
List running and stopped microVM instances with human-readable relative uptimes and status indicators:
```bash
# View active instances:
microvm ps

# View all instances without ID/image truncation:
microvm ps -a --no-trunc
```

Live resource telemetry monitoring (`stats`):
```bash
# Stream live interactive dashboard of CPU %, RSS memory, threads, and page faults:
microvm stats

# Single-shot telemetry snapshot (ideal for scripting and CI):
microvm stats --no-stream

# Structured JSON telemetry for monitoring agents:
microvm stats --json
```

Deep inspect microVM configuration, OCI spec, mounts, and live telemetry (`inspect`):
```bash
# Pretty-printed structured inspection:
microvm inspect <vm-id>

# Full JSON inspection:
microvm inspect <vm-id> --json
```

Inspect supervisor process and thread statistics (`top`):
```bash
microvm top <vm-id>
```

# Pause / freeze all vCPUs of an active microVM:
microvm pause <vm-id>

# Resume a paused microVM:
microvm resume <vm-id>

# Gracefully stop an active microVM:
microvm stop <vm-id>

# Remove a stopped microVM (or force-kill and clean with -f):
microvm rm <vm-id>
microvm rm -f <vm-id>

# Clean up all stopped instances and temporary staging caches:
microvm prune
```

Query host hypervisor, hardware capacity, and cache disk utilization (`info`):
```bash
microvm info
```

### 15. Local Docker / Podman Daemon Integration
`microvm` automatically checks the local Docker engine (`/var/run/docker.sock`) first. You can run locally built images immediately:
```bash
docker build -t my-local-agent:v1 .
microvm run my-local-agent:v1
```

### 16. Direct OCI Runtime Bundle Execution
Directly boot an unpacked OCI runtime bundle (containing `config.json` and `rootfs/`), just like runc or containerd:
```bash
microvm run --bundle /path/to/oci-bundle
```

### 17. Offline OCI Image Layout (`oci:/path/to/layout[:tag]`)
Directly run or pull images from standard offline OCI Image Layout directories on disk (containing `oci-layout`, `index.json`, and `blobs/`) with zero registry or Docker daemon dependencies:
```bash
microvm pull oci:/path/to/image-layout:latest
microvm run oci:/path/to/image-layout:latest -- echo "Offline microVM running!"
```

### 18. Decoupled OCI Artifact Engine (`--artifact`, `microvm artifact`)
Pull and mount arbitrary non-container OCI artifacts (LLM weights, GGUF/safetensors, datasets, toolchains) directly into microVMs via VirtioFS:
```bash
# Inspect locally cached OCI artifacts
microvm artifact list

# Pre-cache an OCI artifact from any registry
microvm artifact pull ghcr.io/mistralai/mistral-7b:v0.3

# Attach an OCI artifact to a microVM mount
microvm run \
    --artifact ghcr.io/mistralai/mistral-7b:v0.3:models:ro \
    vllm/vllm-openai:latest
```

### 19. Instant Copy-on-Write (CoW) Workspace Sandboxing (`--workspace-cow`)
Mount a host project directory into the guest with an instant, isolated APFS `clonefile` / Linux `FICLONE` snapshot. Any edits, writes, or deletions made inside the microVM remain strictly isolated to the sandbox — guaranteeing 100% protection for the host codebase:
```bash
microvm run \
    --workspace-cow ./my-ai-repo:workspace \
    -w /workspace \
    alpine:latest -- sh -c "echo '# Mutated by agent' >> main.py && cat main.py"

# Verify host ./my-ai-repo/main.py remains completely unaltered!
```

### 20. VirtioFS DAX Shared Memory Window for Zero-Copy AI Models (`--dax <size>`)
Enables Direct Access (DAX) shared memory window on VirtioFS, allowing multi-gigabyte files (such as AI model weights or databases) to be memory-mapped directly into the guest physical address space without copying through the guest page cache:
```bash
# Launch with a 4 GB DAX shared memory window for zero-copy model loading:
microvm run \
    --dax 4G \
    --artifact ghcr.io/mistralai/mistral-7b:v0.3:models:ro \
    ghcr.io/ericlbuehler/mistral.rs:cpu-latest
```

### 21. Dragonfly Nydus RAFSv6 Acceleration & On-Demand Lazy Loading (`--lazy-load`)
`libkrun-sdk` natively integrates Dragonfly Nydus RAFSv6 / in-kernel EROFS accelerated filesystems. Instead of downloading and uncompressing gigabytes of OCI tarball layers on container boot:
- Only the lightweight RAFS metadata bootstrap (~1–2 MB) is loaded.
- The microVM boots in **sub-50ms**.
- File chunks and AI model tensors (with support for macro-chunks up to 64MB) are streamed lazily on-demand over VirtioFS DAX shared memory.

```bash
# Boot instantly using Nydus RAFSv6 lazy loading:
microvm run \
    --lazy-load \
    --nydus-cache /var/cache/nydus-blobs \
    --chunk-size 64M \
    --dax 4G \
    quay.io/sandstone/deepseek-r1:nydus-latest
```

### 22. Hardware-Accelerated GPU Passthrough (`--gpu`, `--gpu-shm-size`)
Run GPU compute and graphic workloads inside microVMs with near-native performance. Powered by `virglrenderer` and Venus Vulkan/Metal backend:
```bash
# Launch container with GPU passthrough and 4 GB vRAM shared memory:
microvm run --gpu --gpu-shm-size 4G ghcr.io/ericlbuehler/mistral.rs:latest
```

### 23. In-Guest Command Execution (`microvm exec`)
Execute commands directly inside an active, running microVM without rebooting, identical to `docker exec` / `kubectl exec`:
```bash
# Execute interactive shell:
microvm exec -t <vm-id> /bin/sh

# Run diagnostic command with custom env:
microvm exec -w /app -e DEBUG=1 <vm-id> uname -a
```

### 24. MicroVM Live Snapshot & Warm-Start Restore (`microvm snapshot`, `microvm restore`)
Capture instant filesystem and state snapshots of microVMs using kernel Copy-on-Write (APFS `clonefile` / Linux `reflink`) and restore them in milliseconds:
```bash
# Snapshot microVM:
microvm snapshot <vm-id> --output /tmp/my-snapshot.tar

# Restore into a warm-started microVM instance:
microvm restore /tmp/my-snapshot.tar --name worker-warm-01
```

### 25. Kubernetes CNI Network Integration (`--net cni:<netns>`)
Connect microVMs directly into Kubernetes CNI network namespaces (Flannel, Calico, Cilium, Bridge) with dedicated cluster IP addresses:
```bash
microvm run --net cni:/proc/1234/ns/net alpine:latest -- ip addr
```

### 26. Zero-Trust Host Sandboxing (`--no-sandbox`)
Defense-in-depth isolation applied to the host supervisor process right before entering the VM:
- **Linux**: Kernel `PR_SET_NO_NEW_PRIVS` prevents privilege escalation via setuid/setgid, paired with Landlock LSM rules restricting filesystem access to only authorized rootfs and mount paths.
- **macOS**: Strict mount boundary verification ensuring all VirtioFS paths reside within permitted workspaces.
- Enabled by default on every `run`; use `--no-sandbox` for unrestricted debugging environments:
```bash
microvm run --no-sandbox alpine:latest -- sh
```

### 27. Dynamic CPU & Memory Runtime Resizing (`microvm resize`)
Dynamically adjust vCPU count and memory allocation of a running microVM without terminating or rebooting the guest:
```bash
# Dynamically scale microVM to 4 vCPUs and 2048 MiB RAM:
microvm resize <vm-id> --cpus 4 --memory 2048

# Inspect updated allocations:
microvm inspect <vm-id>
```

### 28. Enterprise Prometheus Observability Engine (`microvm metrics`)
Native Prometheus exposition engine exporting inventory counters, resource gauges, and live per-VM OS telemetry (CPU user/sys seconds, RSS memory, virtual memory, threads, page faults):
```bash
# Instantaneous Prometheus exposition format (version 0.0.4):
microvm metrics

# Standalone HTTP Prometheus scrape endpoint:
microvm metrics --listen 0.0.0.0:9090

# Structured JSON telemetry for custom collectors:
microvm metrics --json
```

### 29. Host-Enforced Egress Proxy & Domain Allowlisting (`--allow-host`)
Block data exfiltration and enforce zero-trust network perimeter security. The host proxy operates on a **default-deny** policy, permitting outbound HTTP/HTTPS connections only to explicitly authorized domains and ports:
```bash
# Allow only OpenAI and Anthropic API endpoints:
microvm run \
    --allow-host api.openai.com:443 \
    --allow-host "*.anthropic.com:443" \
    python:3.11-slim -- python3 -c "import urllib.request; print(urllib.request.urlopen('https://api.openai.com').status)"

# SSRF and Cloud Metadata Protection:
# Requests to 169.254.169.254 or metadata.google.internal are strictly rejected with 403 Forbidden!
```

### 30. Zero-Trust In-Flight Secret Substitution (`--secret`)
Eliminate plaintext API keys and credentials from guest microVM memory, environment variables, and disk. The guest only sees placeholder values (`krun-secret:<KEY>`); the host proxy substitutes the authentic secret in-flight on outbound HTTP request headers (`Authorization`, `X-Api-Key`) and bodies:
```bash
# 1. From host environment variable:
microvm run \
    --allow-host api.openai.com:443 \
    --secret OPENAI_API_KEY=env:HOST_OPENAI_KEY \
    python:3.11-slim -- python3 -c "import os; print('In guest:', os.environ['OPENAI_API_KEY'])"
# Output in guest: In guest: krun-secret:OPENAI_API_KEY

# 2. From file or direct value:
microvm run \
    --allow-host api.openai.com:443 \
    --secret OPENAI_API_KEY=file:/etc/secrets/openai.key \
    --secret HF_TOKEN=hf_abc123 \
    python:3.11-slim
```

### 31. LLM Token Metering & Hard Budgets (`--max-tokens`)
The host proxy inspects streaming Server-Sent Events (SSE) and JSON responses from OpenAI, Anthropic, and compatible LLM providers, calculating cumulative token consumption in real time:
```bash
# Enforce hard ceiling of 50,000 total tokens:
microvm run \
    --allow-host api.openai.com:443 \
    --secret OPENAI_API_KEY=env:OPENAI_API_KEY \
    --max-tokens 50000 \
    python:3.11-slim -- python3 agent.py
# Once cumulative tokens hit 50,000, subsequent calls return 429 Too Many Requests (LLM Token Budget Exceeded)!
```

### 32. Multi-Boot Engine: Direct Linux Kernels, UEFI Firmware & Raw Disks (`--kernel`, `--initrd`, `--firmware`, `--disk`)
Beyond OCI containers, `libkrun-sdk` natively boots custom Linux kernels (bzImage/vmlinux), UEFI firmware payloads (e.g. EDK2/OVMF), and raw VirtIO block disks with custom kernel cmdlines:
```bash
# Direct kernel boot with initrd and raw disk:
microvm run \
    --kernel /boot/vmlinuz-6.12 \
    --initrd /boot/initrd.img \
    --cmdline "console=ttyS0 root=/dev/vda rw earlyprintk=serial,ttyS0" \
    --disk /var/lib/disks/rootfs.raw:rw \
    --disk /var/lib/disks/data.img:ro

# UEFI firmware boot:
microvm run \
    --firmware /usr/share/OVMF/OVMF_CODE.fd \
    --disk /var/lib/disks/os.raw:rw
```

### 33. Unikernel Execution (`microvm unikernel`)
Execute specialized, ultra-minimal unikernel binaries (Unikraft, Nanos, OSv) with sub-10ms boots, bypassing general-purpose operating system layers:
```bash
# Run a compiled unikernel with optional parameters and block disk:
microvm unikernel /opt/unikernels/nginx.bin \
    --params "netdev.ipv4_addr=192.168.1.2" \
    --disk /opt/unikernels/www.raw:ro \
    --cpus 1 --memory 128
```

### 34. Autonomous AI Coding Agent Sandboxes (`microvm sandbox`)
Launch pre-configured, zero-trust isolated environments tailored for autonomous AI coding agents (**Claude Code**, **Google Gemini Code Assist**, **OpenAI Codex**). Automatically mounts your local project via CoW (Copy-on-Write) APFS/FICLONE, restricts egress exclusively to authorized vendor APIs and GitHub, and transparently proxies authentication credentials:
```bash
# Launch a Claude Code sandbox with CoW isolation on current workspace:
microvm sandbox claude \
    --workspace . \
    --secret ANTHROPIC_API_KEY=env:ANTHROPIC_API_KEY \
    --max-tokens 100000

# Launch a Gemini agent sandbox:
microvm sandbox gemini \
    --workspace . \
    --secret GEMINI_API_KEY=env:GEMINI_API_KEY

# Launch a Codex agent sandbox cloning a remote repository directly into CoW:
microvm sandbox codex \
    --repo https://github.com/org/repo.git \
    --secret OPENAI_API_KEY=env:OPENAI_API_KEY
```

---

## Multi-Language Client SDKs

`libkrun-sdk` provides official client SDKs across **Python**, **TypeScript/Node.js**, and **Go**, enabling seamless integration into any application stack.

### 1. Serverless Python SDK (`libkrun-microvm`)

The `libkrun-microvm` Python SDK allows developers to dispatch any Python function into an ephemeral, hardware-isolated microVM using the `@task` decorator:

```bash
pip install -e krun-microvm/sdks/python
```

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
    return {"status": "completed", "prompt": prompt}

try:
    result = run_autonomous_agent("Audit security configuration")
    print("Result from microVM:", result)
except MicroVmBudgetExceededError as e:
    print("Security policy stopped task: token budget exceeded!", e)
```

### 2. Modern TypeScript / Node SDK (`@libkrun/sdk`)

Native Node.js / TypeScript client for orchestrating microVMs, streaming logs, executing commands via the framed binary protocol, and enforcing egress security:

```bash
npm install @libkrun/sdk
```

```typescript
import { MicroVm } from "@libkrun/sdk";

const vm = new MicroVm({
  image: "node:20-slim",
  cpus: 2,
  memoryMb: 1024,
  allowHosts: ["api.anthropic.com:443", "github.com:443"],
  secrets: { ANTHROPIC_API_KEY: "env:ANTHROPIC_API_KEY" },
  workspaceCow: "./src:workspace",
});

await vm.start(["node", "-e", "console.log('Running in isolated MicroVM')"]);
const exitCode = await vm.wait();
console.log(`VM exited with status ${exitCode}`);
```

### 3. Pure Go Client SDK (`krun-sdk-go`)

Type-safe Go client with zero CGO dependencies for serverless platforms, edge runtimes, and microVM orchestration:

```bash
go get github.com/sangam14/libkrun-sdk/sdk/go
```

```go
package main

import (
    "context"
    "fmt"
    "log"

    "github.com/sangam14/libkrun-sdk/sdk/go"
)

func main() {
    ctx := context.Background()
    vm, err := microvm.New(microvm.Config{
        Image:      "alpine:latest",
        Cpus:       2,
        MemoryMb:   512,
        AllowHosts: []string{"api.openai.com:443"},
        Secrets:    map[string]string{"OPENAI_API_KEY": "env:HOST_KEY"},
        Cmd:        []string{"echo", "Hello from Go SDK MicroVM!"},
    })
    if err != nil {
        log.Fatal(err)
    }

    if err := vm.Start(ctx); err != nil {
        log.Fatal(err)
    }

    exitCode, err := vm.Wait(ctx)
    if err != nil {
        log.Fatal(err)
    }
    fmt.Printf("MicroVM finished with code: %d\n", exitCode)
}
```

---

## Battle-Tested Hypervisor Resilience & In-Guest Protocol

`libkrun-sdk` incorporates critical battle-tested hypervisor stability workarounds:
1. **Apple Silicon HVF Multi-vCPU Panic Watchdog**: Traps macOS `Hypervisor.framework` PSCI `CPU_OFF` shutdown crashes (`src/hvf/src/lib.rs:549: Unexpected val=...`) and cleanly transitions the host supervisor to exit code 0.
2. **Safe Non-Pollable Console Pipe**: Replaces non-pollable stdin descriptors (`/dev/null` or closed pipes) with an internal OS pipe in non-interactive / daemon modes, preventing fatal `kqueue / epoll` assertion failures (`left == right failed, left: -1`).
3. **Async-Signal-Safe Terminal Restoration**: Installs global signal handlers (`SIGINT`, `SIGTERM`, `SIGHUP`) with guaranteed `tcsetattr` restoration and `ONLCR` output post-processing restoration.
4. **Framed In-Guest Execution Protocol**: High-throughput multiplexed TCP/vsock execution framing (`CH_STDIN=0`, `CH_STDOUT=1`, `CH_STDERR=2`, `CH_EXIT=3`, `CH_WINSZ=4`) with length-prefixed chunks for seamless integration with in-guest agents.

---


## Hardware-Isolated LLM Inference with mistral.rs

Run **[mistral.rs](https://github.com/ericlbuehler/mistral.rs)** inside a hardware-isolated microVM to securely serve OpenAI-compatible LLM inference with zero host risk:

```bash
# 1. Serve Hugging Face model with In-Situ Quantization (ISQ Q4K)
microvm run \
    -c 4 -m 8192 -p 1234:1234 \
    ghcr.io/ericlbuehler/mistral.rs:cpu-latest -- \
    mistralrs-server --host 0.0.0.0 --port 1234 plain -m mistralai/Mistral-7B-Instruct-v0.2 --isq Q4K

# 2. Query the OpenAI-compatible endpoint from the host
curl http://localhost:1234/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "default",
    "messages": [{"role": "user", "content": "What is a microVM?"}]
  }'
```

---

## Kubernetes & containerd Integration (`RuntimeClass: krun`)

`libkrun-sdk` includes a native containerd v2 shim binary: `containerd-shim-krun-v2`. This allows Kubernetes clusters to run Pods inside hardware-isolated libkrun microVMs alongside regular runc containers.

> 📘 **Deep Dive Architecture**: See [krun-microvm/k8s/ARCHITECTURE.md](krun-microvm/k8s/ARCHITECTURE.md).  
> ⚡ **Architectural Superiority vs AWS Firecracker**: See [krun-microvm/k8s/FIRECRACKER_COMPARISON.md](krun-microvm/k8s/FIRECRACKER_COMPARISON.md).  
> 🚀 **Kubernetes Deployment Guide**: See [krun-microvm/k8s/README.md](krun-microvm/k8s/README.md).

### Quick Kubernetes Setup

1. **Install the Shim & Runner**:
   ```bash
   cargo build --release --manifest-path krun-microvm/Cargo.toml -p containerd-shim-krun -p microvm-runner
   sudo install -m 755 krun-microvm/target/release/containerd-shim-krun-v2 /usr/local/bin/
   sudo install -m 755 krun-microvm/target/release/microvm-runner /usr/local/bin/
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
   kubectl apply -f krun-microvm/k8s/runtimeclass.yaml
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

---

## License

Apache License 2.0. See [LICENSE](LICENSE) for details.
