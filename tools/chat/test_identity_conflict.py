"""Generic live-singleton conflict regressions for the staged shared suite."""

import json
import os
import sys
import tempfile
import unittest
from collections.abc import Mapping
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tests"))
import _isolation

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import chatlib
import identities as ids

_isolation.check_bound(chatlib, ids)


class IdentityConflictTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.cfg = {
            "owner": "pat",
            "assistants": ["sam"],
            "agent_identities_enabled": True,
            "identity_aliases": {"codex": "sam", "claude": "sam"},
            "accounts": {"pat": "x", "sam": "x", "beta-guide": "x"},
            "identities": {"beta-guide": {"role": "guide", "project": "beta"}},
            "projects": {
                "alpha": {
                    "prefix": "alpha-",
                    "channel": "#alpha",
                    "manager": "alpha-manager",
                    "repo": "example/alpha",
                },
                "beta": {
                    "prefix": "beta-",
                    "channel": "#beta",
                    "manager": "beta-manager",
                    "repo": "example/beta",
                },
            },
        }
        self.config = self.root / "config.json"
        self.config.write_text(json.dumps(self.cfg))
        self.state = self.root / "state"
        self.enterContext(mock.patch.object(chatlib, "CONFIG_PATH", self.config))
        self.enterContext(mock.patch.object(chatlib, "STATE_DIR", self.state))
        self.enterContext(mock.patch.object(chatlib, "load_config", return_value=self.cfg))
        self.enterContext(mock.patch.object(chatlib, "login", return_value=mock.Mock()))
        self.enterContext(mock.patch.object(ids, "inventory", return_value=[]))
        self.enterContext(mock.patch.object(ids, "label_tabs"))  # tested in test_identities
        self.enterContext(
            mock.patch.object(
                ids, "process_start", side_effect=lambda pid: {101: "live"}.get(pid, "")
            )
        )
        self.enterContext(mock.patch.dict(os.environ, {}, clear=True))
        self.registry(
            {
                "beta-guide": {
                    "name": "beta-guide",
                    "project": "beta",
                    "role": "guide",
                    "keys": ["codex:incumbent"],
                    "status": "active",
                    "pid": 101,
                    "pid_start": "live",
                    "surface_id": "incumbent-surface",
                }
            }
        )

    def registry(self, agents: Mapping[str, object]) -> None:
        self.state.mkdir(parents=True, exist_ok=True)
        ids.atomic_json(ids.registry_path(), {"version": 1, "counters": {}, "agents": agents})

    def conflict_row(self, surface: str = "conflict-surface", pid: int = 202) -> dict[str, object]:
        return {
            "project": "beta",
            "role": "guide",
            "key": "codex:conflicting-session",
            "session_id": "conflicting-session",
            "surface_id": surface,
            "workspace_id": "workspace",
            "location": "header",
            "brand": "codex",
            "pid": pid,
            "pid_start": "candidate",
            "lifecycle": "running",
        }

    def healthy_row(
        self, session: str, surface: str, project: str = "alpha", pid: int = 303
    ) -> dict[str, object]:
        return {
            "project": project,
            "role": "solver",
            "key": f"codex:{session}",
            "session_id": session,
            "surface_id": surface,
            "workspace_id": "workspace",
            "location": "solver",
            "brand": "codex",
            "pid": pid,
            "pid_start": f"start-{pid}",
            "lifecycle": "running",
        }

    def test_discovery_skips_conflict_and_keeps_healthy_rows_and_attribution_aligned(self) -> None:
        agents = ids.read_registry()["agents"]
        agents.update(
            {
                "alpha-solver-9": {
                    "name": "alpha-solver-9",
                    "project": "alpha",
                    "role": "solver",
                    "keys": ["codex:after"],
                    "status": "active",
                    "pid": 404,
                    "pid_start": "start-404",
                    "surface_id": "after-surface",
                },
                "alpha-solver-8": {
                    "name": "alpha-solver-8",
                    "project": "alpha",
                    "role": "solver",
                    "keys": ["process:404:start-404"],
                    "status": "active",
                    "pid": 404,
                    "pid_start": "start-404",
                },
            }
        )
        self.registry(agents)
        rows = [
            self.healthy_row("before", "before-surface", pid=303),
            self.conflict_row(),
            self.healthy_row("after", "after-surface", pid=404),
        ]
        with mock.patch.object(ids, "inventory", return_value=rows):
            records = ids.sync(self.cfg)
        self.assertEqual([r["name"] for r in records], ["alpha-solver-1", "alpha-solver-9"])
        agents = ids.read_registry()["agents"]
        self.assertEqual(agents["beta-guide"]["keys"], ["codex:incumbent"])
        self.assertEqual(agents["beta-guide"]["status"], "active")
        self.assertEqual(agents["alpha-solver-9"]["session_id"], "after")
        self.assertEqual(agents["alpha-solver-8"]["lifecycle"], "duplicate of alpha-solver-9")
        diagnostic = self.state / "identity-conflicts.json"
        self.assertTrue(diagnostic.exists())
        self.assertEqual(diagnostic.stat().st_mode & 0o777, 0o600)
        self.assertNotIn("accounts", diagnostic.read_text())

    def test_live_incumbent_absent_from_inventory_is_preserved(self) -> None:
        with mock.patch.object(ids, "inventory", return_value=[self.conflict_row()]):
            self.assertEqual(ids.sync(self.cfg), [])
        self.assertEqual(ids.read_registry()["agents"]["beta-guide"]["status"], "active")

    def test_conflicting_surface_fails_closed_even_for_explicit_account(self) -> None:
        with (
            mock.patch.object(ids, "inventory", return_value=[self.conflict_row()]),
            mock.patch.dict(os.environ, {"CMUX_SURFACE_ID": "conflict-surface"}),
        ):
            with self.assertRaises(ids.IdentityConflict):
                ids.resolve(self.cfg)
            with self.assertRaises(ids.IdentityConflict):
                ids.resolve(self.cfg, "pat")

    def test_healthy_foreign_project_resolves_on_first_call(self) -> None:
        rows = [
            self.conflict_row(),
            self.healthy_row("alpha-session", "alpha-surface"),
        ]
        with (
            mock.patch.object(ids, "inventory", return_value=rows),
            mock.patch.dict(os.environ, {"CMUX_SURFACE_ID": "alpha-surface"}),
        ):
            self.assertEqual(ids.resolve(self.cfg), "alpha-solver-1")
        agents = ids.read_registry()["agents"]
        self.assertEqual(agents["beta-guide"]["status"], "active")
        self.assertEqual(agents["alpha-solver-1"]["project"], "alpha")
        self.assertTrue((self.state / "identity-conflicts.json").exists())

    def test_explicit_registration_rejects_live_ownership_conflict(self) -> None:
        with self.assertRaises(ids.IdentityConflict):
            ids.register(
                project="beta",
                role="guide",
                key="codex:conflicting-session",
                surface_id="conflict-surface",
                pid=202,
                pid_start="candidate",
            )
        self.assertEqual(ids.read_registry()["agents"]["beta-guide"]["keys"], ["codex:incumbent"])

    def test_repeated_discovery_does_not_bind_candidate_or_churn_diagnostic(self) -> None:
        with mock.patch.object(ids, "inventory", return_value=[self.conflict_row()]):
            ids.sync(self.cfg)
            diagnostic = self.state / "identity-conflicts.json"
            first = diagnostic.read_bytes()
            ids.sync(self.cfg)
            self.assertEqual(diagnostic.read_bytes(), first)
        agent = ids.read_registry()["agents"]["beta-guide"]
        self.assertEqual(agent["keys"], ["codex:incumbent"])
        self.assertNotIn("alpha-solver-1", ids.read_registry()["agents"])

    def test_conflict_recovers_through_ordinary_resolve_and_clears_diagnostic(self) -> None:
        with mock.patch.object(ids, "inventory", return_value=[self.conflict_row()]):
            ids.sync(self.cfg)
        diagnostic = self.state / "identity-conflicts.json"
        self.assertTrue(diagnostic.exists())
        with (
            mock.patch.object(ids, "process_start", return_value=""),
            mock.patch.object(ids, "inventory", return_value=[self.conflict_row()]),
            mock.patch.dict(os.environ, {"CMUX_SURFACE_ID": "conflict-surface"}),
        ):
            self.assertEqual(ids.resolve(self.cfg), "beta-guide")
        self.assertFalse(diagnostic.exists())

    def test_discover_false_keeps_known_conflict_fail_closed(self) -> None:
        with mock.patch.object(ids, "inventory", return_value=[self.conflict_row()]):
            ids.sync(self.cfg)
        with (
            mock.patch.object(ids, "inventory", side_effect=AssertionError("unexpected discovery")),
            mock.patch.dict(os.environ, {"CMUX_SURFACE_ID": "conflict-surface"}),
            self.assertRaises(ids.IdentityConflict),
        ):
            ids.resolve(self.cfg, "pat", discover=False)

    def test_unrelated_registration_errors_still_propagate(self) -> None:
        row = self.healthy_row("one", "one-surface")
        with mock.patch.object(ids, "inventory", return_value=[row]):
            with (
                mock.patch.object(ids, "register", side_effect=OSError("provisioning failed")),
                self.assertRaisesRegex(OSError, "provisioning failed"),
            ):
                ids.sync(self.cfg)
            with (
                mock.patch.object(
                    ids, "register", side_effect=chatlib.ChatError("account refused")
                ),
                self.assertRaisesRegex(chatlib.ChatError, "account refused"),
            ):
                ids.sync(self.cfg)


if __name__ == "__main__":
    unittest.main()
