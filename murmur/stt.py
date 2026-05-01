from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
import urllib.error
import urllib.request
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
    if provider in ("elevenlabs", "eleven-labs", "scribe"):
        return ElevenLabsBackend()
    if provider == "groq":
        return GroqBackend()
    raise MurmurError(f"Unknown STT provider: {provider}", "Use provider faster-whisper, whispercpp, elevenlabs, groq, or auto.")


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


class GroqBackend:
    name = "groq"

    def __init__(self) -> None:
        self.model_name = os.environ.get("MURMUR_GROQ_STT_MODEL", "whisper-large-v3-turbo")
        self.endpoint = os.environ.get("MURMUR_GROQ_STT_ENDPOINT", "https://api.groq.com/openai/v1/audio/transcriptions")
        self.api_key = _groq_api_key()
        self.timeout_seconds = float(os.environ.get("MURMUR_GROQ_TIMEOUT", "20"))
        self.language = os.environ.get("MURMUR_GROQ_LANGUAGE")

    def transcribe(self, audio_path: Path, dictionary_terms: Iterable[str] | None = None) -> str:
        if not self.api_key:
            raise MurmurError(
                "Missing Groq API key.",
                "Set GROQ_API_KEY or MURMUR_GROQ_API_KEY, or write ~/.config/murmur/groq_api_key.",
            )
        fields = {
            "model": self.model_name,
            "response_format": "json",
            "temperature": "0",
        }
        if self.language:
            fields["language"] = self.language
        prompt = _dictionary_prompt(dictionary_terms)
        if prompt:
            fields["prompt"] = prompt[:900]
        body, content_type = _multipart_form_data(fields, file_field="file", file_path=audio_path)
        req = urllib.request.Request(
            self.endpoint,
            data=body,
            method="POST",
            headers={
                "Authorization": f"Bearer {self.api_key}",
                "Content-Type": content_type,
                "Accept": "application/json",
                "User-Agent": "murmur-dictation/0.1",
            },
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout_seconds) as response:
                payload = json.loads(response.read().decode("utf-8"))
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="replace")
            raise MurmurError("Groq transcription failed.", f"HTTP {exc.code}: {detail}") from exc
        except Exception as exc:
            raise MurmurError("Groq transcription failed.", str(exc)) from exc
        return str(payload.get("text", "")).strip()


class ElevenLabsBackend:
    name = "elevenlabs"

    def __init__(self) -> None:
        self.model_name = os.environ.get("MURMUR_ELEVENLABS_MODEL", "scribe_v2")
        self.endpoint = os.environ.get("MURMUR_ELEVENLABS_ENDPOINT", "https://api.elevenlabs.io/v1/speech-to-text")
        self.api_key = _elevenlabs_api_key()
        self.no_verbatim = os.environ.get("MURMUR_ELEVENLABS_NO_VERBATIM", "1") not in ("0", "false", "False")
        self.timeout_seconds = float(os.environ.get("MURMUR_ELEVENLABS_TIMEOUT", "20"))

    def transcribe(self, audio_path: Path, dictionary_terms: Iterable[str] | None = None) -> str:
        if not self.api_key:
            raise MurmurError(
                "Missing ElevenLabs API key.",
                "Set ELEVENLABS_API_KEY or MURMUR_ELEVENLABS_API_KEY in your environment before using stt.provider = \"elevenlabs\".",
            )
        if not audio_path.exists():
            raise MurmurError("Audio file does not exist.", str(audio_path))

        fields = {
            "model_id": self.model_name,
            "tag_audio_events": "false",
            "diarize": "false",
        }
        if self.no_verbatim:
            fields["no_verbatim"] = "true"
        # Supplying language improves speed/accuracy when configured. ElevenLabs
        # expects ISO-639 codes; Murmur's "auto" means omit this field.
        language = os.environ.get("MURMUR_ELEVENLABS_LANGUAGE")
        if language:
            fields["language_code"] = language

        body, content_type = _multipart_form_data(fields, file_field="file", file_path=audio_path)
        req = urllib.request.Request(
            self.endpoint,
            data=body,
            method="POST",
            headers={
                "xi-api-key": self.api_key,
                "Content-Type": content_type,
                "Accept": "application/json",
            },
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout_seconds) as response:
                payload = json.loads(response.read().decode("utf-8"))
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="replace")
            raise MurmurError("ElevenLabs transcription failed.", f"HTTP {exc.code}: {detail}") from exc
        except Exception as exc:
            raise MurmurError("ElevenLabs transcription failed.", str(exc)) from exc
        text = str(payload.get("text", "")).strip()
        if not text and isinstance(payload.get("transcripts"), list):
            text = " ".join(str(item.get("text", "")).strip() for item in payload["transcripts"] if isinstance(item, dict)).strip()
        return text


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
        elif config.provider == "groq":
            backend = GroqBackend()
            backend.model_name = config.model or backend.model_name
            if config.language and config.language != "auto":
                backend.language = config.language
        elif config.provider in ("elevenlabs", "eleven-labs", "scribe"):
            backend = ElevenLabsBackend()
            backend.model_name = config.model or backend.model_name
            if config.language and config.language != "auto":
                os.environ.setdefault("MURMUR_ELEVENLABS_LANGUAGE", config.language)
        else:
            backend = build_backend(config.provider)
        terms = list(dictionary_terms or [])
        return Transcription(text=backend.transcribe(audio_path, dictionary_terms=terms), provider=backend.name)
    except MurmurError as exc:
        raise SttError(exc.doctor()) from exc


def _groq_api_key() -> str | None:
    key = os.environ.get("MURMUR_GROQ_API_KEY") or os.environ.get("GROQ_API_KEY")
    if key:
        return key.strip()
    key_file = Path(os.environ.get("MURMUR_GROQ_API_KEY_FILE", Path.home() / ".config" / "murmur" / "groq_api_key")).expanduser()
    try:
        if key_file.exists():
            return key_file.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return None


def _elevenlabs_api_key() -> str | None:
    key = os.environ.get("MURMUR_ELEVENLABS_API_KEY") or os.environ.get("ELEVENLABS_API_KEY")
    if key:
        return key.strip()
    key_file = Path(os.environ.get("MURMUR_ELEVENLABS_API_KEY_FILE", Path.home() / ".config" / "murmur" / "elevenlabs_api_key")).expanduser()
    try:
        if key_file.exists():
            return key_file.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return None


def _multipart_form_data(fields: dict[str, str], *, file_field: str, file_path: Path) -> tuple[bytes, str]:
    boundary = "----murmur-elevenlabs-boundary"
    chunks: list[bytes] = []
    for name, value in fields.items():
        chunks.append(f"--{boundary}\r\n".encode("utf-8"))
        chunks.append(f'Content-Disposition: form-data; name="{name}"\r\n\r\n'.encode("utf-8"))
        chunks.append(str(value).encode("utf-8"))
        chunks.append(b"\r\n")
    filename = file_path.name
    chunks.append(f"--{boundary}\r\n".encode("utf-8"))
    chunks.append(f'Content-Disposition: form-data; name="{file_field}"; filename="{filename}"\r\n'.encode("utf-8"))
    chunks.append(b"Content-Type: audio/wav\r\n\r\n")
    chunks.append(file_path.read_bytes())
    chunks.append(b"\r\n")
    chunks.append(f"--{boundary}--\r\n".encode("utf-8"))
    return b"".join(chunks), f"multipart/form-data; boundary={boundary}"


def _dictionary_prompt(dictionary_terms: Iterable[str] | None) -> str:
    terms = [term.strip() for term in (dictionary_terms or []) if term.strip()]
    if not terms:
        return ""
    return "Vocabulary: " + ", ".join(terms[:80])
