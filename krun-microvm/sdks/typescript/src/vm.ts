import { spawn, execFile } from "child_process";
import { promisify } from "util";
import { RunOptions, ExecOptions, ExecResult, VmInfo } from "./types.js";

const execFileAsync = promisify(execFile);

function findBinary(): string {
  return process.env.MICROVM_BIN || "microvm";
}

export function buildRunArgs(opts: RunOptions): string[] {
  const args: string[] = ["run", "-d"];

  if (opts.cpus) args.push("-c", opts.cpus.toString());
  if (opts.memoryMb) args.push("-m", opts.memoryMb.toString());
  if (opts.workdir) args.push("-w", opts.workdir);
  if (opts.noNetwork) args.push("--no-network");

  if (opts.ports) {
    for (const p of opts.ports) {
      args.push("-p", `${p.host}:${p.guest}`);
    }
  }

  if (opts.volumes) {
    for (const v of opts.volumes) {
      args.push("-v", `${v.hostPath}:${v.tag}${v.readOnly ? ":ro" : ""}`);
    }
  }

  if (opts.env) {
    for (const [k, val] of Object.entries(opts.env)) {
      args.push("-e", `${k}=${val}`);
    }
  }

  if (opts.kernel) args.push("--kernel", opts.kernel);
  if (opts.initrd) args.push("--initrd", opts.initrd);
  if (opts.cmdline) args.push("--cmdline", opts.cmdline);
  if (opts.firmware) args.push("--firmware", opts.firmware);

  if (opts.disks) {
    for (const d of opts.disks) {
      args.push("--disk", d);
    }
  }

  if (opts.allowHosts) {
    for (const h of opts.allowHosts) {
      args.push("--allow-host", h);
    }
  }

  if (opts.secrets) {
    for (const [k, val] of Object.entries(opts.secrets)) {
      args.push("--secret", `${k}=${val}`);
    }
  }

  if (opts.maxTokens) {
    args.push("--max-tokens", opts.maxTokens.toString());
  }

  if (opts.image) {
    args.push(opts.image);
  }

  if (opts.cmd && opts.cmd.length > 0) {
    args.push("--", ...opts.cmd);
  }

  return args;
}

export function buildExecArgs(id: string, opts: ExecOptions): string[] {
  const args: string[] = ["exec"];

  if (opts.tty) args.push("-t");
  if (opts.workdir) args.push("-w", opts.workdir);
  if (opts.env) {
    for (const [k, v] of Object.entries(opts.env)) {
      args.push("-e", `${k}=${v}`);
    }
  }

  args.push(id, "--", ...opts.cmd);
  return args;
}

export class MicroVm {
  readonly id: string;

  constructor(id: string) {
    this.id = id;
  }

  /**
   * Spawns a new hardware-isolated microVM.
   */
  static async run(opts: RunOptions): Promise<MicroVm> {
    const bin = findBinary();
    const args = buildRunArgs(opts);

    const { stdout } = await execFileAsync(bin, args);
    const id = stdout.trim().split("\n").pop() || "";
    if (!id) {
      throw new Error(`Failed to retrieve microVM ID from runner output: ${stdout}`);
    }

    return new MicroVm(id);
  }

  /**
   * Executes a command inside the running microVM.
   */
  async exec(opts: ExecOptions): Promise<ExecResult> {
    const bin = findBinary();
    const args = buildExecArgs(this.id, opts);

    try {
      const { stdout, stderr } = await execFileAsync(bin, args);
      return {
        exitCode: 0,
        stdout,
        stderr,
      };
    } catch (err: any) {
      return {
        exitCode: err.code || 1,
        stdout: err.stdout || "",
        stderr: err.stderr || err.message,
      };
    }
  }

  /**
   * Pauses all vCPUs of the microVM.
   */
  async pause(): Promise<void> {
    const bin = findBinary();
    await execFileAsync(bin, ["pause", this.id]);
  }

  /**
   * Resumes execution of a paused microVM.
   */
  async resume(): Promise<void> {
    const bin = findBinary();
    await execFileAsync(bin, ["resume", this.id]);
  }

  /**
   * Stops the running microVM.
   */
  async stop(): Promise<void> {
    const bin = findBinary();
    await execFileAsync(bin, ["stop", this.id]);
  }

  /**
   * Removes the stopped microVM instance.
   */
  async rm(force = false): Promise<void> {
    const bin = findBinary();
    const args = ["rm"];
    if (force) args.push("-f");
    args.push(this.id);
    await execFileAsync(bin, args);
  }

  /**
   * Lists active microVMs.
   */
  static async list(all = false): Promise<VmInfo[]> {
    const bin = findBinary();
    const args = ["ps"];
    if (all) args.push("-a");

    const { stdout } = await execFileAsync(bin, args);
    const lines = stdout.trim().split("\n");
    if (lines.length <= 1) return [];

    const vms: VmInfo[] = [];
    for (const line of lines.slice(1)) {
      const parts = line.split(/\s{2,}/).map((s) => s.trim());
      if (parts.length >= 4) {
        vms.push({
          id: parts[0],
          image: parts[1],
          pid: parseInt(parts[2], 10) || 0,
          status: parts[3].includes("Up") ? "Running" : parts[3].includes("Paused") ? "Paused" : "Stopped",
          portForwards: [],
          createdAt: Date.now(),
        });
      }
    }
    return vms;
  }
}
