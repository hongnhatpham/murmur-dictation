from __future__ import annotations

from dataclasses import asdict, dataclass
from datetime import datetime, timezone
import json
from pathlib import Path
import sqlite3
from typing import Iterable

from . import paths


SCHEMA = """
CREATE TABLE IF NOT EXISTS dictations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL,
    mode TEXT NOT NULL,
    provider TEXT NOT NULL,
    transcript TEXT,
    deterministic_text TEXT,
    final_text TEXT,
    correction_provider TEXT,
    correction_status TEXT NOT NULL DEFAULT 'skipped',
    correction_latency_ms INTEGER,
    focused_app_id TEXT,
    app_category TEXT,
    style_applied TEXT,
    actions TEXT NOT NULL DEFAULT '',
    insertion_status TEXT NOT NULL DEFAULT 'unknown',
    audio_duration_ms INTEGER,
    latency_ms INTEGER,
    error_message TEXT
);
CREATE INDEX IF NOT EXISTS idx_dictations_created_at ON dictations(created_at DESC);
"""


@dataclass(frozen=True)
class HistoryEntry:
    id: int
    created_at: str
    mode: str
    provider: str
    transcript: str | None
    final_text: str | None
    insertion_status: str
    error_message: str | None
    deterministic_text: str | None = None
    correction_provider: str | None = None
    correction_status: str = "skipped"
    correction_latency_ms: int | None = None
    focused_app_id: str | None = None
    app_category: str | None = None
    style_applied: str | None = None


@dataclass(frozen=True)
class HistoryRecord:
    created_at: str
    mode: str
    provider: str = "manual"
    transcript: str | None = None
    final_text: str | None = None
    copied: bool = False
    insertion_status: str = "unknown"
    error_message: str | None = None

    @classmethod
    def now(
        cls,
        *,
        mode: str,
        provider: str = "manual",
        transcript: str | None = None,
        final_text: str | None = None,
        copied: bool = False,
        insertion_status: str | None = None,
        error_message: str | None = None,
    ) -> "HistoryRecord":
        return cls(
            created_at=datetime.now(timezone.utc).isoformat(timespec="seconds"),
            mode=mode,
            provider=provider,
            transcript=transcript,
            final_text=final_text,
            copied=copied,
            insertion_status=insertion_status or ("copied-only" if copied else "unknown"),
            error_message=error_message,
        )


class HistoryStore:
    def __init__(self, db_path: Path):
        self.db_path = db_path

    def connect(self) -> sqlite3.Connection:
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        conn = sqlite3.connect(self.db_path)
        conn.row_factory = sqlite3.Row
        conn.executescript(SCHEMA)
        _ensure_columns(conn)
        return conn

    def add(
        self,
        *,
        mode: str,
        provider: str,
        transcript: str | None,
        final_text: str | None,
        insertion_status: str = "unknown",
        error_message: str | None = None,
        error: str | None = None,
        actions: list[object] | None = None,
        audio_duration_ms: int | None = None,
        latency_ms: int | None = None,
        created_at: str | None = None,
        deterministic_text: str | None = None,
        correction_provider: str | None = None,
        correction_status: str = "skipped",
        correction_latency_ms: int | None = None,
        focused_app_id: str | None = None,
        app_category: str | None = None,
        style_applied: str | None = None,
    ) -> int:
        timestamp = created_at or datetime.now(timezone.utc).isoformat(timespec="seconds")
        action_text = ",".join(getattr(a, "name", str(a)) for a in (actions or []))
        conn = self.connect()
        try:
            cur = conn.execute(
                """
                INSERT INTO dictations
                (created_at, mode, provider, transcript, deterministic_text, final_text, correction_provider, correction_status, correction_latency_ms, focused_app_id, app_category, style_applied, actions, insertion_status, audio_duration_ms, latency_ms, error_message)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    timestamp,
                    mode,
                    provider,
                    transcript,
                    deterministic_text if deterministic_text is not None else final_text,
                    final_text,
                    correction_provider,
                    correction_status,
                    correction_latency_ms,
                    focused_app_id,
                    app_category,
                    style_applied,
                    action_text,
                    insertion_status,
                    audio_duration_ms,
                    latency_ms,
                    error_message or error,
                ),
            )
            conn.commit()
            return int(cur.lastrowid)
        finally:
            conn.close()

    def recent(self, limit: int = 10) -> list[HistoryEntry]:
        conn = self.connect()
        try:
            rows = conn.execute(
                """
                SELECT id, created_at, mode, provider, transcript, deterministic_text, final_text, correction_provider, correction_status, correction_latency_ms, focused_app_id, app_category, style_applied, insertion_status, error_message
                FROM dictations
                ORDER BY id DESC
                LIMIT ?
                """,
                (limit,),
            ).fetchall()
            return [_entry(row) for row in rows]
        finally:
            conn.close()

    def last(self) -> HistoryEntry | None:
        items = self.recent(1)
        return items[0] if items else None


def _ensure_columns(conn: sqlite3.Connection) -> None:
    existing = {row[1] for row in conn.execute("PRAGMA table_info(dictations)")}
    for name, ddl in {
        "actions": "ALTER TABLE dictations ADD COLUMN actions TEXT NOT NULL DEFAULT ''",
        "audio_duration_ms": "ALTER TABLE dictations ADD COLUMN audio_duration_ms INTEGER",
        "latency_ms": "ALTER TABLE dictations ADD COLUMN latency_ms INTEGER",
        "deterministic_text": "ALTER TABLE dictations ADD COLUMN deterministic_text TEXT",
        "correction_provider": "ALTER TABLE dictations ADD COLUMN correction_provider TEXT",
        "correction_status": "ALTER TABLE dictations ADD COLUMN correction_status TEXT NOT NULL DEFAULT 'skipped'",
        "correction_latency_ms": "ALTER TABLE dictations ADD COLUMN correction_latency_ms INTEGER",
        "focused_app_id": "ALTER TABLE dictations ADD COLUMN focused_app_id TEXT",
        "app_category": "ALTER TABLE dictations ADD COLUMN app_category TEXT",
        "style_applied": "ALTER TABLE dictations ADD COLUMN style_applied TEXT",
    }.items():
        if name not in existing:
            conn.execute(ddl)


def _entry(row: sqlite3.Row) -> HistoryEntry:
    return HistoryEntry(
        id=int(row["id"]),
        created_at=str(row["created_at"]),
        mode=str(row["mode"]),
        provider=str(row["provider"]),
        transcript=row["transcript"],
        final_text=row["final_text"],
        insertion_status=str(row["insertion_status"]),
        error_message=row["error_message"],
        deterministic_text=row["deterministic_text"],
        correction_provider=row["correction_provider"],
        correction_status=str(row["correction_status"]),
        correction_latency_ms=row["correction_latency_ms"],
        focused_app_id=row["focused_app_id"],
        app_category=row["app_category"],
        style_applied=row["style_applied"],
    )


def default_store() -> HistoryStore:
    return HistoryStore(paths.history_db())


def add_entry(**kwargs) -> int:
    return default_store().add(**kwargs)


def recent(limit: int = 10) -> list[HistoryEntry]:
    return default_store().recent(limit)


def last() -> HistoryEntry | None:
    return default_store().last()


def append_history(record: HistoryRecord, path: Path | None = None) -> None:
    path = path or (paths.state_dir() / "history.jsonl")
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as f:
        f.write(json.dumps(asdict(record), ensure_ascii=False) + "\n")


def load_recent(path: Path | None = None, limit: int = 10) -> list[dict[str, object]]:
    path = path or (paths.state_dir() / "history.jsonl")
    if not path.exists():
        return []
    rows: list[dict[str, object]] = []
    with path.open("r", encoding="utf-8") as f:
        for line in f:
            if line.strip():
                rows.append(json.loads(line))
    return rows[-limit:][::-1]


def format_entries(entries: Iterable[HistoryEntry]) -> str:
    lines: list[str] = []
    for e in entries:
        text = (e.final_text or e.transcript or "").replace("\n", " ").strip()
        if len(text) > 80:
            text = text[:77] + "..."
        error = f" error={e.error_message}" if e.error_message else ""
        correction = f" correction={e.correction_status}"
        category = f" app={e.app_category}" if e.app_category else ""
        lines.append(
            f"{e.id:>4}  {e.created_at}  mode={e.mode} provider={e.provider} "
            f"status={e.insertion_status}{correction}{category}{error}  {text}"
        )
    return "\n".join(lines)
