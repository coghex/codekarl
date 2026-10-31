"""Codekarl inbox and posting entry point. Other projects use pchat directly."""

import argparse
import contextlib
import copy
import hashlib
import importlib.machinery
import importlib.util
import json
import os
import re
import sys
import time
from collections.abc import Callable, Iterator, Mapping, Sequence
from pathlib import Path
from typing import Literal, NamedTuple

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "tools/chat"))
sys.path.append(str(Path.home() / ".codex/skills/chat/scripts"))

import chatlib  # noqa: E402  # sys.path is set up above
import codekarl_chat  # noqa: E402  # sys.path is set up above
import identities  # noqa: E402  # sys.path is set up above
from codekarl_chat import EffectRow, Inbox  # noqa: E402  # sys.path is set up above

Agent = dict[str, object]  # a registry record (identities.read_registry()["agents"][name])

EMPTY_INVENTORY = "empty cmux process inventory"
RETRY_DELAYS = (0.5, 1.0)
NOT_SENT = (
    "ck-chat: NOT SENT: identity discovery failed before sending (empty cmux process inventory); "
    "nothing was posted, acknowledged or queued; safe to retry"
)
UNCERTAIN = (
    "Uncertain stays uncertain: the transport gives no receipt that this was neither sent nor "
    "queued, so ck-chat never resends it. The task claim is durable and the task proceeds; "
    "mention a possibly missing acknowledgement in your next message."
)
STARTED = time.strftime("%Y-%m-%dT%H:%M:%S")
INCOMPLETE = {"pending", "attempting", "not-sent", "uncertain"}


class DiscoveryFailed(Exception):
    """Identity discovery kept failing before anything was sent, acknowledged or queued."""


def resolve_actor(cfg: Mapping[str, object]) -> Agent | None:
    """identities.current(cfg), retrying only a transient empty cmux inventory (3 tries)."""
    for delay in (*RETRY_DELAYS, None):
        try:
            return identities.current(cfg)
        except chatlib.ChatError as error:
            if not str(error).startswith(EMPTY_INVENTORY):
                raise
            if delay is None:
                raise DiscoveryFailed from error
        time.sleep(delay)
    return None


def bound_name(actor: Agent | None) -> str | None:
    """The actor's name when it is an active bound Codekarl agent, else None."""
    if actor and actor.get("project") == "codekarl" and actor.get("status") == "active":
        name = actor.get("name")
        if not isinstance(name, str):
            raise ValueError("The bound agent record has no name; inspect it with: ck-chat status")
        return name
    return None


@contextlib.contextmanager
def bound_as(name: str | None) -> Iterator[None]:
    """Run an in-process pchat call as an already-resolved agent, so it skips a second discovery.

    CHAT_AGENT_ID selects identities.current's registry branch, which still checks surface
    conflicts. The previous value is restored afterwards, including when it was absent."""
    if not name:
        yield
        return
    previous = os.environ.get("CHAT_AGENT_ID")
    os.environ["CHAT_AGENT_ID"] = name
    try:
        yield
    finally:
        if previous is None:
            os.environ.pop("CHAT_AGENT_ID", None)
        else:
            os.environ["CHAT_AGENT_ID"] = previous


@contextlib.contextmanager
def probes() -> Iterator[dict[str, bool]]:
    """Record whether the shared chatlib reached the network or the outbox during one call.

    login is the first network step of chatlib.post and chatlib.ack; outbox_append is the only
    queueing step. The originals are restored on every exit."""
    entered = {"login": False, "outbox_append": False}
    originals: dict[str, Callable[..., object]] = {name: getattr(chatlib, name) for name in entered}

    def probe(name: str) -> Callable[..., object]:
        def wrapped(*args: object, **kwargs: object) -> object:
            entered[name] = True
            return originals[name](*args, **kwargs)

        return wrapped

    for name in entered:
        setattr(chatlib, name, probe(name))
    try:
        yield entered
    finally:
        for name, original in originals.items():
            setattr(chatlib, name, original)


def classified_call(name: str | None, arguments: Sequence[str]) -> tuple[str, str]:
    """One pchat call as (state, detail): sent, queued, not-sent (proven pre-send) or uncertain."""
    error: BaseException | None = None
    code: int | None = None
    with probes() as entered:
        try:
            with bound_as(name):
                code = pchat(arguments)
        except (Exception, SystemExit) as caught:
            error = caught
    reached = entered["login"] or entered["outbox_append"]
    if error is not None:
        detail = f"{type(error).__name__}: {error}"[:200]
        return ("uncertain" if reached else "not-sent"), detail
    detail = f"pchat exit {code}"
    if code == 0:
        return "sent", detail
    if code == 3:  # noqa: PLR2004  # exit code 3 means queued
        return "queued", detail
    return ("not-sent" if code in (2, 4) and not reached else "uncertain"), detail


def effect_arguments(name: str, mid: str, effect: str) -> list[str]:
    if effect == "reaction":
        return ["ack", "#codekarl", mid]
    return ["post", "#codekarl", "--reply-to", mid, f"[ack] claimed by {name}"]


def attempt(state: Inbox, name: str, mid: str, effect: str) -> str | None:
    """Acquire one effect, make its single call, store the result. None when not acquired."""
    token = state.acquire(mid, name, effect, f"pid={os.getpid()} started={STARTED}")
    if not token:
        return None
    result, detail = classified_call(name, effect_arguments(name, mid, effect))
    state.record_effect(mid, name, effect, token, result, detail)
    return result


def run_effects(state: Inbox, name: str, mid: str) -> int:
    """Attempt each eligible effect in order, independently; returns how many were attempted."""
    attempted = 0
    for effect in codekarl_chat.EFFECTS:
        row = state.effects(mid, name).get(effect)
        if row and row["state"] in codekarl_chat.RETRYABLE:
            attempted += attempt(state, name, mid, effect) is not None
    return attempted


def effect_states(state: Inbox, name: str, mid: str) -> dict[str, str | None]:
    """What claim reports: the stored reaction and visible states; 'not recorded' if none."""
    rows = state.effects(mid, name)
    if not rows:
        return {"reaction": "not recorded", "visible": None}
    return {
        effect: rows[effect]["state"] if effect in rows else None
        for effect in ("reaction", "visible")
    }


def aggregate(states: Mapping[str, str | None]) -> int:
    values = [value for value in states.values() if value is not None]
    if all(value == "sent" for value in values):
        return 0
    if all(value in {"sent", "queued"} for value in values):
        return 3
    return 6


def incomplete_notice(mid: str, states: Mapping[str, str | None]) -> str:
    named = ", ".join(
        f"{effect}={value}" for effect, value in states.items() if value in INCOMPLETE
    )
    return f"ck-chat: acknowledgement not complete for {mid}: {named}; inspect with: ck-chat effects {mid}"


def pid_alive(holder: str | None) -> bool:
    match = re.search(r"pid=(\d+)", holder or "")
    if not match:
        return False
    try:
        os.kill(int(match[1]), 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def describe(row: EffectRow) -> dict[str, object]:
    info: dict[str, object] = {
        "state": row["state"],
        "detail": row["detail"],
        "at": time.strftime("%Y-%m-%dT%H:%M:%S", time.localtime(row["at"])),
    }
    if row["state"] == "attempting":
        info |= {"holder": row["holder"], "alive": pid_alive(row["holder"])}
    if row["state"] == "uncertain":
        info |= {
            "notice": UNCERTAIN,
            "inspect": [
                "the #codekarl room history, for a visible line (pchat read #codekarl)",
                f"{chatlib.STATE_DIR / 'acks.jsonl'}, for a reaction",
                f"the shared outbox and outbox.claimed-* files in {chatlib.STATE_DIR}",
            ],
        }
    return info


def effects_command(state: Inbox, name: str, mid: str, retry: bool) -> int:
    if not any(row["message"] == mid for row in state.rows(name, history=True)):
        raise ValueError("Message is not addressed to this identity")
    attempted = run_effects(state, name, mid) if retry else 0
    rows = state.effects(mid, name)
    shown = (
        {effect: describe(row) for effect, row in rows.items()}
        if rows
        else {effect: {"state": "not recorded"} for effect in codekarl_chat.EFFECTS}
    )
    print(json.dumps({"message": mid, "recipient": name, "effects": shown}, indent=2))
    if not attempted:
        return 0
    states: dict[str, str | None] = {effect: row["state"] for effect, row in rows.items()}
    code = aggregate(states)
    if code == 6:  # noqa: PLR2004  # exit code 6 means the ack effect is incomplete
        print(incomplete_notice(mid, states), file=sys.stderr)
    return code


class DeliveryFailure(Exception):
    """A delivery command that must end with a specific exit code and message."""

    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code


def canonical(value: object) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def codekarl_section(config: Mapping[str, object]) -> Mapping[str, object]:
    """`projects.codekarl` of the configuration; empty when it is absent or not an object."""
    projects = config.get("projects", {})
    section = projects.get("codekarl", {}) if isinstance(projects, dict) else {}
    return section if isinstance(section, dict) else {}


def other_keys(config: Mapping[str, object]) -> str:
    """The config with delivery_enabled removed from the Codekarl project, in canonical JSON."""
    rest = copy.deepcopy(dict(config))
    section = codekarl_section(rest)
    if isinstance(section, dict):
        section.pop("delivery_enabled", None)
    return canonical(rest)


def delivery_value(config: Mapping[str, object]) -> bool:
    return bool(codekarl_section(config).get("delivery_enabled", False))


def config_identity() -> tuple[int, int, int]:
    """What replacing or editing the config file changes: inode, mtime and size."""
    info = Path(chatlib.CONFIG_PATH).stat()
    return info.st_ino, info.st_mtime_ns, info.st_size


def delivery_status(config: Mapping[str, object]) -> dict[str, object]:
    info = Path(chatlib.CONFIG_PATH).stat()
    return {
        "configured": delivery_value(config),
        "config_mtime": time.strftime("%Y-%m-%dT%H:%M:%S", time.localtime(info.st_mtime)),
        "bridge_effective": "unknown",
    }


STATUS_NOTE = (
    "note: the bridge applies the configured value when it next receives an IRC line after the "
    "config change (the change's own record line normally causes that); it does not report it, "
    "so this is the configured value, not proof of effect"
)


def check_reason(reason: str | None) -> str:
    if reason is None or not reason.strip():
        raise ValueError("delivery on/off requires a non-empty --reason")
    if len(reason) > 200:  # noqa: PLR2004  # maximum length of a delivery reason
        raise ValueError("delivery --reason is at most 200 characters")
    if "\n" in reason or "\r" in reason:
        raise ValueError("delivery --reason must be a single line")
    if "@" in reason:
        raise ValueError("delivery --reason must not contain '@' (the record must address no one)")
    return reason.strip()


def delivery_authority(cfg: Mapping[str, object], actor: Agent | None) -> tuple[str, str | None]:
    """Who is recording: (label, bound name or None). Refuses everyone but the manager and owners."""
    if actor:
        if bound_name(actor) != "ck-manager":
            raise ValueError("Only the bound ck-manager or an owner shell may change delivery")
        return "ck-manager", "ck-manager"
    account = os.environ.get("CHAT_AS")
    owners = codekarl_chat.settings(cfg).get("owners", [])
    if account and isinstance(owners, list) and account in owners:
        return account, None
    raise ValueError(
        "Only the bound ck-manager or an owner shell (CHAT_AS an owner account) may change delivery"
    )


Written = tuple[tuple[int, int, int], str]
"""A write's identity: the config file's inode, mtime and size, and its content digest."""


class Verified(NamedTuple):
    """What re-reading the config after a write proved."""

    outcome: Literal["ok", "unchanged", "unreadable", "uncertain"]
    written: Written | None = None
    value: bool | None = None


def digest_of(config: Mapping[str, object]) -> str:
    return hashlib.sha256(canonical(config).encode()).hexdigest()


def with_delivery(config: Mapping[str, object], value: bool) -> dict[str, object]:
    """A copy of the config whose Codekarl project has delivery_enabled set to value."""
    updated = copy.deepcopy(dict(config))
    section = codekarl_section(updated)
    if not isinstance(section, dict):  # the Codekarl project was verified to exist by the caller
        raise ValueError("Codekarl chat is not configured; run the accepted chat installer")
    section["delivery_enabled"] = value
    return updated


def write_and_verify(config: Mapping[str, object], value: bool) -> Verified:
    """Write only delivery_enabled and prove the result by re-reading.

    Returns "ok" when the re-read proves the requested value with every other key unchanged,
    "unchanged" when the file provably still holds the loaded content, "unreadable", or
    "uncertain" with the value read. `atomic_json` can raise after its replace, so a write error
    is judged only from the file."""
    before = config_identity(), digest_of(config)
    updated = with_delivery(config, value)
    failed = False
    try:
        identities.atomic_json(chatlib.CONFIG_PATH, updated)
    except Exception:
        failed = True
    try:
        reread = chatlib.load_config()
        identity = config_identity()
    except (OSError, ValueError, chatlib.ChatError):
        return Verified("unreadable")
    if delivery_value(reread) == value and other_keys(reread) == other_keys(config):
        return Verified("ok", (identity, digest_of(reread)))
    if failed:
        same_file = (identity, digest_of(reread)) == before
        if delivery_value(reread) == delivery_value(config) and same_file:
            return Verified("unchanged")
    return Verified("uncertain", None, delivery_value(reread))


def delivery_change(
    cfg: Mapping[str, object],
    actor: Agent | None,
    value: bool,
    reason: str | None,
) -> int:
    label, name = delivery_authority(cfg, actor)
    reason = check_reason(reason)
    word = "on" if value else "off"
    with identities.locked():
        config = chatlib.load_config()
        if codekarl_section(config).get("routing") != codekarl_chat.POLICY:
            raise ValueError("Codekarl chat is not configured; run the accepted chat installer")
        previous = delivery_value(config)
        verified = None if previous == value else write_and_verify(config, value)
    if verified is None:
        print(json.dumps(delivery_status(config)))
        return 0
    if verified.outcome == "unreadable":
        raise DeliveryFailure(6, "delivery state unknown: config unreadable after a write error")
    if verified.outcome == "unchanged":
        raise ValueError("delivery NOT changed: the config write failed")
    if verified.outcome == "uncertain":
        raise DeliveryFailure(
            6,
            f"delivery state uncertain: config verification failed (value read: {verified.value})",
        )
    if verified.written is None:  # an "ok" outcome always carries the write's identity
        raise DeliveryFailure(6, "delivery state unknown: the write was not identified")
    # The record is posted with no lock held: pchat takes the identity lock itself.
    state, _detail = classified_call(
        name, ["post", "#codekarl", f"[status] delivery {word} by {label}: {reason}"]
    )
    if state in ("sent", "queued"):
        print(json.dumps(delivery_status(chatlib.load_config())))
        return 0 if state == "sent" else 3
    if state == "uncertain":
        raise DeliveryFailure(6, "record fate unknown; check the room (the change is kept)")
    return delivery_rollback(previous, value, verified.written)


def delivery_rollback(previous: bool, value: bool, written: Written) -> int:
    """Undo this command's change after its record was provably not posted, if nothing else wrote."""
    with identities.locked():
        try:
            current = chatlib.load_config()
            identity = config_identity()
        except (OSError, ValueError, chatlib.ChatError) as error:
            raise DeliveryFailure(
                6, f"delivery changed by this command but not recorded; rollback failed ({error})"
            ) from error
        if (identity, digest_of(current)) != written:
            raise DeliveryFailure(
                6,
                "delivery changed by this command but not recorded, and the config changed "
                f"since; current value {delivery_value(current)}; not rolled back",
            )
        outcome = write_and_verify(current, previous).outcome
        if outcome == "ok":
            raise ValueError("delivery NOT changed: record could not be posted")
        now: bool | str
        try:
            now = delivery_value(chatlib.load_config())
        except (OSError, ValueError, chatlib.ChatError):
            now = "unknown"
        raise DeliveryFailure(
            6,
            f"delivery changed by this command but not recorded; rollback failed; "
            f"current value {now}",
        )


def pchat(arguments: Sequence[str]) -> int:
    path = Path.home() / ".codex/skills/chat/scripts/pchat"
    loader = importlib.machinery.SourceFileLoader("ck_pchat", str(path))
    spec = importlib.util.spec_from_loader("ck_pchat", loader)
    if spec is None:
        raise ValueError(f"cannot load pchat from {path}; reinstall the shared chat skill")
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    code: object = module.main(arguments) or 0
    if not isinstance(code, int):
        raise ValueError(f"pchat returned {code!r}, not an exit code; inspect {path}")
    return code


def run() -> int:  # noqa: PLR0911,PLR0912,PLR0915  # PLR0911: one exit code per command outcome; PLR0912: linear workflow kept whole so failure ordering stays visible; PLR0915: linear workflow kept whole so failure ordering stays visible
    parser = argparse.ArgumentParser(prog="ck-chat", description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    inbox = sub.add_parser("inbox")
    inbox.add_argument("--all", action="store_true", help="show every Codekarl recipient")
    inbox.add_argument("--history", action="store_true")
    sub.add_parser("status")
    claim = sub.add_parser("claim")
    claim.add_argument("message")
    effects = sub.add_parser(
        "effects", help="show (and with --retry, redo) acknowledgement effects"
    )
    effects.add_argument("message")
    effects.add_argument(
        "--retry", action="store_true", help="re-attempt pending and not-sent effects"
    )
    for verb in ("done", "decline"):
        finish = sub.add_parser(verb)
        finish.add_argument("message")
        finish.add_argument("text")
    delivery = sub.add_parser(
        "delivery", help="show or change whether the bridge delivers Codekarl notices"
    )
    delivery.add_argument("action", choices=("status", "on", "off"))
    delivery.add_argument("--reason", help="required for on/off; one line, at most 200 characters")
    post = sub.add_parser("post")
    post.add_argument("arguments", nargs=argparse.REMAINDER)
    args = (
        argparse.Namespace(command="post", arguments=sys.argv[2:])
        if sys.argv[1:2] == ["post"]
        else parser.parse_args()
    )
    cfg = chatlib.load_config()
    if not codekarl_chat.settings(cfg):
        raise ValueError("Codekarl chat is not configured; run the accepted chat installer")
    if args.command == "delivery":
        if args.action == "status":
            print(json.dumps(delivery_status(cfg)))
            print(STATUS_NOTE, file=sys.stderr)
            return 0
        return delivery_change(cfg, resolve_actor(cfg), args.action == "on", args.reason)
    state = Inbox(chatlib.STATE_DIR)
    if args.command == "status":
        print(
            json.dumps(
                {
                    "delivery_enabled": codekarl_chat.settings(cfg).get("delivery_enabled", False),
                    "inbox": state.rows(),
                    "routing_diagnostics": state.diagnostics(),
                },
                indent=2,
            )
        )
        return 0
    if args.command == "inbox" and args.all:
        print(json.dumps(state.rows(history=args.history), indent=2))
        return 0
    actor = resolve_actor(cfg)
    if args.command == "post":
        if actor and actor.get("project") != "codekarl":
            raise ValueError("A bound agent from another project cannot use ck-chat to post")
        with bound_as(bound_name(actor)):
            return pchat(["post", "#codekarl", *args.arguments])
    name = bound_name(actor)
    if not name:
        raise ValueError("Use a bound Codekarl agent session for this command")
    if args.command == "inbox":
        print(json.dumps(state.rows(name, args.history), indent=2))
        return 0
    if args.command == "effects":
        return effects_command(state, name, args.message, args.retry)
    if args.command == "claim":
        result = state.claim(args.message, name)
        if not result["fresh"]:
            states = effect_states(state, name, args.message)
            print(json.dumps({"claim": result, "effects": states}))
            if any(value in INCOMPLETE for value in states.values()):
                print(
                    f"ck-chat: acknowledgement not complete; inspect with: ck-chat effects {args.message}",
                    file=sys.stderr,
                )
            return 4
        run_effects(state, name, args.message)
        states = effect_states(state, name, args.message)
        print(json.dumps({"claim": result, "effects": states}))
        code = aggregate(states)
        if code == 6:  # noqa: PLR2004  # exit code 6 means the ack effect is incomplete
            print(incomplete_notice(args.message, states), file=sys.stderr)
        return code
    if not any(row["message"] == args.message for row in state.rows(name, history=True)):
        raise ValueError("Message is not addressed to this identity")
    kind = "done" if args.command == "done" else "declined"
    with bound_as(name):
        return pchat(["post", "#codekarl", "--reply-to", args.message, f"[{kind}] {args.text}"])


def main() -> int:
    try:
        return run()
    except DiscoveryFailed:
        print(NOT_SENT, file=sys.stderr)
        return 5
    except DeliveryFailure as failure:
        print(f"ck-chat: {failure}", file=sys.stderr)
        return failure.code


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, chatlib.ChatError) as error:
        print(f"ck-chat: {error}", file=sys.stderr)
        sys.exit(2)
