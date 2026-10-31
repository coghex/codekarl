#!/usr/bin/env python3
"""Reviewed helper for the one-time legacy-to-v2 gate installation."""

from __future__ import annotations

import hashlib
import importlib.util
import os
import sys
from pathlib import Path
from typing import NoReturn


def fail(message: str) -> NoReturn:
    raise SystemExit(f"ck bootstrap: {message}")


def main() -> None:  # noqa: PLR0915  # linear workflow kept whole so failure ordering stays visible
    if not os.environ.get("HOME"):
        fail("HOME must be set")
    if not os.environ.get("PATH"):
        os.environ["PATH"] = "/usr/bin:/bin:/usr/sbin:/sbin"
    capture = Path(__file__).resolve().parents[2]
    helper_path = capture / "tools/gate/bootstrap.py"
    if Path(__file__).resolve() != helper_path:
        fail("helper is outside its immutable submission capture")
    try:
        index = sys.argv.index("--hub")
        hub = Path(sys.argv[index + 1]).resolve()
        del sys.argv[index : index + 2]
    except (ValueError, IndexError):
        fail("usage: bootstrap.py --hub HUB COMMAND")
    marker = capture / ".ck-hub"
    if not marker.exists() or Path(marker.read_text().strip()).resolve() != hub:
        fail("--hub differs from the capture's .ck-hub")
    if capture.parent != hub / ".pipeline/submissions":
        fail("helper must run from .pipeline/submissions/<ID>/tools/gate/bootstrap.py")
    os.environ["PATH"] = os.pathsep.join(
        ["/opt/homebrew/opt/rustup/bin", str(Path.home() / ".local/bin"), os.environ["PATH"]]
    )
    tools = capture / "tools"
    sys.path.insert(0, str(tools))
    spec = importlib.util.spec_from_file_location(
        "ck_capture_environment", tools / "environment.py"
    )
    if spec is None or spec.loader is None:
        fail("capture environment module is missing")
    environment = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(environment)
    origin = environment.__file__
    if origin is None or not Path(origin).resolve().is_relative_to(tools.resolve()):
        fail("environment module origin check failed")
    configured = environment.configure(capture)
    if configured != hub:
        fail("capture environment resolved a different hub")
    import pipeline as pipeline_module  # noqa: PLC0415  # lazy: pipeline imports gate.cli (import cycle)

    from gate import pipeline_api  # noqa: PLC0415  # the capture's own module, after sys.path

    if not Path(pipeline_module.__file__).resolve().is_relative_to(tools.resolve()):
        fail("pipeline module origin check failed")
    pipeline = pipeline_api.load()
    pipeline.clean(capture)
    submission_id = capture.name
    manifest_path = hub / ".pipeline/queue" / f"{submission_id}.json"
    from gate.pointer import canonical  # noqa: PLC0415  # the capture's own module, after sys.path
    from gate.records import RecordError, load_manifest, read_json  # noqa: PLC0415  # as above

    try:
        manifest = load_manifest(read_json(manifest_path), str(manifest_path))
    except RecordError as error:
        fail(str(error))
    if (
        hashlib.sha256(canonical(manifest["fingerprint"])).hexdigest()
        != hashlib.sha256(canonical(pipeline.fingerprint(capture))).hexdigest()
    ):
        fail("capture fingerprint differs from its manifest")
    from gate.bootstrap_core import execute  # noqa: PLC0415  # command-local import

    with environment.signing(hub):
        execute(hub, capture, sys.argv[1:])


if __name__ == "__main__":
    main()
