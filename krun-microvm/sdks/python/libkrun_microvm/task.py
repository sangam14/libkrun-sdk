"""Task decorator and execution harness for libkrun_microvm."""

import base64
import functools
import inspect
import json
import pickle
import sys
from dataclasses import dataclass, field
from typing import (
    Any,
    Callable,
    Dict,
    Generic,
    List,
    Optional,
    TypeVar,
    Union,
)

from .client import build_run_args, execute_microvm_command, find_microvm_binary
from .exceptions import (
    MicroVmBudgetExceededError,
    MicroVmError,
    MicroVmExecutionError,
)

R = TypeVar("R")

RES_MARKER_START = "__KRUN_SDK_RES__"
RES_MARKER_END = "__KRUN_SDK_RES__"

GUEST_HARNESS_TEMPLATE = """
import sys, base64, pickle, traceback

def main():
    payload_b64 = "{payload_b64}"
    raw = base64.b64decode(payload_b64)
    data = pickle.loads(raw)
    func = data["func"]
    args = data.get("args", ())
    kwargs = data.get("kwargs", {{}})

    try:
        res = func(*args, **kwargs)
        payload_out = pickle.dumps({{"status": "ok", "result": res}})
        encoded = base64.b64encode(payload_out).decode("utf-8")
        print("{start}" + encoded + "{end}")
    except Exception as exc:
        payload_out = pickle.dumps({{
            "status": "error",
            "error": str(exc),
            "error_type": type(exc).__name__,
            "traceback": traceback.format_exc(),
        }})
        encoded = base64.b64encode(payload_out).decode("utf-8")
        print("{start}" + encoded + "{end}")
        sys.exit(1)

if __name__ == "__main__":
    main()
"""

@dataclass
class TaskConfig:
    """Configuration for microVM task execution."""
    image: str = "python:3.11-slim"
    cpus: int = 2
    memory_mb: int = 512
    allow_hosts: List[str] = field(default_factory=list)
    secrets: Dict[str, str] = field(default_factory=dict)
    max_tokens: Optional[int] = None
    volumes: List[str] = field(default_factory=list)
    env: Dict[str, str] = field(default_factory=dict)
    workdir: Optional[str] = None
    timeout: Optional[float] = None
    no_sandbox: bool = False
    binary_path: Optional[str] = None

class MicroVmTask(Generic[R]):
    """Callable wrapper that runs a Python function inside an ephemeral microVM."""

    def __init__(self, func: Callable[..., R], config: TaskConfig):
        self.func = func
        self.config = config
        functools.update_wrapper(self, func)

    def with_options(
        self,
        image: Optional[str] = None,
        cpus: Optional[int] = None,
        memory_mb: Optional[int] = None,
        allow_hosts: Optional[List[str]] = None,
        secrets: Optional[Dict[str, str]] = None,
        max_tokens: Optional[int] = None,
        volumes: Optional[List[str]] = None,
        env: Optional[Dict[str, str]] = None,
        workdir: Optional[str] = None,
        timeout: Optional[float] = None,
        no_sandbox: Optional[bool] = None,
    ) -> "MicroVmTask[R]":
        """Return a new task instance with overridden configuration."""
        new_config = TaskConfig(
            image=image if image is not None else self.config.image,
            cpus=cpus if cpus is not None else self.config.cpus,
            memory_mb=memory_mb if memory_mb is not None else self.config.memory_mb,
            allow_hosts=allow_hosts if allow_hosts is not None else list(self.config.allow_hosts),
            secrets=secrets if secrets is not None else dict(self.config.secrets),
            max_tokens=max_tokens if max_tokens is not None else self.config.max_tokens,
            volumes=volumes if volumes is not None else list(self.config.volumes),
            env=env if env is not None else dict(self.config.env),
            workdir=workdir if workdir is not None else self.config.workdir,
            timeout=timeout if timeout is not None else self.config.timeout,
            no_sandbox=no_sandbox if no_sandbox is not None else self.config.no_sandbox,
            binary_path=self.config.binary_path,
        )
        return MicroVmTask(self.func, new_config)

    def __call__(self, *args: Any, **kwargs: Any) -> R:
        """Synchronously execute the task inside the microVM and return the result."""
        return self._execute(args, kwargs)

    def remote(self, *args: Any, **kwargs: Any) -> R:
        """Alias for executing task inside the microVM."""
        return self._execute(args, kwargs)

    def _execute(self, args: Any, kwargs: Any) -> R:
        # Package function and arguments
        payload = {
            "func": self.func,
            "args": args,
            "kwargs": kwargs,
        }
        try:
            pickled = pickle.dumps(payload)
        except Exception as e:
            raise MicroVmExecutionError(
                f"Failed to serialize function '{self.func.__name__}' or arguments: {e}"
            ) from e

        payload_b64 = base64.b64encode(pickled).decode("utf-8")

        harness_script = GUEST_HARNESS_TEMPLATE.format(
            payload_b64=payload_b64,
            start=RES_MARKER_START,
            end=RES_MARKER_END,
        )

        guest_cmd = ["python3", "-c", harness_script]

        run_args = build_run_args(
            image=self.config.image,
            cmd=guest_cmd,
            cpus=self.config.cpus,
            memory_mb=self.config.memory_mb,
            allow_hosts=self.config.allow_hosts,
            secrets=self.config.secrets,
            max_tokens=self.config.max_tokens,
            volumes=self.config.volumes,
            env=self.config.env,
            workdir=self.config.workdir,
            no_sandbox=self.config.no_sandbox,
        )

        proc = execute_microvm_command(
            args=run_args,
            binary_path=self.config.binary_path,
            timeout=self.config.timeout,
            capture_output=True,
        )

        return self._parse_output(proc.stdout, proc.stderr, proc.returncode)

    def _parse_output(self, stdout: str, stderr: str, returncode: int) -> R:
        if RES_MARKER_START in stdout:
            parts = stdout.split(RES_MARKER_START)
            content = parts[1].split(RES_MARKER_END)[0].strip()
            try:
                decoded = base64.b64decode(content)
                result_data = pickle.loads(decoded)
            except Exception as e:
                raise MicroVmExecutionError(
                    f"Failed to deserialize task result from microVM: {e}\nStdout: {stdout}\nStderr: {stderr}",
                    exit_code=returncode,
                    stderr=stderr,
                )

            if result_data.get("status") == "ok":
                return result_data["result"]
            else:
                err_msg = result_data.get("error", "Unknown error inside microVM task")
                err_type = result_data.get("error_type", "Exception")
                tb = result_data.get("traceback", "")
                raise MicroVmExecutionError(
                    f"[{err_type}] {err_msg}\n{tb}",
                    exit_code=returncode,
                    stderr=stderr,
                    traceback_str=tb,
                )

        # Check for token budget breach or connection denial in stderr/stdout
        if "429 Too Many Requests" in stderr or "LLM Token Budget Exceeded" in stderr or "LLM Token Budget Exceeded" in stdout:
            raise MicroVmBudgetExceededError(f"Task exceeded LLM token budget: {stderr.strip()}")

        if returncode != 0:
            raise MicroVmExecutionError(
                f"MicroVM exited with error code {returncode}.\nStderr:\n{stderr}\nStdout:\n{stdout}",
                exit_code=returncode,
                stderr=stderr,
            )

        raise MicroVmExecutionError(
            f"MicroVM finished without returning task payload markers.\nStdout:\n{stdout}\nStderr:\n{stderr}",
            exit_code=returncode,
            stderr=stderr,
        )

def task(
    image: str = "python:3.11-slim",
    cpus: int = 2,
    memory_mb: int = 512,
    allow_hosts: Optional[List[str]] = None,
    secrets: Optional[Dict[str, str]] = None,
    max_tokens: Optional[int] = None,
    volumes: Optional[List[str]] = None,
    env: Optional[Dict[str, str]] = None,
    workdir: Optional[str] = None,
    timeout: Optional[float] = None,
    no_sandbox: bool = False,
    binary_path: Optional[str] = None,
) -> Callable[[Callable[..., R]], MicroVmTask[R]]:
    """Decorator to declare a Python function as an isolated, hardware-virtualized MicroVM task."""
    config = TaskConfig(
        image=image,
        cpus=cpus,
        memory_mb=memory_mb,
        allow_hosts=allow_hosts or [],
        secrets=secrets or {},
        max_tokens=max_tokens,
        volumes=volumes or [],
        env=env or {},
        workdir=workdir,
        timeout=timeout,
        no_sandbox=no_sandbox,
        binary_path=binary_path,
    )

    def decorator(fn: Callable[..., R]) -> MicroVmTask[R]:
        return MicroVmTask(fn, config)

    return decorator

def run(
    func: Callable[..., R],
    *args: Any,
    image: str = "python:3.11-slim",
    cpus: int = 2,
    memory_mb: int = 512,
    allow_hosts: Optional[List[str]] = None,
    secrets: Optional[Dict[str, str]] = None,
    max_tokens: Optional[int] = None,
    volumes: Optional[List[str]] = None,
    env: Optional[Dict[str, str]] = None,
    workdir: Optional[str] = None,
    timeout: Optional[float] = None,
    no_sandbox: bool = False,
    binary_path: Optional[str] = None,
    **kwargs: Any,
) -> R:
    """Convenience function to execute a callable directly in an ephemeral microVM."""
    t = task(
        image=image,
        cpus=cpus,
        memory_mb=memory_mb,
        allow_hosts=allow_hosts,
        secrets=secrets,
        max_tokens=max_tokens,
        volumes=volumes,
        env=env,
        workdir=workdir,
        timeout=timeout,
        no_sandbox=no_sandbox,
        binary_path=binary_path,
    )(func)
    return t(*args, **kwargs)
