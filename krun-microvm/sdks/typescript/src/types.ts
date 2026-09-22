export interface PortForward {
  host: number;
  guest: number;
}

export interface VolumeMount {
  hostPath: string;
  tag: string;
  readOnly?: boolean;
}

export interface RunOptions {
  image?: string;
  cpus?: number;
  memoryMb?: number;
  ports?: PortForward[];
  volumes?: VolumeMount[];
  env?: Record<string, string>;
  workdir?: string;
  cmd?: string[];
  detach?: boolean;
  interactive?: boolean;
  tty?: boolean;
  noNetwork?: boolean;
  kernel?: string;
  initrd?: string;
  cmdline?: string;
  firmware?: string;
  disks?: string[];
  allowHosts?: string[];
  secrets?: Record<string, string>;
  maxTokens?: number;
}

export interface ExecOptions {
  cmd: string[];
  env?: Record<string, string>;
  workdir?: string;
  tty?: boolean;
}

export interface ExecResult {
  exitCode: number;
  stdout: string;
  stderr: string;
}

export interface VmInfo {
  id: string;
  image: string;
  pid: number;
  status: "Running" | "Paused" | "Stopped";
  portForwards: PortForward[];
  createdAt: number;
}
