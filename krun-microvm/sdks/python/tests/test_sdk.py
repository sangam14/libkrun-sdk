"""Unit tests for libkrun_microvm Python SDK."""

import base64
import os
import pickle
import unittest
from unittest.mock import patch, MagicMock

from libkrun_microvm.client import build_run_args, find_microvm_binary
from libkrun_microvm.exceptions import (
    MicroVmBinaryNotFoundError,
    MicroVmBudgetExceededError,
    MicroVmExecutionError,
)
from libkrun_microvm.task import (
    MicroVmTask,
    TaskConfig,
    task,
    run,
    RES_MARKER_START,
    RES_MARKER_END,
)

class TestClientAndArgs(unittest.TestCase):
    def test_build_run_args_complete(self):
        args = build_run_args(
            image="python:3.11-slim",
            cmd=["python3", "app.py"],
            cpus=4,
            memory_mb=1024,
            allow_hosts=["api.openai.com:443", "*.anthropic.com:443"],
            secrets={"OPENAI_API_KEY": "sk-12345", "HF_TOKEN": "env:HF_TOKEN"},
            max_tokens=25000,
            volumes=["/host/data:/data:ro"],
            env={"DEBUG": "1"},
            workdir="/app",
            no_sandbox=True,
        )

        self.assertIn("run", args)
        self.assertIn("-c", args)
        self.assertEqual(args[args.index("-c") + 1], "4")
        self.assertIn("-m", args)
        self.assertEqual(args[args.index("-m") + 1], "1024")
        self.assertIn("--no-sandbox", args)

        # Check allow-hosts
        self.assertIn("--allow-host", args)
        self.assertIn("api.openai.com:443", args)
        self.assertIn("*.anthropic.com:443", args)

        # Check secrets
        self.assertIn("--secret", args)
        self.assertIn("OPENAI_API_KEY=sk-12345", args)
        self.assertIn("HF_TOKEN=env:HF_TOKEN", args)

        # Check max-tokens
        self.assertIn("--max-tokens", args)
        self.assertEqual(args[args.index("--max-tokens") + 1], "25000")

        # Check volumes, env, workdir
        self.assertIn("-v", args)
        self.assertEqual(args[args.index("-v") + 1], "/host/data:/data:ro")
        self.assertIn("-e", args)
        self.assertEqual(args[args.index("-e") + 1], "DEBUG=1")
        self.assertIn("-w", args)
        self.assertEqual(args[args.index("-w") + 1], "/app")

        # Check image and command
        self.assertIn("python:3.11-slim", args)
        self.assertIn("--", args)
        self.assertEqual(args[args.index("--") + 1:], ["python3", "app.py"])

    def test_find_microvm_binary_env(self):
        with patch.dict(os.environ, {"MICROVM_BIN": "/custom/bin/microvm"}):
            with patch("os.path.isfile", return_value=True), patch("os.access", return_value=True):
                self.assertEqual(find_microvm_binary(), "/custom/bin/microvm")

    def test_find_microvm_binary_not_found(self):
        with patch.dict(os.environ, {}, clear=True):
            with patch("shutil.which", return_value=None), \
                 patch("pathlib.Path.is_file", return_value=False):
                with self.assertRaises(MicroVmBinaryNotFoundError):
                    find_microvm_binary()

class TestTaskDecorator(unittest.TestCase):
    def test_task_metadata_and_options(self):
        @task(
            image="python:3.10-slim",
            cpus=2,
            memory_mb=512,
            allow_hosts=["api.openai.com:443"],
            max_tokens=5000,
        )
        def sample_add(a: int, b: int) -> int:
            """Sample addition task."""
            return a + b

        self.assertEqual(sample_add.__name__, "sample_add")
        self.assertEqual(sample_add.__doc__, "Sample addition task.")
        self.assertEqual(sample_add.config.cpus, 2)
        self.assertEqual(sample_add.config.memory_mb, 512)
        self.assertEqual(sample_add.config.allow_hosts, ["api.openai.com:443"])
        self.assertEqual(sample_add.config.max_tokens, 5000)

        # with_options override
        overridden = sample_add.with_options(cpus=8, memory_mb=2048)
        self.assertEqual(overridden.config.cpus, 8)
        self.assertEqual(overridden.config.memory_mb, 2048)
        self.assertEqual(overridden.config.allow_hosts, ["api.openai.com:443"])

    def test_parse_output_success(self):
        @task()
        def dummy():
            pass

        data = {"status": "ok", "result": {"answer": 42, "items": ["a", "b"]}}
        b64 = base64.b64encode(pickle.dumps(data)).decode("utf-8")
        stdout = f"MicroVM starting...\n{RES_MARKER_START}{b64}{RES_MARKER_END}\nMicroVM shutting down."

        res = dummy._parse_output(stdout, "", 0)
        self.assertEqual(res, {"answer": 42, "items": ["a", "b"]})

    def test_parse_output_remote_exception(self):
        @task()
        def failing_task():
            pass

        data = {
            "status": "error",
            "error": "Division by zero",
            "error_type": "ZeroDivisionError",
            "traceback": "Traceback (most recent call last):\n  ...\nZeroDivisionError: Division by zero",
        }
        b64 = base64.b64encode(pickle.dumps(data)).decode("utf-8")
        stdout = f"{RES_MARKER_START}{b64}{RES_MARKER_END}"

        with self.assertRaises(MicroVmExecutionError) as ctx:
            failing_task._parse_output(stdout, "", 1)
        self.assertIn("ZeroDivisionError", str(ctx.exception))
        self.assertIn("Division by zero", str(ctx.exception))

    def test_parse_output_token_budget_exceeded(self):
        @task()
        def token_hungry_task():
            pass

        stderr = "HTTP/1.1 429 Too Many Requests\r\nContent-Type: text/plain\r\n\r\nLLM Token Budget Exceeded: cumulative 10050 tokens used, exceeding ceiling of 10000"
        with self.assertRaises(MicroVmBudgetExceededError) as ctx:
            token_hungry_task._parse_output("", stderr, 1)
        self.assertIn("exceeded LLM token budget", str(ctx.exception))

if __name__ == "__main__":
    unittest.main()
