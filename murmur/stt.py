from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Protocol

from .errors import MurmurError
from .config import SttConfig


class SttError(RuntimeError):
    pass


@dataclass(frozen=True)
class Transcription:
    text: str
    provider: str


class SttBackend(Protocol):
    name: str

    def transcribe(self, audio_path: Path, dictionary_terms: Iterable[str] | None = None) -> str: ...


def build_backend(provider: str) -> SttBackend:
    provider = provider.lower()
    if provider == "auto":
        if os.environ.get("MURMUR_WHISPER_CPP") or shutil.which("whisper-cli") or shutil.which("main"):
            provider = "whispercpp"
        else:
            provider = "faster-whisper"
    if provider in ("faster-whisper", "faster_whisper"):
        return FasterWhisperBackend()
    if provider in ("whispercpp", "whisper.cpp", "whisper-cpp"):
        return WhisperCppBackend()
    raise MurmurError(f"Unknown STT provider: {provider}", "Use --provider faster-whisper, --provider whispercpp, or --provider auto.")


class FasterWhisperBackend:
    name = "faster-whisper"

    def __init__(self) -> None:
        self.model_name = os.environ.get("MURMUR_WHISPER_MODEL", "base.en")
        self.device = os.environ.get("MURMUR_WHISPER_DEVICE", "cpu")
        self.compute_type = os.environ.get("MURMUR_WHISPER_COMPUTE_TYPE", "int8")
        self.initial_prompt: str | None = None

    def transcribe(self, audio_path: Path, dictionary_terms: Iterable[str] | None = None) -> str:
        try:
            from faster_whisper import WhisperModel  # type: ignore
        except ImportError as exc:
            raise MurmurError(
                "Missing Python STT dependency: faster-whisper",
                "Install it in your environment with `python -m pip install faster-whisper`, or configure whisper.cpp with MURMUR_WHISPER_CPP and MURMUR_WHISPER_CPP_MODEL.",
            ) from exc

        try:
            model = WhisperModel(self.model_name, device=self.device, compute_type=self.compute_type)
            initial_prompt = self.initial_prompt or _dictionary_prompt(dictionary_terms)
            kwargs = {"beam_size": 1}
            if initial_prompt:
                kwargs["initial_prompt"] = initial_prompt
            segments, _info = model.transcribe(str(audio_path), **kwargs)
            return " ".join(segment.text.strip() for segment in segments).strip()
        except Exception as exc:  # model download/path/runtime errors should be actionable
            raise MurmurError(
                "faster-whisper transcription failed.",
                f"Model={self.model_name!r}, device={self.device!r}, compute_type={self.compute_type!r}. Set MURMUR_WHISPER_MODEL to a downloaded model name/path or try `base.en`. Details: {exc}",
            ) from exc


class WhisperCppBackend:
    name = "whisper.cpp"

    def __init__(self) -> None:
        self.binary = os.environ.get("MURMUR_WHISPER_CPP") or shutil.which("whisper-cli") or shutil.which("main")
        self.model_path = os.environ.get("MURMUR_WHISPER_CPP_MODEL")

    def transcribe(self, audio_path: Path, dictionary_terms: Iterable[str] | None = None) -> str:
        if not self.binary:
            raise MurmurError(
                "Missing whisper.cpp executable.",
                "Install/build whisper.cpp and set MURMUR_WHISPER_CPP=/path/to/whisper-cli, or use --provider faster-whisper.",
            )
        if not self.model_path or not Path(self.model_path).exists():
            raise MurmurError(
                "Missing whisper.cpp model file.",
                "Download a ggml model and set MURMUR_WHISPER_CPP_MODEL=/path/to/ggml-*.bin.",
            )

        with tempfile.TemporaryDirectory(prefix="murmur-whispercpp-") as tmpdir:
            out_base = Path(tmpdir) / "transcript"
            cmd = [self.binary, "-m", self.model_path, "-f", str(audio_path), "-otxt", "-of", str(out_base)]
            prompt = _dictionary_prompt(dictionary_terms)
            if prompt:
                cmd.extend(["--prompt", prompt])
            result = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            if result.returncode != 0:
                raise MurmurError(
                    "whisper.cpp transcription failed.",
                    f"Command exited {result.returncode}: {result.stderr.strip() or result.stdout.strip()}",
                )
            txt_path = out_base.with_suffix(".txt")
            if txt_path.exists():
                return txt_path.read_text(encoding="utf-8").strip()
            return result.stdout.strip()


def transcribe(audio_path: Path, config: SttConfig, vocabulary: str | None = None, dictionary_terms: Iterable[str] | None = None) -> Transcription:
    """Transcribe audio using the configured local backend."""
    try:
        if config.provider in ("faster-whisper", "faster_whisper"):
            backend = FasterWhisperBackend()
            backend.model_name = config.model
            backend.initial_prompt = vocabulary
        elif config.provider in ("whisper-cpp", "whisper.cpp", "whispercpp"):
            backend = WhisperCppBackend()
            backend.binary = shutil.which(config.whisper_cpp_binary) or config.whisper_cpp_binary
            if config.whisper_cpp_model is not None:
                backend.model_path = str(config.whisper_cpp_model)
        else:
            backend = build_backend(config.provider)
        terms = list(dictionary_terms or [])
        return Transcription(text=backend.transcribe(audio_path, dictionary_terms=terms), provider=backend.name)
    except MurmurError as exc:
        raise SttError(str(exc)) from exc


def _dictionary_prompt(dictionary_terms: Iterable[str] | None) -> str:
    terms = [term.strip() for term in (dictionary_terms or []) if term.strip()]
    if not terms:
        return ""
    return "Vocabulary: " + ", ".join(terms[:80])
