import { test } from "node:test";
import assert from "node:assert/strict";
import { MicroVm, buildRunArgs, buildExecArgs } from "./index.js";

test("buildRunArgs generates minimal run arguments", () => {
  const args = buildRunArgs({
    image: "alpine:latest",
    cpus: 2,
    memoryMb: 1024,
  });

  assert.deepEqual(args, [
    "run",
    "-d",
    "-c",
    "2",
    "-m",
    "1024",
    "alpine:latest",
  ]);
});

test("buildRunArgs generates comprehensive arguments with network, mounts, and secrets", () => {
  const args = buildRunArgs({
    image: "alpine:latest",
    cpus: 4,
    memoryMb: 2048,
    workdir: "/app",
    noNetwork: true,
    ports: [
      { host: 8080, guest: 80 },
      { host: 9000, guest: 9000 },
    ],
    volumes: [
      { hostPath: "/host/data", tag: "data", readOnly: false },
      { hostPath: "/host/ro", tag: "ro_data", readOnly: true },
    ],
    env: {
      NODE_ENV: "production",
      PORT: "80",
    },
    allowHosts: ["api.anthropic.com:443", "github.com:443"],
    secrets: {
      API_KEY: "secret123",
    },
    maxTokens: 50000,
    cmd: ["node", "server.js"],
  });

  assert.ok(args.includes("-c") && args[args.indexOf("-c") + 1] === "4");
  assert.ok(args.includes("-m") && args[args.indexOf("-m") + 1] === "2048");
  assert.ok(args.includes("-w") && args[args.indexOf("-w") + 1] === "/app");
  assert.ok(args.includes("--no-network"));
  assert.ok(args.includes("-p") && args.includes("8080:80"));
  assert.ok(args.includes("-p") && args.includes("9000:9000"));
  assert.ok(args.includes("-v") && args.includes("/host/data:data"));
  assert.ok(args.includes("-v") && args.includes("/host/ro:ro_data:ro"));
  assert.ok(args.includes("-e") && args.includes("NODE_ENV=production"));
  assert.ok(args.includes("--allow-host") && args.includes("api.anthropic.com:443"));
  assert.ok(args.includes("--secret") && args.includes("API_KEY=secret123"));
  assert.ok(args.includes("--max-tokens") && args.includes("50000"));
  assert.ok(args.includes("alpine:latest"));
  assert.ok(args.includes("--"));
  assert.ok(args.includes("server.js"));
});

test("buildRunArgs handles multi-boot unikernel and kernel configs", () => {
  const args = buildRunArgs({
    kernel: "/boot/vmlinuz",
    initrd: "/boot/initrd.img",
    cmdline: "console=ttyS0 quiet",
    firmware: "/boot/OVMF.fd",
    disks: ["/dev/vda:/path/disk.raw", "/dev/vdb:/path/ro.raw:ro"],
  });

  assert.ok(args.includes("--kernel") && args[args.indexOf("--kernel") + 1] === "/boot/vmlinuz");
  assert.ok(args.includes("--initrd") && args[args.indexOf("--initrd") + 1] === "/boot/initrd.img");
  assert.ok(args.includes("--cmdline") && args[args.indexOf("--cmdline") + 1] === "console=ttyS0 quiet");
  assert.ok(args.includes("--firmware") && args[args.indexOf("--firmware") + 1] === "/boot/OVMF.fd");
  assert.ok(args.includes("--disk") && args.includes("/dev/vda:/path/disk.raw"));
});

test("buildExecArgs generates correct exec command arguments", () => {
  const args = buildExecArgs("vm-test-123", {
    cmd: ["sh", "-c", "echo hello"],
    tty: true,
    workdir: "/workspace",
    env: { FOO: "bar" },
  });

  assert.deepEqual(args, [
    "exec",
    "-t",
    "-w",
    "/workspace",
    "-e",
    "FOO=bar",
    "vm-test-123",
    "--",
    "sh",
    "-c",
    "echo hello",
  ]);
});

test("MicroVm instance initializes with id", () => {
  const vm = new MicroVm("microvm-abcde");
  assert.equal(vm.id, "microvm-abcde");
});
