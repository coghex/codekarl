"""Local Pijul batch acceptance. No server, mutable main branch, or polling loop."""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
import uuid
from pathlib import Path

from environment import lock


def identifier():
    return f"{time.time_ns()}-{uuid.uuid4().hex[:8]}"


def read(path):
    return json.loads(path.read_text())


def atomic(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}-{uuid.uuid4().hex}")
    with temporary.open("x") as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    directory = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def pijul(repo, *arguments):
    result = subprocess.run(
        ["pijul", *map(str, arguments)], cwd=repo, text=True, capture_output=True
    )
    if result.returncode:
        raise RuntimeError(f"pijul {' '.join(map(str, arguments))}: {result.stderr.strip()}")
    return result.stdout


def hashes(repo):
    return set(pijul(repo, "log", "--hash-only").splitlines())


def clean(repo):
    status = json.loads(pijul(repo, "status", "--output-format", "json"))
    if status["changes"] or status["untracked"]:
        raise RuntimeError(f"Record or remove pending changes in {repo}: {status}")


def files(repo):
    result = {}
    for name in pijul(repo, "list").splitlines():
        path = repo / name
        if path.is_symlink():
            raise RuntimeError(f"Symlinks are not supported in accepted source yet: {name}")
        if path.is_dir():
            continue
        result[name] = {
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "executable": bool(path.stat().st_mode & 0o111),
        }
    return result


def fingerprint(repo):
    return {"hashes": sorted(hashes(repo)), "files": files(repo)}


def clone(source, destination, hub):
    destination.parent.mkdir(parents=True, exist_ok=True)
    pijul(source, "clone", source, destination)
    (destination / ".ck-hub").write_text(str(hub) + "\n")


def pull(repo, source, changes):
    if not changes:
        return []
    result = pijul(repo, "pull", "--json", "--no-notify", "--complete", source, "--", *changes)
    return json.loads(result)["conflicts"]


def doc_path(name):
    path = Path(name)
    return path.name not in {"AGENTS.md", "CLAUDE.md", "SKILL.md"} and (
        name == "README.md"
        or (len(path.parts) > 1 and path.parts[0] == "docs" and path.suffix == ".md")
    )


class Pipeline:
    def __init__(self, hub, validator=None):
        self.hub = hub.resolve()
        self.directory = self.hub / ".pipeline"
        self.pointer = self.directory / "accepted.json"
        self.validator = validator or self.validate
        self.lock_fd = None

    def state(self):
        if not self.pointer.exists():
            raise RuntimeError("No accepted checkpoint. Run tools/ck init first.")
        return read(self.pointer)

    def verified_state(self):
        state = self.state()
        repo = Path(state["repo"])
        clean(repo)
        if fingerprint(repo) != state["fingerprint"]:
            raise RuntimeError(
                "Accepted checkpoint was edited. Restore it from backup; do not sync."
            )
        return state

    def validate(self, repo, log):
        environment = os.environ.copy()
        environment["CK_CI_LOCK_FD"] = str(self.lock_fd)
        environment["CARGO_TARGET_DIR"] = str(self.directory / "target")
        with log.open("w") as stream:
            result = subprocess.run(
                [sys.executable, "tools/check.py"],
                cwd=repo,
                env=environment,
                stdout=stream,
                stderr=subprocess.STDOUT,
                pass_fds=(self.lock_fd,),
            )
        return result.returncode == 0

    def publish(self, candidate, before, submissions, receipt, previous=None):
        # The single publication point. No working-copy reset follows this operation.
        state = {
            "repo": str(candidate),
            "fingerprint": before,
            "submissions": sorted(set((previous or {}).get("submissions", [])) | set(submissions)),
            "receipt": str(receipt),
            "previous": (previous or {}).get("receipt"),
        }
        atomic(self.pointer, state)
        return state

    def run_checks(self, candidate, submissions, previous=None, conflicts=None):
        receipt = candidate.parent / f"{candidate.name}.json"
        log = candidate.parent / f"{candidate.name}.log"
        report = {"submissions": submissions, "repo": str(candidate), "started": time.time()}
        try:
            if conflicts:
                raise RuntimeError(f"Unresolved Pijul conflicts: {conflicts}")
            clean(candidate)
            before = fingerprint(candidate)
            if previous and not set(previous["fingerprint"]["hashes"]) <= set(before["hashes"]):
                raise RuntimeError(
                    "A patch removes accepted history (amend/squash is unsupported)."
                )
            report["fingerprint"] = before
            print(f"Checking {len(submissions)} submission(s): {candidate.name}", flush=True)
            if not self.validator(candidate, log):
                raise RuntimeError(f"Checks failed; see {log}")
            clean(candidate)
            if fingerprint(candidate) != before:
                raise RuntimeError("Validation changed the candidate source or patch set.")
        except RuntimeError as error:
            report.update(passed=False, error=str(error), finished=time.time())
            atomic(receipt, report)
            print(str(error), flush=True)
            return None
        report.update(passed=True, finished=time.time(), kind="ci")
        atomic(receipt, report)
        return self.publish(candidate, before, submissions, receipt, previous)

    def initialize(self, source):
        with lock(self.directory / "ci.lock") as self.lock_fd:
            if self.pointer.exists():
                raise RuntimeError("Already initialized.")
            clean(source)
            candidate = self.directory / "checkpoints" / identifier()
            clone(source, candidate, self.hub)
            if not self.run_checks(candidate, []):
                raise RuntimeError("Bootstrap has not passed; nothing was accepted.")

    def manifests(self):
        return [read(p) for p in sorted((self.directory / "queue").glob("*.json"))]

    def blocked(self):
        return {p.stem for p in (self.directory / "blocked").glob("*.json")}

    def submit(self, source, spec):
        if not spec.strip():
            raise RuntimeError("Describe the acceptance intent and relevant tests in --spec.")
        clean(source)
        base = self.verified_state()
        patch_set = hashes(source) - set(base["fingerprint"]["hashes"])
        if not patch_set:
            raise RuntimeError("No new recorded changes to submit.")
        for manifest in self.manifests():
            if manifest["id"] in self.blocked() and patch_set & set(manifest["hashes"]):
                raise RuntimeError(
                    f"Includes quarantined patches from {manifest['id']}; retry it first."
                )
        submission = identifier()
        capture = self.directory / "submissions" / submission
        before = fingerprint(source)
        clone(source, capture, self.hub)
        clean(capture)
        if fingerprint(capture) != before or fingerprint(source) != before:
            raise RuntimeError("Source changed during submission; retry after recording.")
        manifest = {
            "id": submission,
            "repo": str(capture),
            "hashes": sorted(patch_set),
            "fingerprint": before,
            "spec": spec,
        }
        atomic(self.directory / "queue" / f"{submission}.json", manifest)
        return submission

    @staticmethod
    def groups(manifests):
        # Each submission includes its entire unaccepted closure. Shared patches
        # mean dependency-coupled work, so these submissions may not be bisected apart.
        groups = []
        for manifest in manifests:
            merged = [manifest]
            patches = set(manifest["hashes"])
            remaining = []
            for group in groups:
                if patches & {h for item in group for h in item["hashes"]}:
                    merged.extend(group)
                    patches.update(h for item in group for h in item["hashes"])
                else:
                    remaining.append(group)
            groups = remaining + [merged]
        return groups

    def attempt(self, manifests):
        previous = self.verified_state()
        candidate = self.directory / "checkpoints" / identifier()
        clone(Path(previous["repo"]), candidate, self.hub)
        conflicts = []
        for manifest in manifests:
            source = Path(manifest["repo"])
            clean(source)
            if fingerprint(source) != manifest["fingerprint"]:
                raise RuntimeError(f"Submission capture was edited: {manifest['id']}")
            missing = set(manifest["hashes"]) - hashes(candidate)
            if missing:
                # Each pull reports conflicts for the resulting working copy, including
                # resolutions in this batch. Do not fail on an intermediate conflict.
                conflicts = pull(candidate, source, sorted(missing))
        expected = set(previous["fingerprint"]["hashes"]) | {
            h for manifest in manifests for h in manifest["hashes"]
        }
        if hashes(candidate) != expected:
            raise RuntimeError(
                "Unexpected dependency or removed patch; candidate was not accepted."
            )
        return self.run_checks(candidate, [m["id"] for m in manifests], previous, conflicts)

    def accept(self, split=True):
        with lock(self.directory / "ci.lock") as self.lock_fd:
            accepted = set(self.verified_state()["submissions"])
            blocked = self.blocked()
            # Freeze once. Later submissions remain queued for another invocation.
            inventory = self.manifests()
            blocked_hashes = {h for m in inventory if m["id"] in blocked for h in m["hashes"]}
            frozen = []
            for manifest in inventory:
                if manifest["id"] in accepted | blocked:
                    continue
                if blocked_hashes & set(manifest["hashes"]):
                    # A dependent submission may have arrived during the failing run,
                    # before its prerequisite acquired the quarantine marker.
                    atomic(self.directory / "blocked" / f"{manifest['id']}.json", manifest)
                else:
                    frozen.append(manifest)
            if not frozen:
                print("No ready submissions.")
                return []
            promoted = []

            def process(groups):
                manifests = [m for group in groups for m in group]
                result = self.attempt(manifests)
                if result:
                    promoted.extend(m["id"] for m in manifests)
                elif split and len(groups) > 1:
                    middle = len(groups) // 2
                    process(groups[:middle])
                    # Test this half on the checkpoint just accepted above. Two
                    # separately passing halves are never blindly combined.
                    process(groups[middle:])
                else:
                    for manifest in manifests:
                        atomic(self.directory / "blocked" / f"{manifest['id']}.json", manifest)

            process(self.groups(frozen))
            return promoted

    def worker(self, name):
        if not name or any(c not in "abcdefghijklmnopqrstuvwxyz0123456789-_" for c in name):
            raise RuntimeError("Worker name must contain lowercase letters, digits, '-' or '_'.")
        state = self.verified_state()
        destination = self.directory / "workers" / name
        clone(Path(state["repo"]), destination, self.hub)
        pijul(destination, "hooks", "approve")
        return destination

    def sync(self, root):
        clean(root)
        state = self.verified_state()
        current = hashes(root)
        accepted = set(state["fingerprint"]["hashes"])
        if not current <= accepted:
            raise RuntimeError(
                "This copy has unaccepted patches. Use a fresh worker to avoid conflicts."
            )
        conflicts = pull(root, Path(state["repo"]), sorted(accepted - current))
        if conflicts:
            raise RuntimeError(f"Unexpected sync conflict: {conflicts}")
        if fingerprint(root) != state["fingerprint"]:
            raise RuntimeError("Sync did not reproduce the accepted checkpoint.")

    def docs(self, root, names, message):
        root = root.resolve()
        for name in names:
            path = root / name
            if not doc_path(name) or path.resolve() != path.absolute() or not path.is_file():
                raise RuntimeError(f"Not an ordinary documentation file: {name}")
        with lock(self.directory / "ci.lock"):
            previous = self.verified_state()
            if not set(previous["fingerprint"]["hashes"]) <= hashes(root):
                raise RuntimeError("Sync before publishing docs to avoid overwriting newer edits.")
            tracked = set(pijul(root, "list").splitlines())
            originals = {name: (root / name).read_bytes() for name in names}
            for name in names:
                accepted_file = previous["fingerprint"]["files"].get(name)
                if accepted_file:
                    recorded = pijul(root, "reset", "--dry-run", name).encode()
                    if hashlib.sha256(recorded).hexdigest() != accepted_file["sha256"]:
                        raise RuntimeError(
                            "Publish docs from a copy without unaccepted docs patches."
                        )
                elif name in tracked:
                    raise RuntimeError(
                        "Leave new docs untracked; tools/ck docs records them for you."
                    )
            candidate = self.directory / "checkpoints" / identifier()
            clone(Path(previous["repo"]), candidate, self.hub)
            for name in names:
                target = candidate / name
                source = root / name
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target)
            pijul(candidate, "add", "--recursive", *names)
            # Internal clones have no hook approval: ordinary docs run no CI.
            pijul(candidate, "record", "-am", message, "--author", os.environ.get("USER", "local"))
            clean(candidate)
            before = fingerprint(candidate)
            old_code = {
                k: v for k, v in previous["fingerprint"]["files"].items() if not doc_path(k)
            }
            new_code = {k: v for k, v in before["files"].items() if not doc_path(k)}
            if old_code != new_code:
                raise RuntimeError("Docs publication changed executable source.")
            receipt = candidate.parent / f"{candidate.name}.json"
            atomic(receipt, {"kind": "docs", "passed": True, "inherits": previous["receipt"]})
            published = self.publish(candidate, before, [], receipt, previous)
            # Reconcile just the published docs. Pulling over their identical pending
            # edits otherwise makes Pijul preserve both versions as a conflict.
            if any((root / name).read_bytes() != original for name, original in originals.items()):
                raise RuntimeError(
                    f"Docs published at {candidate}, but local edits changed; left untouched."
                )
            for name in names:
                old = Path(previous["repo"]) / name
                if name in previous["fingerprint"]["files"]:
                    shutil.copyfile(old, root / name)
                else:
                    (root / name).unlink()
            conflicts = pull(root, candidate, sorted(set(before["hashes"]) - hashes(root)))
            if conflicts:
                raise RuntimeError(
                    f"Docs published at {candidate}; local sync conflicts: {conflicts}"
                )
            return published


def main(root, hub):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("init", "status", "sync", "doctor"):
        commands.add_parser(name)
    worker = commands.add_parser("worker")
    worker.add_argument("name")
    submit = commands.add_parser("submit")
    submit.add_argument(
        "--spec", required=True, help="Acceptance intent, including relevant spec tests"
    )
    accept = commands.add_parser("accept")
    accept.add_argument("--no-split", action="store_true")
    retry = commands.add_parser("retry")
    retry.add_argument("id")
    docs = commands.add_parser("docs")
    docs.add_argument("-m", "--message", default="Update documentation")
    docs.add_argument("paths", nargs="+")
    for name in ("pijul", "cargo", "check"):
        commands.add_parser(name).add_argument("arguments", nargs=argparse.REMAINDER)
    if len(sys.argv) > 1 and sys.argv[1] in {"pijul", "cargo", "check"}:
        args = argparse.Namespace(command=sys.argv[1], arguments=sys.argv[2:])
    else:
        args = parser.parse_args()
    pipeline = Pipeline(hub)
    if args.command == "init":
        pipeline.initialize(root)
    elif args.command == "submit":
        print(pipeline.submit(root, args.spec))
    elif args.command == "accept":
        print(f"Accepted: {pipeline.accept(not args.no_split)}")
    elif args.command == "worker":
        print(pipeline.worker(args.name))
    elif args.command == "sync":
        pipeline.sync(root)
    elif args.command == "docs":
        print(pipeline.docs(root, args.paths, args.message)["repo"])
    elif args.command == "retry":
        with lock(pipeline.directory / "ci.lock"):
            path = pipeline.directory / "blocked" / f"{args.id}.json"
            if path.parent != pipeline.directory / "blocked" or not path.exists():
                raise RuntimeError("Unknown quarantined submission.")
            path.unlink()
    elif args.command == "status":
        state = pipeline.verified_state()
        print(f"Accepted checkpoint: {state['repo']}\nReceipt: {state['receipt']}")
        for manifest in pipeline.manifests():
            status = "accepted" if manifest["id"] in state["submissions"] else "queued"
            if manifest["id"] in pipeline.blocked():
                status = "quarantined"
            print(f"{status}: {manifest['id']} — {manifest['spec']}")
    elif args.command == "doctor":
        from check import doctor

        doctor()
    else:
        command = [args.command, *args.arguments]
        if args.command == "check":
            command = [sys.executable, "tools/check.py", *args.arguments]
        result = subprocess.run(command, cwd=root)
        if result.returncode:
            raise RuntimeError(f"{args.command} exited {result.returncode}")
