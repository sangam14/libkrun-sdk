<div align="center">

# ⚡ krun-microvm

### The Premier Pure-Rust MicroVM Virtualization & Orchestration Platform

[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg?style=for-the-badge)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20(Apple%20Silicon)%20%7C%20Linux%20(KVM)-black.svg?style=for-the-badge&logo=apple)](https://github.com/sangam14/libkrun-sdk)
[![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg?style=for-the-badge&logo=rust)](https://www.rust-lang.org)
[![Cold Boot](https://img.shields.io/badge/boot_time-%3C100ms-success.svg?style=for-the-badge&logo=lightning)](https://github.com/sangam14/libkrun-sdk)
[![Compose](https://img.shields.io/badge/orchestration-Docker%20Compose%20%7C%20K8s%20CRD-purple.svg?style=for-the-badge&logo=docker)](examples/krun-compose.yaml)

<p align="center">
  <b>Run standard OCI container images and AI workloads inside hardware-isolated microVMs.</b><br>
  Sub-100ms cold boot times • Apple Silicon Metal & Linux DRM Venus GPU acceleration • Native Docker Compose orchestration • Zero-trust AI agent sandboxing • Zero CGO & zero runtime overhead.
</p>

[Quickstart](#-quickstart-in-30-seconds) • [Why libkrun-sdk?](#-why-libkrun-sdk) • [Compose & YAML](#-declarative-compose--yaml) • [Architecture](#-architecture--comparison) • [AI Sandboxing](#-ai-agent-sandboxing--gpu) • [Kubernetes](#-kubernetes--containerd-integration) • [Asahi & m1n1](#-asahi-linux--m1n1-boot-integration-apple-silicon) • [SDKs](#-multi-language-client-sdks)

---

</div>

## 🌟 Overview

**krun-microvm** is a modern, high-performance virtualization SDK and microVM orchestration suite powered by [`libkrun`](https://github.com/libkrun/libkrun). Built from the ground up in memory-safe Rust, it transforms ordinary OCI container images (from Docker Hub, GHCR, or local registries) into hardware-isolated virtual machines in **under 100 milliseconds**.

Whether you are building **AI coding agent sandboxes** (Claude, Gemini, Codex), running **multi-container microVM stacks with Docker Compose**, deploying **isolated serverless functions**, or orchestrating **Kubernetes Pods with hardware virtualization boundaries**, `krun-microvm` delivers true hypervisor isolation with native developer ergonomics.

---

## ⚡ Quickstart in 30 Seconds

### 1. One-Line Installation
```bash
# Automated installer for macOS (Apple Silicon) and Linux:
./install.sh
```

### 2. Run Any OCI Container in a MicroVM
```bash
# Boots a hardware-isolated Linux microVM in <100ms:
microvm run alpine:latest -- echo "🚀 Hello from libkrun microVM!"

# Check guest Linux kernel (running isolated on macOS Apple Silicon or Linux KVM):
microvm run alpine:latest -- uname -a
# Linux localhost 6.12.91 #1 SMP aarch64 Linux
```

### 3. Deploy Multi-Service Stacks with Docker Compose
```bash
# Start multi-microVM stack (Redis + Web proxy) in background:
microvm compose up -f examples/krun-compose.yaml -d

# Check live microVM status and port forwards:
microvm compose ps -f examples/krun-compose.yaml

# Stream aggregated service logs:
microvm compose logs -f

# Gracefully stop and clean up:
microvm compose down -f examples/krun-compose.yaml
```

### 4. Launch an Isolated AI Coding Agent Sandbox with Git Cherry-Pick
```bash
# Isolated workspace with copy-on-write filesystem, git cherry-pick, and auto-sync:
microvm sandbox claude \
    --workspace . \
    --cherry-pick 7f4a2b1 \
    --apply-to-host \
    --secret ANTHROPIC_API_KEY=env:ANTHROPIC_API_KEY

# Or cherry-pick any past sandbox commits on demand:
microvm cherry-pick <sandbox-id>
```

---

## 🚀 Why libkrun-sdk?

<table>
<tr>
<td width="33%" valign="top">

### ⚡ Sub-100ms Cold Boots
Direct PID 1 static execution (`init.krun`) and instant APFS `clonefile` / Linux `FICLONE` Copy-on-Write rootfs snapshotting. No guest VM image building, no cloud image bloat.

</td>
<td width="33%" valign="top">

### 🛡️ True Hardware Isolation
Hardware virtualization boundary powered by Apple Silicon's `Hypervisor.framework` on macOS and `/dev/kvm` on Linux. Protection against kernel exploits, container escapes, and host tampering.

</td>
<td width="33%" valign="top">

### 🐳 Declarative Compose & CRDs
Drop-in compatibility with **Docker Compose** (`krun-compose.yaml`) and **Kubernetes MicroVm CRDs** (`krun.io/v1alpha1`). Topological dependency resolution with cycle detection.

</td>
</tr>
<tr>
<td width="33%" valign="top">

### 🏎️ GPU Passthrough & DAX
Hardware-accelerated virtio-gpu (Apple Silicon Metal & Linux DRM Venus). Native **VirtioFS DAX window** for zero-copy memory-mapped LLM weights (GGUF, Safetensors).

</td>
<td width="33%" valign="top">

### 🔒 Zero-Trust AI Sandboxing
Automated agent sandboxes with egress domain allowlists (blocks AWS metadata SSRF `169.254.169.254`), in-flight secret substitution, and streaming LLM token budget meters.

</td>
<td width="33%" valign="top">

### ☸️ Native Kubernetes CRI
Official containerd v2 TTRPC shim (`containerd-shim-krun-v2`) and pure-Rust `kube-rs` Operator. Run isolated Pods alongside runc containers with `runtimeClassName: krun`.

</td>
</tr>
</table>

---

## 📊 Architectural Dominance: libkrun-sdk vs. Alternatives

| Capability | Legacy Docker / runc | AWS Firecracker | Legacy Kata Containers (QEMU) | **`libkrun-sdk` (Pure Rust)** |
|---|:---:|:---:|:---:|:---:|
| **Isolation Boundary** | OS Namespaces / cgroups | Hardware MicroVM (KVM) | Heavy VM (QEMU) | **Hardware MicroVM (`libkrun`)** |
| **Apple Silicon (macOS) Support** | ❌ (requires Linux VM) | ❌ (Linux KVM only) | ❌ (Linux only) | **✅ Native (`Hypervisor.framework`)** |
| **Linux KVM Support** | ✅ | ✅ | ✅ | **✅ Native (`/dev/kvm`)** |
| **Cold Boot Latency** | ~300ms – 1s | ~250ms – 600ms | ~1500ms – 3000ms | **⚡ Sub-100ms (`<90ms`)** |
| **Hypervisor Memory Overhead** | ~30 MB – 50 MB | ~50 MB – 80 MB | ~120 MB – 250 MB | **⚡ < 15 MB razor-thin** |
| **Docker Compose Orchestration** | ✅ (Standard) | ❌ (External tooling) | ❌ | **✅ Native (`microvm compose`)** |
| **GPU / Metal Passthrough** | ❌ (Emulated/None on Mac) | ❌ | Complex VFIO | **✅ Native Metal & DRM Venus** |
| **VirtioFS DAX (LLM Weights)** | ❌ | ❌ | Complex block setup | **✅ Zero-Copy Shared Memory Window** |
| **Zero-Trust Egress & Secret Proxy** | ❌ | ❌ (Manual iptables) | ❌ | **✅ Built-in Host Security Proxy** |
| **Direct OCI Execution** | ✅ | ❌ (Requires raw rootfs) | ✅ | **✅ Direct OCI Image & Registry Pull** |

---

## 🐳 Declarative Compose & YAML

`libkrun-sdk` provides first-class support for multi-container microVM orchestration via Docker Compose YAML and Kubernetes CRD manifests.

### 1. Multi-Service MicroVM Spec (`krun-compose.yaml`)
Define your entire multi-microVM topology with dependencies, CPU/RAM allocations, VirtioFS host mounts, port forwards, and environment variables:

```yaml
version: "3.8"
name: demo-stack

services:
  # Isolated Redis Cache MicroVM
  redis:
    image: redis:alpine
    cpus: 1
    memory: 256M
    ports:
      - "6379:6379"
    volumes:
      - "./redis-data:/data"
    command: ["redis-server", "--appendonly", "yes"]

  # Isolated Web Proxy MicroVM
  web:
    image: nginx:alpine
    cpus: 2
    memory: 512M
    depends_on:
      - redis
    ports:
      - "18080:80"
    environment:
      APP_ENV: production
      CACHE_HOST: 127.0.0.1
    volumes:
      - "./html:/usr/share/nginx/html:ro"
```

### 2. Compose CLI Commands
```bash
# Start all microVM services in topological order (redis first, then web):
microvm compose up -d

# Check status of running microVM services:
microvm compose ps

# Follow aggregated logs across all microVMs with service prefixes:
microvm compose logs -f

# Validate and inspect resolved configuration:
microvm compose config

# Stop and gracefully clean up all microVM instances and networks:
microvm compose down
```

### 3. Kubernetes Declarative MicroVm CRD (`microvm apply`)
Deploy single-microVM declarative manifests matching Kubernetes `krun.io/v1alpha1` CRD format:

```yaml
apiVersion: krun.io/v1alpha1
kind: MicroVm
metadata:
  name: alpine-agent
spec:
  image: alpine:latest
  vcpus: 2
  memory: 512M
  port: 18080
  networkMode: tsi
  workspaceCow: "./workspace:workspace"
  sandbox: true
  allowEgress:
    - "*.github.com:443"
    - "api.openai.com:443"
  cmd:
    - "/bin/sh"
    - "-c"
    - "echo '🚀 Running inside hardware-isolated microVM!' && sleep 1"
```

```bash
# Apply manifest directly with one command:
microvm apply -f examples/microvm.yaml -d

# Or execute with microvm run:
microvm run -f examples/microvm.yaml -d
```

---

## 🤖 AI Agent Sandboxing & GPU

### 1. Zero-Trust Coding Agent Sandboxes
Safely execute autonomous coding agents (Claude, Gemini, Codex, Dev) without exposing your host machine or sensitive credentials:

```bash
# Run an autonomous coding sandbox with Copy-on-Write host mount:
microvm sandbox claude \
    --workspace /path/to/my-repo \
    --secret ANTHROPIC_API_KEY=env:ANTHROPIC_API_KEY \
    --cpus 4 \
    --memory 2048
```

- **Isolated Copy-on-Write (CoW)**: Agent modifications remain isolated in temporary CoW storage; your host directory is untouched until committed.
- **In-Flight Secret Substitution**: Injected API keys are replaced by the host proxy in-flight (`krun-secret:ANTHROPIC_API_KEY`); the guest VM never holds the raw credential in RAM.
- **LLM Token Ceilings**: Enforce hard token budgets with `--max-tokens 50000` to prevent runaway API spend.

### 2. Isolated Git Cherry-Pick & Bi-Directional Commit Syncing
Test feature branches or pull requests inside a safe microVM without altering your local working directory:

```bash
# 🍒 1. Cherry-pick a remote or feature commit directly into the isolated sandbox:
microvm sandbox dev \
    --workspace . \
    --cherry-pick 3a7b9c1 \
    --apply-to-host

# 🍒 2. Auto-forward host Git identity & bypass container 'dubious ownership' issues:
# The microVM automatically injects your host git author credentials and enables safe.directory

# 🍒 3. Cherry-pick commits back to host on demand from any historical sandbox:
microvm cherry-pick <microvm-id> -w .
```

- **In-Guest Cherry-Pick**: Pulls and cherry-picks target commit inside the isolated container before launching the agent or interactive shell.
- **Automatic Host Application (`--apply-to-host`)**: Detects new git commits made by the AI agent in the CoW workspace and cleanly applies them via `git am --3way` upon exit.
- **Zero Risk**: If the cherry-pick conflicts or the agent breaks the codebase, your host files and git history remain pristine.

### 3. Hardware-Accelerated Local LLM Inference
Leverage Apple Silicon Metal or Linux DRM Venus GPU passthrough:

```bash
# Boot mistral.rs inside an isolated microVM with Apple Silicon Metal acceleration:
microvm run \
    --gpu --gpu-shm-size 4G \
    --dax 4G \
    -p 1234:1234 \
    ghcr.io/ericlbuehler/mistral.rs:latest
```

---

## 🌐 Advanced MicroVM Networking & Zero-Trust Security

`krun-microvm` provides a versatile, defense-in-depth networking architecture engineered for everything from local rootless development to multi-tenant cloud Kubernetes clusters:

```mermaid
graph TD
    subgraph "MicroVM Guest Isolation"
        APP["Container Application / AI Agent"]
        GK["Guest Linux Kernel"]
        APP -->|"POSIX Sockets (AF_INET/AF_UNIX)"| GK
    end

    subgraph "Networking Abstraction Layer"
        TSI["⚡ TSI: Transparent Socket Impersonation (AF_VSOCK Bypass)"]
        GVP["🔀 gvproxy: Rootless Virtio-Net Stack (DHCP + DNS + NAT)"]
        CNI_NET["☸️ CNI: Kubernetes Pod Network Namespace (passt / veth)"]
        TAP_NET["🏎️ TAP: Line-Rate Linux Bridge (tap0 -> br0)"]
        AIR["🛡️ None: Strict Air-Gapped Isolation (lo only)"]
    end

    subgraph "Zero-Trust Egress Engine"
        EGR["🛑 Default-Deny Egress Firewall"]
        META["🚫 Cloud Metadata Defense (Blocks 169.254.169.254)"]
        SEC["🔑 In-Flight Secret Substitution (krun-secret:KEY)"]
        METER["📊 Hard LLM Token Ceiling & Metering"]
    end

    GK -->|"tsi_hijack"| TSI
    GK -->|"virtio-net"| GVP
    GK -->|"virtio-net"| CNI_NET
    GK -->|"virtio-net"| TAP_NET
    GK -->|"air-gapped"| AIR

    TSI --> EGR
    GVP --> EGR
    CNI_NET --> EGR
    EGR --> META
    META --> SEC
    SEC --> METER
    METER -->|"Outbound Requests"| WAN["Internet / Upstream APIs"]
```

### 1. Network Driver Modes Comparison

| Mode | Backend | Isolation Level | Root Required? | Virtual NIC (`eth0`) | IP / Subnet Assignment | Latency / Overhead | Recommended Workload |
| :--- | :--- | :--- | :---: | :---: | :---: | :---: | :--- |
| **`tsi`** *(default)* | AF_VSOCK In-Process | Socket Impersonation | ❌ No | Loopback only | Host-Shared (`127.0.0.1`) | Near 0ms / Zero-Copy | Developer containers, CLI tools, high-IOPS web services |
| **`gvproxy`** | Virtio-Net + gVisor | User-Space L2 Virtual Switch | ❌ No | Real `eth0` | Virtual DHCP (`192.168.127.2/24`) | < 1ms / Micro-virtualized | Rootless desktop VMs, VPNs, custom routing, raw packet sockets |
| **`cni`** | Virtio-Net + Netns | Kubernetes Pod Namespace | ❌ No (via CNI) | Pod Veth / Passt | CNI IPAM Subnet | Line-rate bare metal | Kubernetes Pods, containerd CRI, Kind/K3s/EKS-A clusters |
| **`unix`** | Virtio-Net | External User-space Switch | ❌ No | Real `eth0` | Switch Assigned | Minimal | Custom SDN proxies, vfkit on macOS, QEMU bridge |
| **`none`** | No interfaces | Physical Isolation Boundary | ❌ No | Loopback only | None (`127.0.0.1` only) | Zero Network | High-security untrusted code execution, air-gapped sandboxes |

---

### 2. First-Class CLI Network Management (`microvm network`)

Manage, inspect, and diagnose microVM networks with dedicated native commands:

```bash
# List all active microVM networks, modes, IPs, and port mappings:
microvm network ls

# Detailed deep-dive into a microVM's network stack & security rules:
microvm network inspect <vm-id>

# Tabulate active published host-to-guest ports with direct local endpoints:
microvm network ports

# Run live in-guest network connectivity, DNS resolution, and egress diagnostic probes:
microvm network test <vm-id> api.openai.com
```

#### Example Output: `microvm network inspect`
```text
🌐 MicroVM Network Topology & Security: vm-49fa81
-----------------------------------------------------------------
  Status:              ● Running (PID 28419)
  Network Mode:        gvproxy (rootless virtio-net)
  Guest IP Address:    192.168.127.2
  Virtual Gateway:     192.168.127.1
  Virtual MAC:         5a:94:ef:e4:0c:ee
  Interface MTU:       1500
  Guest Hostname:      ai-worker
  DNS Resolvers:       8.8.8.8, 1.1.1.1
  Port Mappings:       0.0.0.0:8080 -> 192.168.127.2:80
  Allowed Egress:      api.openai.com:443, *.github.com:443
  Metadata Defense:    Active (169.254.169.254 exfiltration blocked)
  In-Flight Secrets:   Active (Zero-Trust header/body substitution)
  LLM Token Ceiling:   50000 tokens
  Egress Proxy Server: 127.0.0.1:41823
  Virtual Switch Sock: /Users/apple/.cache/krun-microvm/instances/vm-49fa81/gvproxy.sock
```

---

### 3. Zero-Trust Egress Filtering & In-Flight Secret Masking

Secure autonomous AI agents, multi-tenant workflows, and untrusted code from exfiltrating credentials or cloud infrastructure keys:

```bash
# Run with strict default-deny egress (only OpenAI and GitHub permitted):
microvm run \
    --allow-host api.openai.com:443 \
    --allow-host "*.github.com:443" \
    --secret OPENAI_API_KEY=env:HOST_KEY \
    --max-tokens 50000 \
    python:3.11-slim
```

1. **Default-Deny Egress**: All outbound connections outside the `--allow-host` whitelist are immediately dropped.
2. **Cloud Metadata Defense**: Access to AWS/GCP/Azure instance metadata (`169.254.169.254`) is strictly blocked by default.
3. **In-Flight Secret Substitution**: The microVM only receives an opaque placeholder token (`krun-secret:OPENAI_API_KEY`). The host egress proxy transparently replaces it on the wire with the real secret, so untrusted code can **never read the actual API key from disk or memory**.
4. **Hard LLM Token Budget**: Outbound SSE streams and JSON payloads are inspected in real time. Connections are terminated the instant the cumulative token threshold is exceeded.

---

### 4. Multi-MicroVM Compose Networking & Service Discovery

Services deployed with `microvm compose` automatically receive inter-service DNS discovery:

```yaml
# krun-compose.yaml
version: "krun/v1"
services:
  web:
    image: nginx:alpine
    ports: ["8080:80"]
    depends_on: ["api"]

  api:
    image: python:3.11-alpine
    ports: ["5000:5000"]
    depends_on: ["db"]

  db:
    image: postgres:16-alpine
    ports: ["5432:5432"]
```

- **Seamless Name Resolution**: Inside `web`, requests to `http://api:5000` resolve automatically to the API service. Inside `api`, `postgres://db:5432` resolves seamlessly to the database service.
- **Autonomous Resilient DNS Engine**: The microVM engine parses host nameservers, automatically filters broken systemd-resolved loopback stubs (`127.0.0.53` and `127.0.0.1`), and configures resilient fallback resolvers (`8.8.8.8`, `1.1.1.1`).
- **Custom Hardware Attributes**: Set custom MAC addresses and MTUs with `--mac 5a:94:ef:e4:0c:ee` and `--mtu 9000`.

---

## 🍎 Asahi Linux & m1n1 Boot Integration (Apple Silicon)

`libkrun-sdk` provides native support for booting **Asahi Linux kernels** (`vmlinuz-asahi`, `Image.gz`) and **m1n1 payloads** (`m1n1.bin`, `m1n1.elf`) on Apple Silicon (M1/M2/M3/M4).

```
               +-------------------------------------------------------+
               |        Apple Silicon Host (macOS / Asahi Linux)       |
               |             16KB Memory Page Size Host                |
               +-------------------------------------------------------+
                                          |
                +-------------------------+-------------------------+
                |                                                   |
       [ m1n1 Bootloader ]                                  [ muvm / libkrun ]
   Stage 1/2 Hypervisor & Hardware                      Hardware MicroVM Engine
   Payload (--kernel-format raw)                        Sub-100ms cold boot
                |                                                   |
                +-------------------------+-------------------------+
                                          |
               +-------------------------------------------------------+
               |                libkrun Hardware MicroVM               |
               |              4KB Guest Memory Page Size               |
               +-------------------------------------------------------+
               |  • Asahi Linux Kernel: vmlinuz-asahi / Image.gz       |
               |  • 3D Acceleration: virtio-gpu (Metal / DRM Venus)    |
               |  • High-Throughput I/O: VirtioFS DAX Direct Mapping   |
               |  • x86 Gaming & Emulators: FEX-Emu / Box64 / Wine     |
               +-------------------------------------------------------+
```

### 1. Understanding m1n1 & Asahi Linux Payloads
- **m1n1**: Developed by the Asahi Linux team, `m1n1` serves as the stage 1 and stage 2 bootloader and hypervisor on Apple Silicon. In `libkrun-sdk`, raw `m1n1.bin` payloads can be booted directly with `--kernel-format raw` (`KRUN_KERNEL_FORMAT_RAW`), making `libkrun-sdk` an ideal testbed for low-level Apple Silicon kernel development, hypervisor experimentation, and hardware tracing without risking host instability.
- **Asahi Linux Compressed Kernels (`Image.gz` / `vmlinuz-asahi`)**: Standard Asahi Linux distribution kernels are gzip-compressed ARM64 image binaries (`0x1f, 0x8b`). `libkrun-sdk` automatically inspects the magic bytes and sets `KRUN_KERNEL_FORMAT_IMAGE_GZ`, or allows explicit control via `--kernel-format gz`.

### 2. The 16KB Host vs. 4KB Guest Page Size Problem (Why `muvm` uses `libkrun`)
Apple Silicon hardware operates at **16KB memory page sizes** under both macOS and Asahi Linux to maximize memory bandwidth and TLB hit rates. However:
- The entire x86/x86_64 software ecosystem, including Windows applications, Steam games, and user-space binaries, is hardcoded to **4KB page sizes**.
- Running x86 dynamic translators like **FEX-Emu**, **Box64**, and **Wine / Proton** directly on a 16KB host causes memory corruption, misaligned memory-mapped files, and frequent application crashes.
- **The Solution**: The Asahi Linux project created **`muvm`**, which leverages **`libkrun`** to spin up lightweight Linux microVMs running a **4KB guest kernel** on top of the 16KB Apple Silicon host.
- `libkrun-sdk` brings this exact architecture to developers and engineers with zero-config OCI containers, direct kernel loading, and virtio-gpu passthrough.

### 3. Direct Boot CLI Examples

```bash
# 🍏 1. Boot compressed Asahi Linux ARM64 kernel with rootfs disk and virtio-gpu:
microvm run \
    --kernel /boot/vmlinuz-asahi \
    --kernel-format gz \
    --initrd /boot/initramfs-linux.img \
    --disk rootfs.raw \
    --cmdline "console=ttyAMA0 earlycon root=/dev/vda rw" \
    --gpu --gpu-shm-size 4G \
    -c 4 -m 4096

# 🍎 2. Boot m1n1 stage 1/2 raw payload directly:
microvm run \
    --kernel /usr/lib/asahi-boot/m1n1.bin \
    --kernel-format raw \
    --cmdline "console=ttyAMA0 earlycon" \
    -c 4 -m 2048

# 🚀 3. Run 4KB-page emulation container with declarative manifest:
microvm run -f examples/asahi-microvm.yaml -d
```

### 4. Supported Kernel Formats

| Format Flag | Identifier | Description & Common Targets |
|---|---|---|
| `--kernel-format raw` | `KRUN_KERNEL_FORMAT_RAW` | Flat binary payload, ARM64 uncompressed `Image`, `m1n1.bin` |
| `--kernel-format gz` | `KRUN_KERNEL_FORMAT_IMAGE_GZ` | Gzip-compressed ARM64 Linux kernel (`vmlinuz-asahi`, `Image.gz`) |
| `--kernel-format elf` | `KRUN_KERNEL_FORMAT_ELF` | Uncompressed ELF Linux kernel (`vmlinux`), unikernels |
| `--kernel-format zstd`| `KRUN_KERNEL_FORMAT_IMAGE_ZSTD` | Zstandard-compressed kernel image |
| `--kernel-format bz2` | `KRUN_KERNEL_FORMAT_IMAGE_BZ2` | Bzip2-compressed kernel image |
| `--kernel-format pe`  | `KRUN_KERNEL_FORMAT_PE_GZ` | Compressed EFI PE kernel binary |
| *(Omitted)* | Auto-Detect | Automatic inspection of file magic bytes (`0x1f8b`, `0x28b52ffd`, `\x7fELF`) |

---

## 🛠️ CLI Command Reference

<details>
<summary><b>Click to expand full CLI command catalog</b></summary>

### Lifecycle & Execution
```bash
# Run interactive container shell:
microvm run -it alpine:latest -- sh

# Run detached in background (prints container ID):
microvm run -d alpine:latest -- sleep 300

# Mount host directories via VirtioFS:
microvm run -v /Users/apple/data:data alpine:latest -- ls -la /data

# Forward ports (host:guest):
microvm run -p 8080:80 nginx:alpine

# Stream microVM console logs:
microvm logs -f <vm-id>

# Copy files between host and microVM:
microvm cp ./app.py <vm-id>:/root/app.py
microvm cp <vm-id>:/root/output.log ./output.log

# Execute command inside a running microVM:
microvm exec <vm-id> -- cat /etc/os-release
```

### State, Monitoring & Telemetry
```bash
# List all microVMs:
microvm ps -a

# Live interactive CPU, memory, and thread stats dashboard:
microvm stats

# Single-shot telemetry JSON for monitoring tools:
microvm stats --no-stream --json

# Pause and resume execution:
microvm pause <vm-id>
microvm resume <vm-id>

# Dynamically resize CPU and memory of a running microVM without reboot:
microvm resize <vm-id> --cpus 4 --memory 2048

# Prometheus metrics scrape endpoint:
microvm metrics --listen 0.0.0.0:9090

# Stop and remove:
microvm stop <vm-id>
microvm rm -f <vm-id>
microvm prune
```

### Universal Multi-Boot Engines (Linux, Asahi, m1n1, UEFI & Unikernels)
```bash
# Direct compressed Asahi Linux ARM64 kernel boot (auto-detect or explicit format):
microvm run --kernel /boot/vmlinuz-asahi --kernel-format gz --initrd /boot/initrd.img --cmdline "console=ttyAMA0 earlycon" --disk rootfs.raw

# Direct m1n1 stage 1/2 raw payload boot on Apple Silicon:
microvm run --kernel /usr/lib/asahi-boot/m1n1.bin --kernel-format raw --cmdline "console=ttyAMA0 earlycon" -c 4 -m 2048

# Direct standard Linux kernel boot with initrd and cmdline:
microvm run --kernel /boot/vmlinuz --initrd /boot/initrd.img --cmdline "console=ttyS0" --disk rootfs.raw

# UEFI firmware boot (EDK2 / KRUN_EFI.fd):
microvm run --firmware /usr/share/edk2/aarch64/QEMU_EFI.fd --disk os.img

# Boot unikernels (Unikraft, Nanos, OSv):
microvm unikernel app.unikraft -c 2 -m 512 --cmdline "netdev.ipv4_addr=192.168.1.2"
```

### Virtual Networking & Egress Security
```bash
# List all microVM virtual networks, driver modes, IPs, and port forwards:
microvm network ls
microvm network ls --json

# Deep inspect network configuration, MAC, DNS, and firewall policies:
microvm network inspect <vm-id>

# View published port forwards across all running microVMs:
microvm network ports
microvm network ports <vm-id>

# Run live connectivity, DNS resolution, and egress diagnostic probes:
microvm network test <vm-id> api.openai.com
microvm network test <vm-id> 1.1.1.1

# Launch microVM with custom network mode, MAC address, and jumbo frames:
microvm run --net gvproxy --mac 5a:94:ef:e4:0c:ee --mtu 9000 alpine:latest
```

</details>

---

## 💻 Multi-Language Client SDKs

`libkrun-sdk` provides first-class client SDKs across **Python**, **TypeScript/Node**, **Rust**, and **Go**:

<details>
<summary><b>🐍 Python SDK (Serverless <code>@task</code> decorator)</b></summary>

```python
from libkrun_microvm import task, MicroVmBudgetExceededError

@task(
    image="python:3.11-slim",
    cpus=2,
    memory_mb=512,
    allow_hosts=["api.openai.com:443"],
    secrets={"OPENAI_API_KEY": "env:OPENAI_API_KEY"},
    max_tokens=25000,
)
def run_autonomous_agent(prompt: str) -> dict:
    # Executed inside a hardware-isolated microVM:
    return {"status": "success", "prompt": prompt}

result = run_autonomous_agent("Audit security policy")
print(result)
```
</details>

<details>
<summary><b>🌐 TypeScript / Node.js SDK (<code>@libkrun/sdk</code>)</b></summary>

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
</details>

<details>
<summary><b>🦀 Rust SDK (<code>microvm-core</code>)</b></summary>

```rust
use anyhow::Result;
use microvm_core::MicroVmBuilder;

#[tokio::main]
async fn main() -> Result<()> {
    let mut vm = MicroVmBuilder::new("alpine:latest")
        .cpus(2)
        .memory_mb(512)
        .port_forward(8080, 80)
        .cmd(vec!["echo".into(), "Hello from Rust MicroVM!".into()])
        .run()
        .await?;

    let status = vm.wait().await?;
    println!("MicroVM exited: {status}");
    Ok(())
}
```
</details>

<details>
<summary><b>🔷 Go SDK (<code>krun-sdk-go</code>)</b></summary>

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
        Image:    "alpine:latest",
        Cpus:     2,
        MemoryMb: 512,
        Cmd:      []string{"echo", "Hello from Go SDK MicroVM!"},
    })
    if err != nil {
        log.Fatal(err)
    }

    _ = vm.Start(ctx)
    exitCode, _ := vm.Wait(ctx)
    fmt.Printf("MicroVM finished: %d\n", exitCode)
}
```
</details>

---

## ☸️ Kubernetes & containerd Integration

`libkrun-sdk` includes an official containerd v2 runtime shim (`containerd-shim-krun-v2`) and a pure-Rust `kube-rs` Operator:

```bash
# 1. Install containerd runtime shim:
microvm containerd install

# 2. Register RuntimeClass in Kubernetes:
kubectl apply -f k8s/runtimeclass.yaml

# 3. Launch Pods with hardware virtualization:
kubectl apply -f - <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: isolated-workload
spec:
  runtimeClassName: krun
  containers:
    - name: app
      image: alpine:latest
      command: ["sh", "-c", "echo Hardware-isolated Pod! && sleep 3600"]
EOF
```

---

## 🏗️ Building from Source

```bash
# Clone with submodules:
git clone --recurse-submodules https://github.com/sangam14/libkrun-sdk.git
cd libkrun-sdk

# Build all workspace binaries:
make build

# Sign binaries with macOS Hypervisor entitlement:
make sign

# Run all 105 automated unit and integration tests:
make test
```

---

## 🤝 Community & Contributing

Contributions are warmly welcome! Whether you are fixing bugs, optimizing boot latency, or enhancing SDKs:
1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Run tests (`make test`)
4. Open a Pull Request

---

## 📄 License

This project is licensed under the **Apache License 2.0**. See the [LICENSE](LICENSE) file for details.

<div align="center">

**If you find `krun-microvm` useful, please give us a ⭐ on GitHub! It helps the project grow.**

</div>
