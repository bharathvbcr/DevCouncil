"""Native control-plane storage repositories."""

from __future__ import annotations

import json
import secrets
import uuid
from datetime import datetime, timedelta, timezone
from typing import Optional

from pydantic import BaseModel
from sqlalchemy.exc import IntegrityError
from sqlmodel import Session, col, select

from devcouncil.utils.json_persist import dump_json

from devcouncil.storage.models import (
    AgentHandoffModel,
    CorrectionManifestModel,
    FileChangeEventModel,
    SemanticDiffModel,
    ShellCommandEventModel,
    ShellSessionModel,
    TaskLeaseModel,
    VerificationRunModel,
)


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


class TaskLeaseRecord(BaseModel):
    id: str
    task_id: str
    owner: str
    agent: Optional[str] = None
    client_id: Optional[str] = None
    run_id: Optional[str] = None
    branch: Optional[str] = None
    lease_token: str
    status: str
    created_at: str
    expires_at: Optional[str] = None
    released_at: Optional[str] = None


class ShellSessionRecord(BaseModel):
    id: str
    task_id: str
    lease_id: Optional[str] = None
    shell: str
    cwd: str
    status: str
    started_at: str
    ended_at: Optional[str] = None


class ShellCommandRecord(BaseModel):
    id: int
    task_id: str
    session_id: Optional[str] = None
    command: str
    status: str
    exit_code: Optional[int] = None
    reason: str = ""
    stdout_path: str = ""
    stderr_path: str = ""
    created_at: str


class FileChangeRecord(BaseModel):
    id: int
    task_id: Optional[str] = None
    lease_id: Optional[str] = None
    session_id: Optional[str] = None
    path: str
    operation: str
    allowed: bool
    reason: str = ""
    created_at: str


class SemanticDiffRecord(BaseModel):
    id: int
    task_id: str
    before_snapshot_path: str
    after_snapshot_path: str
    classifications: list[dict]
    summary: str
    created_at: str


class AgentHandoffRecord(BaseModel):
    id: str
    task_id: str
    from_agent: str
    to_agent: str
    run_id: str
    manifest_path: str
    status: str
    created_at: str


class CorrectionManifestRecord(BaseModel):
    id: str
    task_id: str
    run_id: Optional[str] = None
    manifest_path: str
    retry_budget: int
    attempt: int
    status: str
    created_at: str


class VerificationRunRecord(BaseModel):
    id: str
    task_id: str
    sandbox: str
    environment: dict
    commands: list[dict]
    status: str
    started_at: str
    finished_at: Optional[str] = None


def _lease_from_model(model: TaskLeaseModel) -> TaskLeaseRecord:
    return TaskLeaseRecord.model_validate(model.model_dump())


class TaskLeaseRepository:
    def __init__(self, session: Session):
        self.session = session

    def acquire(
        self,
        task_id: str,
        owner: str,
        *,
        agent: Optional[str] = None,
        client_id: Optional[str] = None,
        run_id: Optional[str] = None,
        branch: Optional[str] = None,
        ttl_seconds: Optional[int] = None,
        force: bool = False,
    ) -> TaskLeaseRecord:
        active = self._active_model_for_task(task_id)
        if active is not None and not force:
            raise ValueError(f"Active lease already exists for task {task_id}")
        if active is not None and force:
            active.status = "stale"
            active.released_at = _utc_now()
            self.session.add(active)
            self.session.commit()

        lease_id = str(uuid.uuid4())
        token = secrets.token_urlsafe(32)
        expires_at = None
        if ttl_seconds is not None:
            expires_at = (datetime.now(timezone.utc) + timedelta(seconds=ttl_seconds)).isoformat()

        model = TaskLeaseModel(
            id=lease_id,
            task_id=task_id,
            owner=owner,
            agent=agent,
            client_id=client_id,
            run_id=run_id,
            branch=branch,
            lease_token=token,
            status="active",
            created_at=_utc_now(),
            expires_at=expires_at,
        )
        self.session.add(model)
        try:
            self.session.commit()
        except IntegrityError as exc:
            # Lost a concurrent race: another writer inserted the active lease between
            # our active_for_task() check and this commit. The partial unique index
            # rejected the duplicate — surface it as the same conflict callers expect.
            self.session.rollback()
            raise ValueError(f"Active lease already exists for task {task_id}") from exc
        self.session.refresh(model)
        return _lease_from_model(model)

    def release(self, task_id: str, lease_token: str, status: str = "released") -> bool:
        model = self._active_model_for_task(task_id)
        if model is None or model.lease_token != lease_token:
            return False
        model.status = status
        model.released_at = _utc_now()
        self.session.add(model)
        self.session.commit()
        return True

    def _active_model_for_task(self, task_id: str) -> Optional[TaskLeaseModel]:
        """Return the live ACTIVE lease *model* for a task (or None), lazily expiring a
        lease that is past its TTL. Callers that need a Record wrap the result via
        _lease_from_model — this avoids re-fetching the same row by primary key after a
        Record-returning lookup."""
        statement = (
            select(TaskLeaseModel)
            .where(TaskLeaseModel.task_id == task_id)
            .where(TaskLeaseModel.status == "active")
        )
        model = self.session.exec(statement).first()
        if model is None:
            return None
        if model.expires_at:
            expires = datetime.fromisoformat(model.expires_at)
            if expires.tzinfo is None:
                expires = expires.replace(tzinfo=timezone.utc)
            if datetime.now(timezone.utc) > expires:
                model.status = "stale"
                model.released_at = _utc_now()
                self.session.add(model)
                self.session.commit()
                return None
        return model

    def active_for_task(self, task_id: str) -> TaskLeaseRecord | None:
        model = self._active_model_for_task(task_id)
        if model is None:
            return None
        return _lease_from_model(model)

    def validate(self, task_id: str, lease_token: str) -> bool:
        active = self.active_for_task(task_id)
        return active is not None and active.lease_token == lease_token

    def renew(self, task_id: str, lease_token: str, ttl_seconds: int) -> TaskLeaseRecord | None:
        """Push the lease's expiry out by ``ttl_seconds`` from now. Returns the updated
        record, or None when the token is invalid / the lease already expired."""
        model = self._active_model_for_task(task_id)
        if model is None or model.lease_token != lease_token:
            return None
        model.expires_at = (datetime.now(timezone.utc) + timedelta(seconds=ttl_seconds)).isoformat()
        self.session.add(model)
        self.session.commit()
        self.session.refresh(model)
        return _lease_from_model(model)

    def list_leases(self, *, active_only: bool = True) -> list[tuple[TaskLeaseRecord, bool]]:
        """Return ``(lease, expired)`` pairs for fleet supervision. ``expired`` means the
        lease is still marked active but past its TTL (a crashed/disconnected agent)."""
        statement = select(TaskLeaseModel)
        if active_only:
            statement = statement.where(TaskLeaseModel.status == "active")
        statement = statement.order_by(col(TaskLeaseModel.created_at).desc())
        now = datetime.now(timezone.utc)
        results: list[tuple[TaskLeaseRecord, bool]] = []
        for model in self.session.exec(statement).all():
            expired = False
            if model.status == "active" and model.expires_at:
                expires = datetime.fromisoformat(model.expires_at)
                if expires.tzinfo is None:
                    expires = expires.replace(tzinfo=timezone.utc)
                expired = now > expires
            results.append((_lease_from_model(model), expired))
        return results


class ShellSessionRepository:
    def __init__(self, session: Session):
        self.session = session

    def start(
        self,
        task_id: str,
        shell: str,
        cwd: str,
        *,
        lease_id: Optional[str] = None,
        status: str = "active",
    ) -> ShellSessionRecord:
        session_id = str(uuid.uuid4())
        model = ShellSessionModel(
            id=session_id,
            task_id=task_id,
            lease_id=lease_id,
            shell=shell,
            cwd=cwd,
            status=status,
            started_at=_utc_now(),
        )
        self.session.add(model)
        self.session.commit()
        self.session.refresh(model)
        return ShellSessionRecord.model_validate(model.model_dump())

    def finish(self, session_id: str, status: str) -> bool:
        model = self.session.get(ShellSessionModel, session_id)
        if model is None:
            return False
        model.status = status
        model.ended_at = _utc_now()
        self.session.add(model)
        self.session.commit()
        return True


class ShellCommandRepository:
    def __init__(self, session: Session):
        self.session = session

    def record(
        self,
        task_id: str,
        command: str,
        status: str,
        *,
        session_id: Optional[str] = None,
        exit_code: Optional[int] = None,
        reason: str = "",
        stdout_path: str = "",
        stderr_path: str = "",
    ) -> ShellCommandRecord:
        model = ShellCommandEventModel(
            task_id=task_id,
            session_id=session_id,
            command=command,
            status=status,
            exit_code=exit_code,
            reason=reason,
            stdout_path=stdout_path,
            stderr_path=stderr_path,
            created_at=_utc_now(),
        )
        self.session.add(model)
        self.session.commit()
        self.session.refresh(model)
        return ShellCommandRecord.model_validate(model.model_dump())


class FileChangeRepository:
    def __init__(self, session: Session):
        self.session = session

    def record(
        self,
        path: str,
        operation: str,
        allowed: bool,
        *,
        task_id: Optional[str] = None,
        lease_id: Optional[str] = None,
        session_id: Optional[str] = None,
        reason: str = "",
    ) -> FileChangeRecord:
        model = FileChangeEventModel(
            task_id=task_id,
            lease_id=lease_id,
            session_id=session_id,
            path=path,
            operation=operation,
            allowed=allowed,
            reason=reason,
            created_at=_utc_now(),
        )
        self.session.add(model)
        self.session.commit()
        self.session.refresh(model)
        return FileChangeRecord.model_validate(model.model_dump())

    def list_for_task(self, task_id: str) -> list[FileChangeRecord]:
        statement = (
            select(FileChangeEventModel)
            .where(FileChangeEventModel.task_id == task_id)
            .order_by(col(FileChangeEventModel.created_at))
        )
        return [FileChangeRecord.model_validate(m.model_dump()) for m in self.session.exec(statement).all()]


class SemanticDiffRepository:
    def __init__(self, session: Session):
        self.session = session

    def save(
        self,
        task_id: str,
        before_snapshot_path: str,
        after_snapshot_path: str,
        classifications: list[dict],
        summary: str,
    ) -> SemanticDiffRecord:
        model = SemanticDiffModel(
            task_id=task_id,
            before_snapshot_path=before_snapshot_path,
            after_snapshot_path=after_snapshot_path,
            classifications_json=dump_json(classifications),
            summary=summary,
            created_at=_utc_now(),
        )
        self.session.add(model)
        self.session.commit()
        self.session.refresh(model)
        return SemanticDiffRecord(
            id=model.id or 0,
            task_id=model.task_id,
            before_snapshot_path=model.before_snapshot_path,
            after_snapshot_path=model.after_snapshot_path,
            classifications=classifications,
            summary=model.summary,
            created_at=model.created_at,
        )

    def latest_for_task(self, task_id: str) -> SemanticDiffRecord | None:
        statement = (
            select(SemanticDiffModel)
            .where(SemanticDiffModel.task_id == task_id)
            .order_by(col(SemanticDiffModel.created_at).desc())
        )
        model = self.session.exec(statement).first()
        if model is None:
            return None
        return SemanticDiffRecord(
            id=model.id or 0,
            task_id=model.task_id,
            before_snapshot_path=model.before_snapshot_path,
            after_snapshot_path=model.after_snapshot_path,
            classifications=json.loads(model.classifications_json),
            summary=model.summary,
            created_at=model.created_at,
        )


class AgentHandoffRepository:
    def __init__(self, session: Session):
        self.session = session

    def save(
        self,
        task_id: str,
        from_agent: str,
        to_agent: str,
        run_id: str,
        manifest_path: str,
        status: str,
        *,
        handoff_id: Optional[str] = None,
    ) -> AgentHandoffRecord:
        hid = handoff_id or str(uuid.uuid4())
        model = AgentHandoffModel(
            id=hid,
            task_id=task_id,
            from_agent=from_agent,
            to_agent=to_agent,
            run_id=run_id,
            manifest_path=manifest_path,
            status=status,
            created_at=_utc_now(),
        )
        self.session.add(model)
        self.session.commit()
        self.session.refresh(model)
        return AgentHandoffRecord.model_validate(model.model_dump())


class CorrectionManifestRepository:
    def __init__(self, session: Session):
        self.session = session

    def save(
        self,
        task_id: str,
        manifest_path: str,
        status: str,
        *,
        run_id: Optional[str] = None,
        retry_budget: int = 3,
        attempt: int = 0,
        manifest_id: Optional[str] = None,
    ) -> CorrectionManifestRecord:
        mid = manifest_id or str(uuid.uuid4())
        model = CorrectionManifestModel(
            id=mid,
            task_id=task_id,
            run_id=run_id,
            manifest_path=manifest_path,
            retry_budget=retry_budget,
            attempt=attempt,
            status=status,
            created_at=_utc_now(),
        )
        self.session.add(model)
        self.session.commit()
        self.session.refresh(model)
        return CorrectionManifestRecord.model_validate(model.model_dump())

    def latest_for_task(self, task_id: str) -> CorrectionManifestRecord | None:
        statement = (
            select(CorrectionManifestModel)
            .where(CorrectionManifestModel.task_id == task_id)
            .order_by(col(CorrectionManifestModel.created_at).desc())
        )
        model = self.session.exec(statement).first()
        if model is None:
            return None
        return CorrectionManifestRecord.model_validate(model.model_dump())


class VerificationRunRepository:
    def __init__(self, session: Session):
        self.session = session

    def save(
        self,
        task_id: str,
        sandbox: str,
        environment: dict,
        commands: list[dict],
        status: str,
        *,
        run_id: Optional[str] = None,
        finished_at: Optional[str] = None,
    ) -> VerificationRunRecord:
        rid = run_id or str(uuid.uuid4())
        model = VerificationRunModel(
            id=rid,
            task_id=task_id,
            sandbox=sandbox,
            environment_json=dump_json(environment),
            commands_json=dump_json(commands),
            status=status,
            started_at=_utc_now(),
            finished_at=finished_at,
        )
        self.session.add(model)
        self.session.commit()
        self.session.refresh(model)
        return VerificationRunRecord(
            id=model.id,
            task_id=model.task_id,
            sandbox=model.sandbox,
            environment=environment,
            commands=commands,
            status=model.status,
            started_at=model.started_at,
            finished_at=model.finished_at,
        )

    def list_for_task(self, task_id: str) -> list[VerificationRunRecord]:
        statement = (
            select(VerificationRunModel)
            .where(VerificationRunModel.task_id == task_id)
            .order_by(col(VerificationRunModel.started_at))
        )
        records: list[VerificationRunRecord] = []
        for model in self.session.exec(statement).all():
            records.append(VerificationRunRecord(
                id=model.id,
                task_id=model.task_id,
                sandbox=model.sandbox,
                environment=json.loads(model.environment_json),
                commands=json.loads(model.commands_json),
                status=model.status,
                started_at=model.started_at,
                finished_at=model.finished_at,
            ))
        return records
