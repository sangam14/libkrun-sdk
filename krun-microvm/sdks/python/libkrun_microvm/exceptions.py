"""Exceptions for libkrun_microvm Python SDK."""

class MicroVmError(Exception):
    """Base exception for all microVM errors."""
    pass

class MicroVmBinaryNotFoundError(MicroVmError):
    """Raised when the microvm CLI binary cannot be located."""
    pass

class MicroVmExecutionError(MicroVmError):
    """Raised when a task executed inside the microVM fails or returns non-zero exit code."""
    def __init__(self, message: str, exit_code: int = 1, stderr: str = "", traceback_str: str = ""):
        super().__init__(message)
        self.exit_code = exit_code
        self.stderr = stderr
        self.traceback_str = traceback_str

class MicroVmTimeoutError(MicroVmError):
    """Raised when a microVM task times out."""
    pass

class MicroVmBudgetExceededError(MicroVmError):
    """Raised when LLM token budget is exceeded during execution."""
    pass
