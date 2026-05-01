from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
import re
import sqlite3
from typing import Iterable, Mapping

SCHEMA = """
CREATE TABLE IF NOT EXISTS dictionary_terms (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL,
    term TEXT NOT NULL UNIQUE COLLATE NOCASE,
    note TEXT,
    replacement TEXT,
    category TEXT
);
CREATE INDEX IF NOT EXISTS idx_dictionary_terms_term ON dictionary_terms(term COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS snippets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL,
    trigger TEXT NOT NULL UNIQUE COLLATE NOCASE,
    expansion TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_snippets_trigger ON snippets(trigger COLLATE NOCASE);
"""


@dataclass(frozen=True)
class DictionaryTerm:
    id: int
    term: str
    note: str | None = None
    replacement: str | None = None
    category: str | None = None


@dataclass(frozen=True)
class Snippet:
    id: int
    trigger: str
    expansion: str


class PersonalStore:
    """Local SQLite storage for personal vocabulary and text snippets."""

    def __init__(self, db_path: Path):
        self.db_path = db_path

    def connect(self) -> sqlite3.Connection:
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        conn = sqlite3.connect(self.db_path)
        conn.row_factory = sqlite3.Row
        conn.executescript(SCHEMA)
        _ensure_columns(conn)
        return conn

    def add_term(self, term: str, note: str | None = None, replacement: str | None = None, category: str | None = None) -> int:
        term = _normalize_required(term, "term")
        replacement = replacement.strip() if replacement else None
        category = category.strip() if category else None
        with self.connect() as conn:
            cur = conn.execute(
                """
                INSERT INTO dictionary_terms (created_at, term, note, replacement, category)
                VALUES (?, ?, ?, ?, ?)
                ON CONFLICT(term) DO UPDATE SET note = excluded.note, replacement = excluded.replacement, category = excluded.category
                """,
                (_now(), term, note, replacement, category),
            )
            if cur.lastrowid:
                return int(cur.lastrowid)
            row = conn.execute("SELECT id FROM dictionary_terms WHERE term = ?", (term,)).fetchone()
            return int(row["id"])

    def list_terms(self) -> list[DictionaryTerm]:
        with self.connect() as conn:
            rows = conn.execute("SELECT id, term, note, replacement, category FROM dictionary_terms ORDER BY lower(term)").fetchall()
        return [DictionaryTerm(id=int(r["id"]), term=str(r["term"]), note=r["note"], replacement=r["replacement"], category=r["category"]) for r in rows]

    def remove_term(self, key: str) -> bool:
        with self.connect() as conn:
            if key.isdigit():
                cur = conn.execute("DELETE FROM dictionary_terms WHERE id = ?", (int(key),))
            else:
                cur = conn.execute("DELETE FROM dictionary_terms WHERE term = ?", (key,))
            return cur.rowcount > 0

    def add_snippet(self, trigger: str, expansion: str) -> int:
        trigger = _normalize_required(trigger, "trigger")
        expansion = _normalize_required(expansion, "expansion")
        with self.connect() as conn:
            cur = conn.execute(
                """
                INSERT INTO snippets (created_at, trigger, expansion)
                VALUES (?, ?, ?)
                ON CONFLICT(trigger) DO UPDATE SET expansion = excluded.expansion
                """,
                (_now(), trigger, expansion),
            )
            if cur.lastrowid:
                return int(cur.lastrowid)
            row = conn.execute("SELECT id FROM snippets WHERE trigger = ?", (trigger,)).fetchone()
            return int(row["id"])

    def list_snippets(self) -> list[Snippet]:
        with self.connect() as conn:
            rows = conn.execute("SELECT id, trigger, expansion FROM snippets ORDER BY lower(trigger)").fetchall()
        return [Snippet(id=int(r["id"]), trigger=str(r["trigger"]), expansion=str(r["expansion"])) for r in rows]

    def remove_snippet(self, key: str) -> bool:
        with self.connect() as conn:
            if key.isdigit():
                cur = conn.execute("DELETE FROM snippets WHERE id = ?", (int(key),))
            else:
                cur = conn.execute("DELETE FROM snippets WHERE trigger = ?", (key,))
            return cur.rowcount > 0

    def snippet_map(self) -> dict[str, str]:
        return {s.trigger: s.expansion for s in self.list_snippets()}


def _ensure_columns(conn: sqlite3.Connection) -> None:
    existing = {row[1] for row in conn.execute("PRAGMA table_info(dictionary_terms)")}
    for name, ddl in {
        "replacement": "ALTER TABLE dictionary_terms ADD COLUMN replacement TEXT",
        "category": "ALTER TABLE dictionary_terms ADD COLUMN category TEXT",
    }.items():
        if name not in existing:
            conn.execute(ddl)


def apply_dictionary_terms(text: str, terms: Iterable[str | DictionaryTerm]) -> str:
    """Restore preferred casing/spelling for known terms in cleaned text.

    This intentionally stays deterministic: if Whisper produced the same words
    with different case (for example, "niri" vs "niri"/"Niri" or "aria 03" vs
    "ARIA-03" only when punctuation already matches), Murmur restores the stored
    spelling. It does not attempt fuzzy correction yet.
    """

    result = text
    replacements: dict[str, str] = {}
    for item in terms:
        if isinstance(item, DictionaryTerm):
            source = item.term.strip()
            target = (item.replacement or item.term).strip()
        else:
            source = str(item).strip()
            target = source
        if source:
            replacements[source] = target
    for source, target in sorted(replacements.items(), key=lambda item: len(item[0]), reverse=True):
        pattern = re.compile(rf"(?<!\w){re.escape(source)}(?!\w)", re.IGNORECASE)
        result = pattern.sub(target, result)
    return result


def apply_snippets(text: str, snippets: Mapping[str, str]) -> str:
    result = text
    for trigger, expansion in sorted(snippets.items(), key=lambda item: len(item[0]), reverse=True):
        if not trigger:
            continue
        pattern = re.compile(rf"(?<!\w){re.escape(trigger)}(?!\w)")
        result = pattern.sub(expansion, result)
    return result


def format_terms(terms: Iterable[DictionaryTerm]) -> str:
    lines = []
    for term in terms:
        replacement = f" -> {term.replacement}" if term.replacement else ""
        category = f" [{term.category}]" if term.category else ""
        note = f"  # {term.note}" if term.note else ""
        lines.append(f"{term.id:>4}  {term.term}{replacement}{category}{note}")
    return "\n".join(lines)


def format_snippets(snippets: Iterable[Snippet]) -> str:
    lines = []
    for snippet in snippets:
        expansion = snippet.expansion.replace("\n", "\\n")
        if len(expansion) > 80:
            expansion = expansion[:77] + "..."
        lines.append(f"{snippet.id:>4}  {snippet.trigger} -> {expansion}")
    return "\n".join(lines)


def _normalize_required(value: str, name: str) -> str:
    normalized = value.strip()
    if not normalized:
        raise ValueError(f"{name} must not be empty")
    return normalized


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")
