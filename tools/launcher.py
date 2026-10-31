"""The checkpoint half of the `tools/ck` trampoline: run the selected checkpoint's own controller.

`tools/ck` (in the caller's copy) verifies the accepted checkpoint and then starts a fresh interpreter
with `tools` set to the checkpoint's `tools` directory. That interpreter imports this module from
there, so the code that runs is the checkpoint's, never the caller's. It does nothing before
`cli.main(root, hub)` (or the legacy `pipeline.main`) beyond environment setup and the signing agent
(plan 10.4, "what S12 and later changes must keep"), and it origin-checks every controller module it
imports exactly as the inline launcher string it replaces did.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path
from types import ModuleType

import environment
import pipeline

ORIGIN_ERROR = "controller origin check failed"


def inside(module: ModuleType, tools: str) -> bool:
    """True when the module was loaded from the checkpoint's `tools` directory."""
    origin = getattr(module, "__file__", None)
    return origin is not None and os.path.realpath(origin).startswith(
        os.path.realpath(tools) + os.sep
    )


def run(tools: str, root: str, args: list[str]) -> None:
    """Run one `ck` command for the copy at `root` with the controller found in `tools`."""
    this = sys.modules[__name__]
    for module in (this, environment, pipeline):
        if not inside(module, tools):
            raise RuntimeError(ORIGIN_ERROR)
    sys.argv = ["ck", *args]
    copy = Path(root)
    hub = environment.configure(copy)
    try:
        with environment.signing(hub):
            if (hub / ".pipeline/accepted-v2.json").exists():
                from gate import cli  # noqa: PLC0415  # only an activated hub loads the gate

                if not inside(cli, tools):
                    raise RuntimeError(ORIGIN_ERROR)
                cli.main(copy, hub)
            else:
                pipeline.main(copy, hub)
    except (RuntimeError, KeyboardInterrupt) as error:
        print(
            f"ck: {error or 'interrupted; accepted checkpoint unchanged unless published'}",
            file=sys.stderr,
        )
        sys.exit(1)
