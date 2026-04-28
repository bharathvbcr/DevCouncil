class DevCouncilError(Exception):
    """Base exception for all DevCouncil errors."""
    pass

class GatingError(DevCouncilError):
    """Raised when a task or plan fails a mandatory gate."""
    pass

class ConfigurationError(DevCouncilError):
    """Raised when the project configuration is invalid or missing."""
    pass

class OrchestrationError(DevCouncilError):
    """Raised when the orchestrator encounters an unexpected state."""
    pass

class ExecutionError(DevCouncilError):
    """Raised when an external or internal executor fails to run a task."""
    pass

class VerificationError(DevCouncilError):
    """Raised when the verifier fails to run its checks properly."""
    pass
