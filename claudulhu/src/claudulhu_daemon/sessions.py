"""Persistent session store — saves/loads Claude session IDs per branch to disk."""

from __future__ import annotations

import json
import os
from dataclasses import asdict, dataclass
from typing import Any


@dataclass
class SessionRecord:
    branch: str
    sdk_session_id: str         # Claude SDK session_id from ResultMessage
    worktree_path: str
    last_seen: str              # ISO-8601 timestamp


class SessionStore:
    def __init__(self, repo_name: str):
        self._dir = os.path.expanduser(f"~/.claudulhu/sessions/{repo_name}")
        os.makedirs(self._dir, exist_ok=True)

    def _path(self, branch: str) -> str:
        safe = branch.replace("/", "__")
        return os.path.join(self._dir, f"{safe}.json")

    def save(self, record: SessionRecord) -> None:
        with open(self._path(record.branch), "w") as f:
            json.dump(asdict(record), f, indent=2)

    def load(self, branch: str) -> SessionRecord | None:
        path = self._path(branch)
        if not os.path.exists(path):
            return None
        try:
            with open(path) as f:
                data: dict[str, Any] = json.load(f)
            return SessionRecord(**data)
        except Exception:
            return None

    def delete(self, branch: str) -> None:
        path = self._path(branch)
        if os.path.exists(path):
            os.remove(path)

    def all(self) -> list[SessionRecord]:
        records = []
        for fname in os.listdir(self._dir):
            if not fname.endswith(".json"):
                continue
            branch = fname[:-5].replace("__", "/")
            rec = self.load(branch)
            if rec:
                records.append(rec)
        return records
