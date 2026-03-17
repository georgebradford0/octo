"""Worker registry — tracks active chat sessions per branch."""

from __future__ import annotations

from dataclasses import dataclass

from fastapi import WebSocket


@dataclass
class WorkerInfo:
    branch: str
    worktree_path: str
    status: str = "connected"   # connected | disconnected
    websocket: WebSocket | None = None

    def to_dict(self) -> dict:
        return {
            "branch": self.branch,
            "worktree_path": self.worktree_path,
            "status": self.status,
        }


class WorkerPool:
    def __init__(self) -> None:
        self._workers: dict[str, WorkerInfo] = {}

    def register(self, branch: str, worktree_path: str, websocket: WebSocket) -> WorkerInfo:
        info = WorkerInfo(branch=branch, worktree_path=worktree_path, websocket=websocket)
        self._workers[branch] = info
        return info

    def deregister(self, branch: str) -> None:
        self._workers.pop(branch, None)

    async def disconnect(self, branch: str) -> None:
        info = self._workers.get(branch)
        if info and info.websocket:
            try:
                await info.websocket.close()
            except Exception:
                pass
        self.deregister(branch)

    async def stop_all(self) -> None:
        for branch in list(self._workers):
            await self.disconnect(branch)

    def get(self, branch: str) -> WorkerInfo | None:
        return self._workers.get(branch)

    def all(self) -> list[WorkerInfo]:
        return list(self._workers.values())
