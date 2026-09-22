# @libkrun/sdk (TypeScript / Node.js)

Official TypeScript SDK for launching and orchestrating hardware-isolated microVMs using `libkrun` on macOS (Apple Silicon) and Linux (KVM).

## Installation

```bash
npm install @libkrun/sdk
```

## Quick Start

```typescript
import { MicroVm } from "@libkrun/sdk";

async function main() {
  // 1. Launch a microVM
  const vm = await MicroVm.run({
    image: "alpine:latest",
    cpus: 2,
    memoryMb: 512,
    allowHosts: ["api.openai.com:443", "github.com:443"],
  });
  console.log(`Running microVM: ${vm.id}`);

  // 2. Execute commands with true guest isolation
  const res = await vm.exec({
    cmd: ["uname", "-a"],
  });
  console.log(`Guest Kernel: ${res.stdout.trim()}`);

  // 3. Lifecycle control
  await vm.stop();
  await vm.rm();
}

main().catch(console.error);
```
