from pathlib import Path
from types import ModuleType

CHAT_STATE: Path

def check_bound(*modules: ModuleType) -> None: ...
