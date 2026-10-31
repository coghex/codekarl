"""Resolve bound chat sessions and read immutable-prefix model evidence."""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import re
from collections.abc import Sequence
from pathlib import Path
from typing import NamedTuple, NoReturn, cast

from . import lease
from .records import (
    AgentSession,
    ModelRun,
    ModelUse,
    RecordError,
    RegistryRow,
    Session,
    StoredModelUse,
    load_session_roots,
    registry_row,
)

Parsed = tuple[list[ModelUse], int, str, ModelUse | None]  # contributions, length, sha256, current


class Direct(NamedTuple):
    """A Copilot session the client itself vouches for: its row, validated log and history."""

    row: RegistryRow
    log: Path
    history: Parsed


UUID = re.compile(r"[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}")


def _rows(raw: object) -> list[RegistryRow]:
    """The usable rows of a registry listing; a value that is not a row cannot match this process."""
    rows = raw if isinstance(raw, list) else []
    return [row for row in map(registry_row, rows) if row is not None]


def _registry() -> list[RegistryRow]:
    override = os.environ.get("CK_CHAT_REGISTRY")
    if override:
        value = json.loads(Path(override).read_bytes())
        return _rows(value if isinstance(value, list) else value.get("agents", []))
    result = lease.run(
        ["pchat", "agent", "list", "--project", "codekarl", "--all", "--json"],
        capture_output=True,
        text=True,
        check=True,
    )
    rows = json.loads(result.stdout)
    if not isinstance(rows, list):
        raise RuntimeError(
            "`pchat agent list --json` did not return a JSON list; run it by hand to inspect its output"
        )
    return _rows(rows)


def _ancestors(pid: int | None = None) -> list[int]:
    """Return the process and its ancestors, nearest first."""
    current = os.getpid() if pid is None else pid
    result: list[int] = []
    for _ in range(128):
        if current <= 1 or current in result:
            break
        result.append(current)
        raw = lease.check_output(["ps", "-o", "ppid=", "-p", str(current)], text=True).strip()
        if not raw:
            break
        current = int(raw)
    return result


def _executable(pid: int) -> str:
    process = lease.run(
        ["ps", "-o", "comm=", "-p", str(pid)], capture_output=True, text=True, check=False
    )
    return Path(process.stdout.strip()).name


AGENT_CLIENTS = ("claude", "codex", "copilot")


def agent_ancestor() -> str | None:
    """The nearest ancestor process that is an agent client (`name (pid N)`), checked over the whole
    ancestry; None when there is none. `codex-` helpers count as Codex."""
    for pid in _ancestors():
        name = _executable(pid)
        if name in AGENT_CLIENTS or name.startswith("codex-"):
            return f"{name} (pid {pid})"
    return None


def terminals_present() -> bool:
    """Standard input and standard output are both terminals."""
    return os.isatty(0) and os.isatty(1)


TTY_PATH = "/dev/tty"  # the controlling terminal; tests point it at a disposable device


def read_confirmation(prompt: str) -> str:
    """One line typed at the controlling terminal; refuses when it cannot be opened.

    A terminal is not seekable, so it is read and written through two separate unbuffered-for-seeking
    handles on one descriptor (never a buffered read/write stream, which requires seeking)."""
    try:
        descriptor = os.open(TTY_PATH, os.O_RDWR | os.O_NOCTTY)
        with (
            os.fdopen(os.dup(descriptor), "w") as terminal_out,
            os.fdopen(descriptor) as terminal_in,
        ):
            terminal_out.write(prompt)
            terminal_out.flush()
            return terminal_in.readline().rstrip("\n")
    except OSError as error:
        raise RuntimeError(f"Cannot read a confirmation from {TTY_PATH}: {error}") from error


def _copilot_evidence(chain: list[int]) -> Direct:  # noqa: PLR0912,PLR0915  # linear workflow kept whole so failure ordering stays visible
    """Validate the Copilot client evidence inherited by this process; never fall back."""

    def refuse(reason: str) -> NoReturn:
        raise RuntimeError(f"Copilot session evidence refused: {reason}")

    session = os.environ["COPILOT_AGENT_SESSION_ID"]
    if not UUID.fullmatch(session):
        refuse("COPILOT_AGENT_SESSION_ID is not a canonical lowercase UUID")
    roots = load_session_roots(
        json.loads(os.environ.get("CK_SESSION_ROOTS", "{}")), "CK_SESSION_ROOTS"
    )
    root = Path(roots.get("copilot", Path.home() / ".copilot/session-state")).expanduser()
    directory = root / session
    if directory.is_symlink() or not directory.is_dir():
        refuse(f"session directory {directory} is missing or a symlink")
    log = directory / "events.jsonl"
    if log.is_symlink() or not log.is_file():
        refuse(f"{log} is missing or not a regular file")
    candidates = []
    for path in directory.glob("inuse.*.lock"):
        text = path.name[len("inuse.") : -len(".lock")]
        if text.isdecimal() and text == str(int(text)) and int(text) in chain:
            candidates.append((int(text), path))
    if len(candidates) != 1:
        refuse(f"expected exactly one ancestor inuse lock, found {len(candidates)}")
    pid, lock = candidates[0]
    if lock.is_symlink() or lock.read_text().strip() != str(pid):
        refuse(f"{lock.name} content is not its pid")
    if _executable(pid) != "copilot":
        refuse(f"ancestor {pid} is not a copilot process")
    hold = directory / f"inuse.{pid}.hold"
    try:
        descriptor = os.open(hold, os.O_RDONLY | os.O_NOFOLLOW)
    except OSError as error:
        refuse(f"{hold.name} cannot be opened: {error}")
    try:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_SH | fcntl.LOCK_NB)
        except BlockingIOError:
            pass
        except OSError as error:
            refuse(f"{hold.name} cannot be probed: {error}")
        else:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
            refuse(f"{hold.name} is not held by a live client")
    finally:
        os.close(descriptor)
    with log.open("rb") as handle:
        first = handle.readline()
    try:
        event = json.loads(first)
        started = event["type"] == "session.start" and event["data"]["sessionId"] == session
    except (ValueError, KeyError, TypeError):
        started = False
    if not started:
        refuse("first event is not session.start for this session")
    row: RegistryRow = {
        "name": f"copilot:{session}",
        "brand": "copilot",
        "session_id": session,
        "role": None,
        "pid": pid,
    }
    return Direct(row, log, parse(log, "copilot"))


def _bound() -> tuple[RegistryRow | None, Path | None, Parsed | None]:
    """Return the bound row and, for direct Copilot evidence, its validated log and history."""
    if os.environ.get("CK_SESSION_EVIDENCE"):
        evidence = Path(os.environ["CK_SESSION_EVIDENCE"])
        row = registry_row(json.loads(evidence.read_bytes()))
        if row is None:
            raise RuntimeError(f"{evidence}: not a bound session registry entry")
        return row, None, None
    chain = _ancestors()
    direct = _copilot_evidence(chain) if "COPILOT_AGENT_SESSION_ID" in os.environ else None
    if direct is None and any(_executable(pid) == "copilot" for pid in chain):
        raise RuntimeError("Copilot ancestor without COPILOT_AGENT_SESSION_ID")
    matches = [row for row in _registry() if row.get("pid") in chain]
    if len(matches) > 1:
        matches.sort(key=lambda row: row.get("pid_start", ""), reverse=True)
        matches = matches[:1]
    row = matches[0] if matches else None
    if direct and (row is None or chain.index(direct.row["pid"]) < chain.index(row["pid"])):
        return direct.row, direct.log, direct.history
    return row, None, None


def bound_session() -> RegistryRow | None:
    return _bound()[0]


def effective_role(session: Session, declared: str | None) -> str | None:
    """Return the role an agent acts in (None for the owner), refusing unverifiable claims."""
    if session.get("human"):
        if declared:
            raise RuntimeError("declared role requires verifiable agent session evidence")
        return None
    bound = session.get("role")
    if bound:
        if declared and declared != bound:
            raise RuntimeError("Declared role contradicts the bound chat identity")
        return str(bound)
    if not declared:
        raise RuntimeError("unbound agent session must declare --role")
    return declared


def _session_log(client: str, session: str) -> Path:
    roots = load_session_roots(
        json.loads(os.environ.get("CK_SESSION_ROOTS", "{}")), "CK_SESSION_ROOTS"
    )
    if client in roots:
        root = Path(roots[client]).expanduser()
    elif client == "codex":
        root = Path.home() / ".codex/sessions"
    elif client == "claude":
        root = Path.home() / ".claude/projects"
    elif client == "copilot":
        return Path.home() / ".copilot/session-state" / session / "events.jsonl"
    else:
        raise RuntimeError(f"Unsupported session client {client!r}")
    matches = list(root.rglob(f"*{session}*.jsonl"))
    if len(matches) != 1:
        raise RuntimeError(f"Session log is missing or ambiguous for {session}")
    return matches[0]


def history(path: Path, client: str) -> tuple[list[ModelUse], int, str]:
    """Return the contribution list, the prefix length and its sha256."""
    models, length, digest, _ = parse(path, client)
    return models, length, digest


def compress_history(models: Sequence[ModelUse]) -> list[StoredModelUse]:
    """Store a per-turn session history as maximal consecutive model/effort runs."""
    runs: list[ModelRun] = []
    for item in models:
        if runs and (runs[-1]["model"], runs[-1]["effort"]) == (item["model"], item["effort"]):
            runs[-1]["turns"] += 1
        else:
            runs.append({"model": item["model"], "effort": item["effort"], "turns": 1})
    return cast("list[StoredModelUse]", runs)


def expand_history(models: Sequence[StoredModelUse]) -> list[ModelUse]:
    """Expand either legacy per-turn entries or stored runs into the session-history shape."""
    expanded: list[ModelUse] = []
    for item in models:
        turns = item.get("turns", 1)
        if not isinstance(turns, int) or isinstance(turns, bool) or turns < 1:
            raise RecordError("Stored model history turns must be a positive integer")
        expanded.extend({"model": item["model"], "effort": item["effort"]} for _ in range(turns))
    return expanded


def _name(value: object, what: str, client: str, number: int) -> str | None:
    """A model or effort named by a log event: a string, or None when the event leaves it out.

    Any other type is not a value the log can establish, so it is refused here, naming the line,
    rather than exported as a `ModelUse` string it is not."""
    if value is None or value == "":
        return None
    if not isinstance(value, str):
        raise RecordError(
            f"Malformed {client} session event at line {number}: {what} must be a string, "
            f"found {type(value).__name__}; inspect that line of the session log"
        )
    return value


def parse(path: Path, client: str) -> Parsed:  # noqa: PLR0912,PLR0915  # linear workflow kept whole so failure ordering stays visible
    """Read the contribution list and the current model selection in one pass.

    The current pair is None when the log does not establish it: a Codex context that lacks a
    model or effort, or a Copilot selection change that no later contribution confirms.
    """
    raw = path.read_bytes()
    models: list[ModelUse] = []
    current_model: str | None = None
    current_effort: str | None = None
    codex_current: ModelUse | None = None
    unconfirmed = False
    for number, line in enumerate(raw.splitlines(keepends=True), 1):
        try:
            event = json.loads(line)
            kind = event.get("type")
            data = event.get("payload", event.get("data", event))
            if client == "codex" and kind == "turn_context":
                current_model = _name(data.get("model"), "model", client, number)
                current_effort = _name(
                    data.get("effort", data.get("reasoning_effort")), "effort", client, number
                )
                codex_current = None
                if current_model and current_effort:
                    codex_current = {"model": current_model, "effort": current_effort}
                    models.append({"model": current_model, "effort": current_effort})
            elif client == "copilot":
                selected = False
                if kind == "session.start":
                    current_model = _name(
                        data.get("selectedModel"), "selectedModel", client, number
                    )
                    current_effort = _name(
                        data.get("reasoningEffort"), "reasoningEffort", client, number
                    )
                    selected = True
                elif kind == "session.model_change":
                    current_model = _name(data.get("newModel"), "newModel", client, number)
                    current_effort = _name(
                        data.get("reasoningEffort", current_effort),
                        "reasoningEffort",
                        client,
                        number,
                    )
                    selected = True
                elif kind == "user.message":
                    choice = data.get("responsesReasoning") or {}
                    chosen = _name(choice.get("model"), "responsesReasoning.model", client, number)
                    if chosen:
                        current_model = chosen
                        current_effort = _name(
                            choice.get("effort"), "responsesReasoning.effort", client, number
                        )
                        selected = True
                elif kind in ("assistant.message", "tool.execution_start"):
                    model = _name(data.get("model"), "model", client, number)
                    if not current_model or not current_effort:
                        raise RuntimeError(f"Unselected agent model before event {number}")
                    if model and model != current_model:
                        models.append({"model": model, "effort": current_effort})
                    else:
                        models.append({"model": current_model, "effort": current_effort})
                    unconfirmed = False
                if selected and models:
                    last = models[-1]
                    if (
                        not current_model
                        or not current_effort
                        or current_model != last["model"]
                        or current_effort != last["effort"]
                    ):
                        unconfirmed = True
            elif client == "claude" and kind in ("assistant", "assistant.message"):
                message = data.get("message")
                # Claude also emits synthetic "No response requested." events with this model marker.
                if data.get("isApiErrorMessage") is True or (
                    isinstance(message, dict) and message.get("model") == "<synthetic>"
                ):
                    continue
                model = _name((message or {}).get("model"), "model", client, number)
                effort = _name(data.get("effort"), "effort", client, number) or _name(
                    data.get("reasoning_effort"), "reasoning_effort", client, number
                )
                if not model or not effort:
                    raise RuntimeError(f"Unresolvable Claude effort at event {number}")
                models.append({"model": model, "effort": effort})
        except (json.JSONDecodeError, TypeError, AttributeError) as error:
            raise RuntimeError(f"Unparseable {client} session event at line {number}") from error
    if not models:
        raise RuntimeError("Session model history is unresolvable")
    if client == "codex":
        current = codex_current
    elif client == "copilot":
        current = None if unconfirmed else models[-1].copy()
    else:
        current = models[-1].copy()
    return models, len(raw), hashlib.sha256(raw).hexdigest(), current


def resolve() -> Session:
    row, log, loaded = _bound()
    if row is None:
        return {"human": True, "identity": "human", "family": "human", "role": "owner"}
    identity = row.get("name")
    client = row.get("brand")
    session = row.get("session_id")
    if not identity or not client or not session:
        raise RuntimeError("Bound session registry entry is incomplete")
    path = log or _session_log(client, session)
    models, offset, digest, current = loaded or parse(path, client)
    result: AgentSession = {
        "identity": identity,
        "client": client,
        "session": session,
        "role": row.get("role"),
        "models": models,
        "current": current,
        "families": sorted({item["model"] for item in models}),
        "evidence": {"path": str(path), "offset": offset, "sha256": digest},
    }
    if log:
        result["pid"] = row["pid"]
    return result
