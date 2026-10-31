# Independent review verdict: ck-identity-conflict

> **Historical review record (2026-10-14).** This first verdict was superseded by the
> [rereview](chat-identity-rereview.md), which approved the repaired submissions; the repair is installed. See
> [the incident record](../chat-identity-incident.md).

**Verdict: CHANGES_REQUESTED**

The core repair is correct and fixes the reported incident path. Two defects and
one required-coverage omission block approval: a stale diagnostic is not cleared
on the conflicted caller's own resolve, the new tests use private names despite
the assignment, and required foreign-project and conflict-first cases are not
covered.

## Exact scope

- Task: `ck-identity-conflict`
  (`.pipeline/coordination/tasks/ck-identity-conflict.md`), a bounded incident
  repair requested directly by the owner in coordinator session
  `01a13ab7-838c-76d0-99ff-a04491db37e0`.
- Submission: `1791998764113053000-9e90f31d`
- Capture: `~/work/codekarl/.pipeline/submissions/1791998764113053000-9e90f31d`
- Patch: `KUZOBK52QKFLOO3LPNLPXJ2PD5VR4O6TAFIIHITXRPSOPUS5DBOQC` (Pijul author
  "Codex", 2026-10-14 10:25:56 PDT). It is the only hash in the capture that is
  not in the accepted state.
- Queue manifest SHA-256:
  `f7b5b42ab81714a9ec258002c4583880525a0ff9875baa87c59984ad6d81fd10`. This was
  verified at the start and again before signing, and matches the assignment and
  the worker result.
- Base: `checkpoints/1791998061129711000-75d9d0b0`. This is also the current
  accepted state (`accepted.json` SHA-256
  `383770fe936d53f8ea6fb5fab2c9d8bdc67a5977d97214f25bf82e3f2ec5019e`).
- Spec (from manifest): "Skip only typed live singleton ownership conflicts during
  global identity discovery, preserve incumbent and healthy identities with
  accurate duplicate attribution, fail closed for conflicted callers, and clear
  private diagnostics when resolved; staged shared regression suite covers
  before/between-row collisions, absent-but-live incumbents, explicit
  registration and caller resolution, repeat discovery, diagnostic clearing, and
  unrelated errors."
- Changed paths, all read in full:
  - `tools/chat/adapter.py`: new `identities.py` transforms
  - `tools/chat/identity_conflicts.py`: new
  - `tools/chat/install.py`: adds two EXTRA entries
  - `tools/chat/test_identity_conflict.py`: new
- I also reviewed the fully transformed `identities.py`, generated from the live
  shared source (SHA-256 `3c57…c85b`) in a temp directory, covering `register`,
  `sync`, `current` and `resolve`.

## Author and reviewer

- Author (from the coordinator and the worker result): `ck-worker-1`, Codex,
  `gpt-6-luna`, high effort, Class B. Visible left work pane, PID 14763, launch key
  `1a538d63ec0eb9f87c9bdc3344add8ea`, provider session ID null. The coordinator
  reports no Claude implementation contribution. The Pijul author field
  ("Codex") is consistent.
- Reviewer: Claude family, the opposite brand. I did not implement any part of
  this change.
  - Class A: `modelclass check` returns `ok`, and `modelclass get A claude`
    returns `claude-opus-5-5 high` (Copilot spelling `claude-opus-5.5`).
  - Actual: `claude-opus-5.5`, high effort, GitHub Copilot CLI 1.0.97, session
    `9bcdbe98-41c4-4fdf-9a6f-5eb4cc70a523`.
  - `session.start` and `session.model_change` record effort high. All 46
    assistant messages in this session ran on `claude-opus-5.5`. Other
    `model_call_success` entries (medium/low) are Copilot utility calls with no
    review content.
- Communication: the bounded Copilot bootstrap exception in ck-review, with the
  assignment file from the coordinator. No IRC identity was used or claimed.

## Checks

1. **Integrity.** `tools/pipeline.py` `fingerprint(capture)` equals the manifest
   at start and before signing, and `pijul status` is clean. All execution ran in
   disposable Pijul clones under `/tmp`, with `PYTHONDONTWRITEBYTECODE=1`.
   Nothing was written to the capture, queue, accepted state or live shared
   source. The temp clones and my stage directory were deleted.
2. **Staged suite.** `python3 tools/chat/install.py stage` from a disposable clone
   of the capture ran 237 tests, `OK`. The stage path
   `ck-chat-tested-00g7lyd_` is now deleted.
3. **Failure before the fix.** I staged the new test file against the base
   checkpoint's `identities.py` transform. 6 of 7 new tests error: 4 with
   `ChatError: ck-guide already has a live session` and 2 with
   `identities has no attribute IdentityConflict`. The unrelated-error test
   passes on the base, as expected. This confirms the worker's claim.
4. **Independent probes** (disposable stage, my own test subclass):
   - **P1, conflict recovery: fails (B1).** Sync records a conflict, then the
     incumbent exits (`process_start` → `""`). `resolve()` on the conflicted
     surface still raises `IdentityConflict: ck-guide already has a live session`,
     and the diagnostic is still present afterwards. Only a separate `current()`
     call, which runs `sync()`, clears it. After that, `resolve()` returns
     `ck-guide`.
   - **P2, healthy foreign-project caller: passes.** A Synarchy-style solver on a
     different surface resolves to its own identity on the first call with no
     prior sync, and again on repeat. The incumbent stays `active` with its
     original keys.
   - **P3, conflict as the first row: passes.** The following healthy row still
     registers.
   - **P4, corrupt diagnostic:** `resolve()` on an unrelated surface raises
     `JSONDecodeError`, which is not a `ChatError` (N1).
   - **P5, diagnostic privacy:** the file is mode `0600`. It contains only
     `project`, `role`, `surface_id` and `incumbent`; no credentials or account
     secrets.
5. **Code inspection against the required behaviors:**
   - **Typed conflict only:** passes. `IdentityConflict(chatlib.ChatError)`
     replaces exactly the one live-session raise in `register`. `sync` catches
     only `IdentityConflict`, so other `ChatError`, `OSError` and provisioning
     errors still propagate. The worker's test and my reading agree.
   - **Direct register guard:** passes. `register` still raises (now the typed
     subclass), so `pchat agent register` and `prepare_launch` keep the guard.
   - **Incumbent preserved when absent from inventory:** passes. `conflict_live`
     adds both the candidate's and the incumbent's `(pid, pid_start)` to `live`,
     and `register` has already verified that the incumbent is live.
   - **Duplicate attribution:** passes. `registered_rows` stays aligned with
     `records`, and the worker's test covers retirement of `ck-worker-8` as a
     duplicate of `ck-worker-9` after a skipped row.
   - **Caller fail-closed, including an explicit account:** passes, as long as
     the diagnostic is current. `current()` runs `sync()` before
     `raise_for_surface()`, so the first ordinary resolve with no prior sync still
     fails closed (covered by the worker's test and P1's first sync).
   - **No churn:** passes. `replace_all` skips rewriting identical bytes.
   - **Scope limits:** passes. There is no Copilot transport and no change to
     project routing.

## Blocking findings

### B1. A stale diagnostic locks out the conflicted caller after the conflict resolves

- **Location:** `tools/chat/adapter.py`, the `resolve` transform:
  `identity_conflicts.raise_for_surface(os.environ.get('CMUX_SURFACE_ID', ''))`
  inserted as the first line of `resolve()`.
- **Consequence:**
  - `resolve()` raises from the persisted file before `current()` can run
    `sync()`, which is the only code that clears it.
  - `pchat post`/`ack`/`open` reach identities through `who()` →
    `resolve(discover=True)`. After the incumbent exits, the candidate therefore
    keeps failing with the false message "ck-guide already has a live session".
  - Recovery depends on some other process running `sync()`: another agent
    posting, an explicit `pchat agent sync`, or a `ck-chat` command that calls
    `current()`. `chat-bridge` never calls `sync`.
  - This violates "Persist a private, **current** conflict diagnostic … Clear it
    when resolved". It is also an early-discovery bypass of the same kind the
    coordinator flagged.
- **Required behavior:**
  - On a conflicted surface, an ordinary `resolve()` with discovery must re-run
    discovery before deciding. It should fail closed only if the conflict still
    exists, and otherwise return the identity it now owns.
  - The early check appears unnecessary: `current()` already checks after
    `sync()` (and with `discover=False` it checks the last known state). The
    explicit-account case still fails closed through `current()`.
  - Add a regression test: conflict recorded, then the incumbent exits, then
    `resolve()` on the conflicted surface succeeds and the diagnostic is gone,
    with no intervening direct `sync()` or `current()` call.

### B2. The new generic tests use real private project and account names

- **Location:** `tools/chat/test_identity_conflict.py`:
  - lines 27-45: `owner: "vince"`, `assistants: ["laz"]`, alias targets `laz`,
    and projects `codekarl` and `synarchy` with channels `#codekarl` and
    `#synarchy`, manager `ck-manager`, and repos `example/codekarl` and
    `example/synarchy`;
  - lines 67-192: `ck-guide`, `ck-worker-N` and `project: "codekarl"`;
  - line 169: an explicit `"vince"` request.
- **Consequence:** the assignment requires "Use invented project/account names in
  new generic code and tests so Plateia can carry them without private data." The
  coordinator's review notes specify generic `alpha`/`beta` projects and
  `pat`/`sam` accounts for the Plateia capture. As written, Plateia cannot carry
  this suite without importing the owner's private account, assistant and project
  names. The shared fix would also look project-specific.
  `identity_conflicts.py` itself is generic.
- **Required behavior:** rewrite the fixtures with invented names: for example,
  projects `alpha` and `beta` with matching prefixes and channels, owner `pat`,
  assistant `sam`, and singleton `alpha-guide`. Preserve every assertion.
  - The adapter's `worker` role is scoped to `codekarl`, so any worker-role rows
    need a non-worker numbered role such as `solver`. Alternatively, isolate that
    single Codekarl-specific dependency explicitly.

### B3. The required foreign-project and conflict-first regressions are missing

- **Location:** `tools/chat/test_identity_conflict.py`.
- **Consequence:**
  - The assignment requires coverage of "a foreign conflict before/between healthy
    rows", and the review checklist includes "healthy foreign project
    resolution". These describe the actual incident: a healthy caller in one
    project failing because a different project's candidate collides.
  - Every test row is in the same project. The second project is configured but
    never used.
  - No test resolves as a healthy caller while another surface is conflicted.
  - No test puts the conflict before the first healthy row; only the
    between-rows ordering is tested.
  - My P2 and P3 probes show the current code behaves correctly. The missing
    regressions are still required, because the shared code will be carried and
    edited independently.
- **Required behavior:** add tests in which:
  - a conflict in project `alpha` sits before, and separately between, healthy
    rows from project `beta`;
  - the `beta` caller's first `resolve()` with no prior sync returns its own
    identity;
  - the `alpha` incumbent keeps its status and keys.

## Non-blocking findings

- **N1. One bad diagnostic file breaks resolution for every project.**
  - `identity_conflicts.read()` is called on every `resolve()` and `current()`,
    for every project and surface.
  - Malformed JSON raises `JSONDecodeError`, which `pchat`'s `except
    chatlib.ChatError` does not catch, so it produces a traceback. A non-list
    value raises `ChatError` for every caller.
  - Both reproduce the cross-project blast radius of the original incident.
    Writes are atomic, so the risk is low.
  - Consider letting `sync()` overwrite an unreadable diagnostic, and failing
    closed only for the affected surface, or with a clear `ChatError` that names
    the file and a recovery command.
- **N2.** `modelclass run` catches `ChatError` from `prepare_launch` and launches
  without an identity. With the typed conflict this behavior is unchanged
  (pre-existing, out of scope), but the conflict now presents more clearly.
- **N3.** The test's secrecy check `assertNotIn("accounts", …)` is weak. Asserting
  the exact key set of each diagnostic row would be clearer.

## Re-review expectations

Submit a new capture for fresh opposite-brand review, retaining this one, as
`ck-work` requires. The new capture must:

- remove or relocate the early `resolve()` check so that recovery works (B1);
- use invented-name fixtures (B2);
- add the foreign-project, conflict-first and resolve-recovery tests (B3);
- show a staged-suite pass, with failure-before evidence for the new recovery
  test.

Do not install, accept or restart services from this capture.
