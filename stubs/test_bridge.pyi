from collections.abc import Mapping

import chatlib as chatlib

STATE: str
CFG: dict[str, object]

class DeliveriesQueue:
    items: list[dict[str, object]]
    def __init__(self) -> None: ...
    def process(self, cfg: Mapping[str, object]) -> None: ...

class RecordLog:
    def __init__(self) -> None: ...

class AckLog:
    def __init__(self) -> None: ...

class Bridge:
    Deliveries: type[DeliveriesQueue]
    Record: type[RecordLog]
    Acks: type[AckLog]
    BACKOFF: tuple[int, ...]
    def handle(
        self,
        message: dict[str, object],
        cfg: Mapping[str, object],
        record: RecordLog,
        deliveries: DeliveriesQueue,
        acks: AckLog,
    ) -> None: ...
    def handle_tagmsg(
        self,
        message: dict[str, object],
        cfg: Mapping[str, object],
        deliveries: DeliveriesQueue,
        acks: AckLog,
    ) -> None: ...
    def _wake_text(self, item: Mapping[str, object]) -> str: ...

bridge: Bridge

def reset_state() -> None: ...
def privmsg(
    sender: str,
    text: str,
    msgid: str,
    channel: str = "#mosk",
    reply_to: str | None = None,
    t: str = "2026-10-02T00:00:00.000Z",
) -> dict[str, object]: ...
