class MurmurError(RuntimeError):
    """Base class for actionable Murmur runtime failures."""

    def __init__(self, message: str, hint: str | None = None):
        super().__init__(message)
        self.message = message
        self.hint = hint

    def doctor(self) -> str:
        if self.hint:
            return f"{self.message}\nHint: {self.hint}"
        return self.message
