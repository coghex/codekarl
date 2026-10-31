# Independent rereview verdict: ck-identity-conflict

> **Historical review record (2026-10-14).** The approved repair is accepted and installed. The residual non-blocking
> notes below are kept for reference and are not scheduled tasks. See [the incident record](../chat-identity-incident.md).

Verdict: **APPROVED**. Combined acceptance of both submissions together is
approved: `1791998764113053000-9e90f31d` and `1791999056969754000-87526df1`.
First-capture-only acceptance is **not** approved. On its own, the first capture
still has B1.

The original verdict, `identity-review-verdict.md` (CHANGES_REQUESTED, SHA-256
`faed70d4854333ef6b2258ec586fe1c11408ab7b7c2f53bf252789bc26300973`), is unchanged.
It remains the correct verdict for the first capture on its own.

## Reviewer

- Family and tier: Claude family, Class A, high effort.
- Model: Copilot CLI 1.0.97, `claude-opus-5.5`.
- Session: `9bcdbe98-41c4-4fdf-9a6f-5eb4cc70a523`. This is the same reviewer
  session as `identity-review.md`.
- IRC: I used no IRC identity. I claim no chat identity and impersonate none.
- Author (per the brief and the worker result): `ck-worker-1`, Codex, Class B
  `gpt-6-luna`, high effort.
  - Provider session: `01a13b6e-7c6d-7b41-90e5-8d0c2b967b55`, verified by the
    coordinator against PID 14763.
  - This rereview is opposite-family, as required.

## Exact scope

| Item | Value |
| --- | --- |
| First submission | `1791998764113053000-9e90f31d` |
| First manifest SHA-256 | `f7b5b42ab81714a9ec258002c4583880525a0ff9875baa87c59984ad6d81fd10` (matches) |
| First manifest hashes | `KUZOBK52QKFLOO3LPNLPXJ2PD5VR4O6TAFIIHITXRPSOPUS5DBOQC` |
| Dependent submission | `1791999056969754000-87526df1` |
| Dependent manifest SHA-256 | `722dac5430877b282ea2bb234ca4cf6d7e5b06462a6d87f6758cc77f82188389` (matches) |
| Dependent manifest hashes | `HY4IGZB5DPP3PXQ3VYZQNPE67UGAPODC7JUJCNXJDQ3PVPFVYWBAC`, `KUZOBK52QKFLOO3LPNLPXJ2PD5VR4O6TAFIIHITXRPSOPUS5DBOQC` |
| Base | `checkpoints/1791998061129711000-75d9d0b0` |

Capture checks:

- `pipeline.fingerprint()` matches the manifest fingerprint for both captures,
  both before and after this review.
- `pijul status` reports no changes and no untracked files in either capture.
- Capture logs:
  - First capture: `KUZOBK…`, then `Z2ORXPIW…` (base).
  - Dependent capture: `HY4IGZB5…`, then `KUZOBK…`, then base.
- `HY4IGZB5…` depends on `KUZOBK…` and on the already-accepted `2WF33GF4…`.

The patch diff between the two captures touches only two files:

- In `tools/chat/adapter.py`, the `resolve()` transform is changed.
- `tools/chat/test_identity_conflict.py` is rewritten.

`tools/chat/identity_conflicts.py` and `tools/chat/install.py` are byte-identical
to the first capture. My earlier evidence for them still applies:

- typed `IdentityConflict(ChatError)`;
- an atomic mode-0600 diagnostic file;
- the EXTRA install entries;
- the incumbent is preserved;
- duplicate attribution is aligned with the registering row;
- unrelated errors propagate.

Accepted-pointer note: during this review, `accepted.json` advanced to the
docs-kind checkpoint `1791999123711330000-a0f7cf5e`.

- It inherits `75d9d0b0`.
- It adds `ASDWCI3V…`, "Record the first shared identity incident review", which
  only touches `docs/reviews`.
- Neither identity patch is included.
- There is no overlap with `tools/chat`.

## Validation (disposable clones only)

I used `pijul clone` into `/tmp` with `PYTHONDONTWRITEBYTECODE=1`. Each staged
run used `tools/chat/install.py stage` into a temporary `ck-chat-tested-*`
directory. There was no live installation, no shared-skill write, no service
restart, no full CI and no acceptance.

| Stage | Result |
| --- | --- |
| Combined capture, full staged shared suite | 239 tests, **OK** |
| First capture, full staged shared suite | 237 tests, OK |
| New combined test file run against the first-capture stage | 9 run, **1 error**: `test_conflict_recovers_through_ordinary_resolve_and_clears_diagnostic` raises `IdentityConflict: beta-guide already has a live session`. This is exactly B1. |
| New combined test file run against the base-checkpoint stage | 9 run, **8 errors**. Only `test_unrelated_registration_errors_still_propagate` passes, which is expected because it holds before and after the fix. |
| Ruff 0.14.0 (project-pinned): `check` and `format --check` on `tools/chat` in the combined clone | All checks passed; 6 files already formatted |

Ordinary recovery therefore fails against the first capture and passes against
the combined capture, as the brief requires.

## Blocking findings from the original verdict

### B1: stale diagnostic blocks ordinary `resolve()` recovery — RESOLVED

The transformed `resolve()` no longer checks the diagnostic before discovery
when identities are enabled. Enabled callers now reach `current()`, which calls
`sync()` when `discover` is true, a surface is present and `CHAT_AGENT_ID` is
unset. `sync()` rewrites or unlinks the diagnostic before the
`raise_for_surface()` check.

New regression: `test_conflict_recovers_through_ordinary_resolve_and_clears_diagnostic`.

1. It creates the conflict.
2. It marks the incumbent as dead.
3. Ordinary `resolve()` on the conflicted surface then returns `beta-guide`.
4. It asserts the diagnostic file is gone.

This test fails on the first capture and passes on the combined capture.

`test_discover_false_keeps_known_conflict_fail_closed` covers the intended
asymmetry: with `discover=False`, a known conflict still fails closed even for
an explicit `pat`, and inventory is never called. This matches the
assignment's fail-closed requirement for callers that do not discover.

My probe with an upper-case `CMUX_SURFACE_ID` on the enabled path also failed
closed with the typed conflict.

### B2: invented names — RESOLVED

The fixtures now use:

- projects `alpha` and `beta`;
- accounts `pat` and `sam`;
- incumbent `beta-guide`;
- healthy role `solver`, with names like `alpha-solver-N`.

A case-insensitive grep for `vince|laz|codekarl|synarchy|ck-|syn-|worker` across
`test_identity_conflict.py` and `identity_conflicts.py` returns nothing. The
`healthy_row(role=…)` parameter was dropped, and every healthy row is now
`solver`. The fixtures are suitable for Plateia to carry.

### B3: foreign-project and first-call coverage — RESOLVED

- **Before and between rows:**
  `test_discovery_skips_conflict_and_keeps_healthy_rows_and_attribution_aligned`
  runs the rows `alpha` healthy, then the `beta` conflict, then `alpha` healthy.
  The conflict is in a foreign project, between healthy rows.
  - Healthy registrations continue: `alpha-solver-1` and `alpha-solver-9`.
  - The incumbent `beta-guide` keeps its keys and stays active.
  - Duplicate attribution is correct: `alpha-solver-8` becomes a duplicate of
    `alpha-solver-9`.
  - The diagnostic is mode 0600.
- **First call, conflict as first row:**
  `test_healthy_foreign_project_resolves_on_first_call` puts the conflict row
  first, then a healthy `alpha` row.
  - Resolving from `alpha-surface` returns `alpha-solver-1` on the first call.
  - The incumbent stays active.
  - The diagnostic persists for `beta`.
  - Against base, this test errors, reproducing the incident.

## Residual, non-blocking scope

- **N1 (narrowed, still relevant):** a corrupt `identity-conflicts.json` now
  repairs itself on the ordinary discovering path. `sync()`'s `replace_all`
  rewrites or unlinks the file. My probe confirmed it: a healthy `alpha` caller
  resolved and the corrupt file was removed. However, `read()` still raises a
  bare `json.JSONDecodeError`, which is not a `ChatError`, on paths that check
  without syncing first:
  - `discover=False`;
  - callers with `CHAT_AGENT_ID` set;
  - the identities-disabled path.

  In each case a surface must be present. `pchat` catches only `ChatError`, so
  these paths produce a traceback rather than a clean fail-closed message.
  Suggested follow-up: in `read()`, catch `JSONDecodeError` and raise `ChatError`.
- **N2 (unchanged, pre-existing, outside this scope):** `modelclass` catches
  `ChatError` (which includes `IdentityConflict`) and launches unbound. That
  is fail-open at launch time, separate from posting, which correctly fails
  closed.
- **N3 (unchanged):** the secrecy assertion is still only
  `assertNotIn("accounts", …)`, with fixture credential values of `"x"`. The
  diagnostic schema is restricted to project, role, surface ID and incumbent
  name, so the risk is low. A stronger test would plant a distinctive secret
  value and assert it is absent.
- **N4 (new observation, non-blocking):** the relocated check now runs only when
  `agent_identities_enabled` is false. Discovery never runs in that mode, so a
  diagnostic entry for the surface keeps that surface failing closed until
  identities are re-enabled or the file is removed. This applies even if the
  conflict has ended. It is conservative, the first capture behaved the same
  way, and no test covers it. If identities are ever disabled as an emergency
  rollback, the rollback runbook should say to remove `identity-conflicts.json`.
- **Pre-existing behavior, recorded so it is not mistaken for a regression:**
  base `sync()` retires any surfaced agent missing from a complete cmux
  inventory. The new "absent but live incumbent" preservation applies only
  during the sync in which the conflicting row is present. This is unchanged
  base semantics.

## Process disclosure

When verifying the captures, I ran `tools/ck pijul status` and `tools/ck pijul log`
from inside each capture. This created `tools/__pycache__/environment.*.pyc` and
`pipeline.*.pyc` in both captures, timestamped 10:31:57 and 10:31:58. These are
ignored by `.ignore`, are not tracked, and are outside the fingerprint. I removed
exactly those two self-created directories immediately. Both fingerprints and
`pijul status` were clean before and after the removal.

The first capture also contains `tools/chat/__pycache__/adapter.cpython-314.pyc`,
timestamped 10:27:28. That timestamp falls after its 10:26:04 submission and
inside my original review window, so it may come from that review. I did not
create it in this rereview, and I left it in place. It is also ignored, untracked
and outside the fingerprint. The coordinator may remove it.

I cleaned up all `/tmp` clones, logs, the Ruff cache and the three
`ck-chat-tested-*` stages.

No code, capture content, queue manifest, accepted pointer or earlier verdict
was edited. I performed no submission, acceptance, docs publish, IRC post,
installation, service restart, Git or GitHub operation, and launched no
subagents.

## Acceptance condition

Approve only if both submission IDs are accepted together as one shared-hash
group (`KUZOBK…` plus `HY4IGZB5…`), on top of the current accepted checkpoint,
with the normal combined validation. Before accepting, re-verify the two
manifest SHA-256 values above. If anything changes, a fresh review is required.
Live installation and rollout remain separate coordinator steps.
