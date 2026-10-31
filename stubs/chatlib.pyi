from pathlib import Path

CONFIG_PATH: Path
STATE_DIR: Path
LOG_DIR: Path
OUTBOX: Path

class ChatError(RuntimeError): ...

class Connection:
    caps: set[str]

def load_config() -> dict[str, object]: ...
def channel_log(channel: str) -> Path: ...
def login(
    account: str, cfg: dict[str, object] | None = None, caps: tuple[str, ...] = ()
) -> Connection: ...
def post(
    channel: str,
    text: str,
    account: str,
    cfg: dict[str, object] | None = None,
    cont: str = "",
    reply_to: str | None = None,
) -> int: ...
def ack(channel: str, msgid: str, account: str, cfg: dict[str, object] | None = None) -> None: ...
def outbox_append(entries: list[dict[str, object]]) -> None: ...
def outbox_claim() -> list[Path]: ...
