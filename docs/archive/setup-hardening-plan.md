# Codekarl setup hardening: completed record

**Status (2026-10-17): complete; kept as the reference record.** Every slice in §12 (S0–S13, S7a–S7e, S10b, S12a/S12b)
and every pilot repair is accepted and published through the review gate; §3.2 lists each batch and time, and
`.pipeline/coordination/workflow-ready/final-readiness-report-20261017.md` summarizes the final state. No slice, repair or
readiness item in this document remains to be done. What remains is deliberately deferred by the owner (§14) and is not
scheduled work. The owner approved six policy decisions (§2), and the manager (Class S) chose the rest of the design here.
New work starts only from a new owner assignment, not from this document.

Sections below were written before implementation, in the future tense; their "Design" items are now installed as
described, except where a later note says otherwise (for example §7.1's deployment target, superseded by S11).

This plan uses four labels:

- **Decided** marks owner policy.
- **Design** marks the manager's chosen implementation.
- **Installed** marks behaviour that exists today.
- **Deferred** marks topics deliberately left out (§14).

When the plan was written, "Installed" meant the state before any slice. Today every "Design" item is installed too.

The review gate (`docs/review-gate-plan.md`, installed by bootstrap
`1792012417088242000`) is authoritative for publication, locks, docs and
pointer state; where they differ, the gate wins. Earlier versions of this
plan, including its reconciliation history, remain in the Pijul docs
history. This is ordinary documentation, published through the docs lane.

## 1. Outcome and boundaries

Make the Rust/Metal and local Pijul setup dependable for parallel agents
without making full CI a per-patch requirement.

These properties hold throughout:

- **Batch validation stays authoritative.** Full validation runs on a frozen
  combined candidate (`ck batch prepare`). An independent integration review
  then precedes `ck batch publish`, and later submissions wait for the next
  batch.
- **One slot for batches only.** `ci.lock` serializes automated batch
  validation and nothing else. Manual full checks run beside it (decision 4).
- **No CI before submitting.** Workers record and submit without a full CI
  pass.
- **Docs need no CI or review.** They queue while a batch is active.
  Executable Markdown is code.
- **On demand only.** Commands run when invoked; there is no scheduler,
  server or daemon.
- **Existing state stays readable.** Accepted state, manifests and receipts,
  including the legacy schema-1 records beside the v2 pointer, stay readable.
  There is no destructive migration and no history rewriting.
- **The FFI boundary stays.** Rust calls Metal through the existing
  Objective-C/C ABI.

Game design, map representation, rendering architecture and new engine
subsystems are out of scope.

## 2. Owner decisions

1. **Three check levels.** The owner answered: "Yes—keep those three levels".
   - The record hook runs formatting and lint only.
   - Workers compile and test in their own copy, lock-free.
   - The single CI slot validates a frozen batch.
2. **A stricter lint baseline, with justified exceptions.** The owner chose
   "Stricter baseline, justified exceptions". That means:
   - Clippy pedantic plus the safety and API rules;
   - the broader Ruff groups;
   - strict Python typing;
   - exceptions only where named, with a reason.
3. **The test split.** The owner answered: "Yes—use that split".
   - Rust owns the executable and Metal tests.
   - Python owns the workflow tests and exploratory probes.
   - `specs/bootstrap.json` stays the one shared CLI specification.
   - Full batch CI must execute Metal coverage.
   - `tools/probes.py` stays until Rust coverage is shown equivalent. Only
     then is that duplicate runner removed.
4. **Manual full checks run independently of batch CI.** The owner said: "if
   the user manually runs ci, then i dont want it to wait, it should run right
   away".
   - A manual full check neither takes nor waits for `ci.lock`, and needs no
     flag to do so.
   - Batch validation keeps the lock, descriptor inheritance and interruption
     protection. A second batch is still rejected.
   - Manual runs never produce an authoritative receipt.
   - Manual and batch runs never share Cargo target or build directories, or
     logs.
   - Timing is not promised.
5. **Add Clang static analysis and ShellCheck.** The owner answered: "Add both
   checks".
   - Clang static analysis covers the Objective-C bridge.
   - ShellCheck covers shell scripts. `tools/bootstrap` is an extensionless
     POSIX `#!/bin/sh` script, so discovery must find it by shebang.
   - Each check gets a controlled failing example that proves it fails the
     recipe.
6. **Include safe cleanup now.** The owner answered: "Include safe cleanup
   now". GC is designed in §10 and implemented in slice S12. Nothing is
   deleted before that slice is accepted and its first dry run is reviewed.

## 3. State when the plan was written (historical)

This section records the starting point, before any slice. The current recipe is §4 as implemented (13 steps;
`docs/tooling.md`), and the current status is §3.2.

**Installed recipe at that time** (`tools/check.py`):

- **`--quick`**, also the `pijul.toml` record hook, runs:
  - `cargo fmt --check`;
  - `ruff check` and `ruff format --check` over `tools`, `tools/ck`,
    `tools/ck-chat` and `tests`.

  The Ruff rules are `E4, E7, E9, F, I`, targeting py311 at line length 100.
- **Full** runs `--quick`, then:
  1. doctor, which pins rustc 1.91.1, pijul 1.0.0-beta.24 and Ruff 0.14.0,
     prints the Xcode and Metal versions, and requires Python 3.11 or later;
  2. Clippy (`--workspace --all-targets --all-features --locked -D warnings`);
  3. `cargo test`;
  4. `unittest discover -s tests -v`;
  5. `cargo build`;
  6. `tools/probes.py`.

**Installed locks** (`tools/gate/locks.py`):

| Lock | Behaviour |
| --- | --- |
| Order | `ci.lock` → `state.lock` → `meta.lock`, with a runtime order error |
| `state.lock`, `meta.lock` | Block for up to 60 s, polling every 50 ms. The timeout message gives only the lock's path. |
| `ci.lock` | Nonblocking: "Already running". Batch CI inherits it as `CK_CI_LOCK_FD`, validated by inode. |
| Standalone `tools/ck check` | Still takes `ci.lock` (`environment.lock`). Decision 4 changes this in S3. |

**Baseline counts.** The current baseline is the L1 batch (§3.1): 182 Python
workflow tests and every probe, including `--metal-smoke`, pass on one
combined run. The breakdown below is the earlier bootstrap baseline:

- **Python workflow tests:** 145 at bootstrap. Pipeline 19, gate 12, gate
  criteria 13, batch 20, bootstrap 54, chat 27.
- **Executable probes:** 6. The `--metal-smoke` probe verifies 64 GPU results
  and is the only GPU coverage.
- **Cargo tests:** 0.

The Rust, native and shader sources are byte-identical to checkpoint
`1792004416603094000-55db2e79`, so the 14 Rust lint findings measured there
still apply.

**Proposed Ruff groups.** `W, B, UP, SIM, RUF, PTH, PL, C4, PIE, RET, PGH, ANN`
report 872 findings:

| Area | Findings | Of which `ANN*` |
| --- | --- | --- |
| Live code and tests | 781 | 642 |
| `tests/fixtures/legacy` | 91 | 78 |

The 139 non-`ANN` findings in live code are led by `PLC0415`, `SIM117`,
`PLR0912` and `PLR0915`.

**Fixture classification.** This decides the exclusion in §6.

- **`tests/fixtures/legacy/{ck,pipeline.py,environment.py}`.** A deliberate
  byte-pinned copy of the pre-gate controller.
  - `tests/test_gate.py` asserts their SHA-256 values.
  - `tests/test_bootstrap.py` executes them to simulate the old controller
    during activation.
  - Editing them for lint would break the pin and the simulation, so they are
    historical, not live.
- **`tests/fixtures/sessions/*.jsonl`.** Data, not code.

**Tool availability, probed 2026-10-14 (no CI run):**

| Tool | Observed |
| --- | --- |
| Venv Python | 3.14.8 |
| Xcode | 27.0 (27A266a); Apple clang 21.0.0 |
| PyPI `shellcheck-py` | 0.11.0.1, which ships ShellCheck 0.11.0 |
| PyPI `mypy` | 2.4.0 |
| `shellcheck tools/bootstrap` | No findings |
| Analyzer on `native/bridge.m` (bridge flags) | No findings |

Analyzer exit status on a deliberate null dereference:

- `clang --analyze` exits **0**;
- adding `-Xclang -analyzer-werror` makes it exit **1**.

**Stale guidance.** `AGENTS.md` still says "Record with `tools/ck pijul
record`". The trampoline disables that after activation (`tools/ck`, the
`record` guard), so records go through `ck record`.

**Pre-G1 evidence: `ck sync` omitted live queued docs.** This paragraph
describes the gate before G1. It is kept as the evidence for that repair;
§3.1 describes the current behaviour.

- **Expected.** Gate plan §8.2 says `ck sync` supplies the live queued docs
  that docs capture's freshness rule requires. §8.4 says sync brings a copy
  to the accepted state plus the live queue.
- **Installed before G1.** `tools/gate/docsqueue.py` `sync` (lines 439–449
  at that time) pulled only the accepted hashes.
- **Probe.** The coordinator ran a bounded real-Pijul probe with the existing
  `BatchGateSpecification` fixture:
  - a pending `README.md` entry was absent after sync;
  - a follow-up docs capture on that path then failed with "Working copy is
    stale for these docs paths; run ck sync first".
  - Evidence: `.pipeline/coordination/docs-sync-queue-probe.json`.
  - Temporary repositories were removed. The production queue and pointers
    were untouched, and no CI ran.
- **Disposition.** Repaired by slice G1, now accepted (§3.1).

### 3.1 Accepted since the plan was consolidated

**Copilot provenance repair (CK-1).**

- Submission `1792033208928860000`, batch `1792033565464600000`, pointer
  `e4aa6c8b…`.
- The gate now resolves Copilot sessions from direct client evidence and fails
  closed. `ck session --role R` is a read-only provenance diagnostic.
- It also fixed a test-isolation bug: `test_c05` patched
  `gate.meta._authors` from two threads, and an out-of-order exit leaked a
  fixture author into later tests. One combined run had failed on it.
- CI: `.pipeline/ci/1792033565464600000.json`, 164 tests and Metal.
- Evidence: `.pipeline/coordination/copilot-provenance-repair-result.md`.

**G1 (CK-2@r2).**

- `ck sync` pulls the live docs queue after the accepted checkpoint.
- `ck submit` refuses any closure patch that changes a docs path. Each patch
  is replayed, so directory moves and executable-bit changes count. Review
  rounds found both cases.
- Submission `1792036526376252000`, batch `1792036793934196000`, current
  pointer `f10eb07f…`.
- CI: `.pipeline/ci/1792036793934196000.json` (sha256 `024fa5fb…`) passed on
  one combined run, with no split. The log
  `.pipeline/checkpoints/1792036793934196000-combined.log` shows "Ran 173
  tests … OK" and `PASS ['--metal-smoke']`.
- Evidence: `.pipeline/coordination/g1-reviewed-fix-result.md`.

**L1, model-switch readiness (CK-3@r1).**

- Readiness now separates the current pair from attributable history (gate
  plan §2.4). A reconciled author entry keeps `off_class`, the full `models`
  list and the prefix evidence. Reviews stay strict.
- Submission `1792090900847282000`, batch `1792091100586830000`, pointer
  `c609d762…`.
- CI: `.pipeline/ci/1792091100586830000.json` passed on one combined run, with
  182 tests and Metal.
- Evidence: `.pipeline/coordination/model-switch-repair-result.md`.

### 3.2 Live pilot (2026-10-15) and the completed goal (to 2026-10-17)

**Pilot record (2026-10-15).** The pilot's reliability findings were later repaired or given an owner disposition
(see the goal status below and the findings ledger). The native Claude/Codex
pilot exercised owner authentication, routing, dispatch, peer reply,
deduplication, interruption and exact-session resume. Delivery was turned off after the pilot (it has been on as the
routine state since 2026-10-16 01:24:52Z), and the outstanding inbox was empty. Both `vince` and `coghex` authenticated.

The single report is
[Live pilot findings and worklist](live-pilot-findings.md), now closed. It consolidates
preflight F1–F8, L1–L3, D1, manager O1–O5 and the coordinator's independent
findings, including the omitted P5 pre-send failure. It gives them unique
PF IDs because the two reports used O4 for different problems.

The report qualifies the earlier all-pass summary: P2/P3/P5/P7 worked but
lack promised delivery-event evidence; every pilot recipient row still has
`notice=pending`. P5 recovered after a failed first reply; P6 cleanup needed
a repeated coordination instruction. P6h was skipped, not passed. Unknown
bracket-kind behavior (F7) was subsequently reproduced in an isolated local
probe, without live IRC or terminal activity.

L1 is accepted (§3.1). Its retained mixed/unknown-history limitation L3 is
PF-12. Stale notices L2 are PF-08; guidance intersects S13. The remaining
pilot workflow contracts, evidence, recommendations and acceptance checks
live in the consolidated report rather than a second competing list here.
Archived operational reports remain under `.pipeline/coordination/`.

**Owner authorization since then.** The owner later approved the
pilot-first, S3/S0 and remaining-S sequence as an implementation goal,
`ck-workflow-ready` (`.pipeline/coordination/tasks/ck-workflow-ready-goal.md`).
Its current status, as of 2026-10-17 ~04:50Z (code status and live verification are listed separately;
earlier status summaries are in the Pijul docs history):

- **Accepted** (batch, publication time): CK-4 (PF-01, PF-04, PF-07); CK-5 (PF-02, PF-05) and CK-6 (RC-1),
  `1792113249834006000`, 01:23:25Z, adapter installed; CK-9 (PF-12) and CK-8 (RC-2), `1792115313013817000`, 01:53:33Z;
  CK-13 (RC-3), `1792116357358030000`, 02:09:11Z; CK-7 (PF-06/08/10 guidance), `1792116963374632000`, 02:20:54Z;
  CK-10 (PF-11 `ck-chat delivery`), `1792118143758785000`, 02:39:14Z; **S0** (CK-12), `1792118805068784000`, 02:52:42Z;
  **S9** (CK-16), `1792119642558394000`, 03:06:46Z; **S2** (CK-15), `1792120997803062000`, 03:27:04Z; **S1** (CK-14),
  `1792122308508819000`, 03:49:20Z; CK-33 (bootstrap tests stop reading `docs/review-gate-plan.md`),
  `1792123806339165000`, 04:14:14Z; **S3** (CK-11, after two review rounds on interrupt cleanup and test isolation),
  `1792125055414350000`, 04:34:54Z, with the gate plan §7.2/§9 lock text published through `ck docs` afterwards;
  **S6** (CK-17, Ruff rules and `./`-anchored discovery), `1792126420178293000`, 04:57:24Z (submission review `4820079f`,
  integration `9156fad2`), with the chat adapter re-installed from the accepted root (backup
  `codekarl-install/1792126654491304000`; the shared chat-input aggregate is now `03eba647…`; the running bridge keeps its
  loaded modules until its next restart).
- **S2 evidence is headless only:** a disposable LSP harness drove the pinned rust-analyzer with the shipped settings
  (pinned Clippy, `len_zero` diagnostic, separate target directory). No editor or rust-analyzer extension is installed
  on this Mac, so the settings take effect when the project is opened in such an editor.
- **Live verified:** PF-01 (Claude and Codex), PF-03, PF-04, PF-05 (visible `[ack]` on both clients, informational
  quiet), PF-07 (bridge post routed with role `bridge`, no recipients), PF-09 draft (Claude, Codex) and dialog (Claude).
  A Codex idle interrupt worked; a **busy** Codex turn was steered but not stopped by the bridge (shared-bridge
  `lifecycle` check). PF-02's transient trigger is unit-tested only.
- **Delivery is ON** as the routine state since 01:24:52Z.
- **S3 proof:** a real manual full check ran start to finish while a real batch held `ci.lock` (`workflow-ready/s3-proof/`).
  `docs/review-gate-plan.md` is ordinary documentation since CK-33 (no test reads it), under the existing `AGENTS.md`
  rule; no owner exception was needed.
- **Accepted since:** CK-34 (owner sign-off receipts, 07:06Z) and **S4** (CK-19, 08:58Z). S4 took nine fixes: the
  reviewer found a series of signal and cleanup windows in the step watchdog, then batch CI exposed a test-environment
  leak, and the first passing batch showed the 20-minute watchdog was under 10× the slowest step (127.8 s), so CK-19 r2
  set 30 minutes (13.9× in the accepted batch). The owner's human sign-off of an early fix stayed on record and was not
  used, because that fix had a known defect.
- **Accepted:** S5 (CK-20, 09:44Z): pinned ShellCheck and mypy, doctor enforcement, ShellCheck discovery and the Clang
  static analyzer. S8 (CK-21, 10:06Z): Clippy pedantic and safety rules, rustc lints, rustdoc in fast, bounded FFI
  parsing. S10 (CK-23, 15:20Z): Rust CLI spec runner and supervised native GPU test, Phase A (both runners). S12a
  (CK-24, 15:50Z): checkpoint leases, private-container clones and the owned-group cleanup contract, with CK-35 (step
  watchdog 45 minutes). S10b (CK-31, 17:06Z): Phase B recipe; `tools/probes.py` and the
  `cargo build` step removed. S12b (CK-25, 17:30Z): `ck gc`. The first real dry run on this hub was reviewed, and the
  first `--apply` then reclaimed 169.1 MiB of superseded build output (`workflow-ready/gc-first-dry-run/`,
  `gc-first-apply/`). CK-36 (18:06Z): the Python tests split into four bounded groups (slowest step 63.5 s, 42.5× under
  the watchdog). S13 (CK-32, 18:17Z): agent guidance in `AGENTS.md` and a `CLAUDE.md` that imports it; README and
  `docs/tooling.md` brought up to date. **S7 (CK-26..CK-30), 2026-10-16 20:14Z to 2026-10-17 03:30Z:** strict mypy over
  `tools`, `tools/ck`, `tools/ck-chat` (now a three-line shim) and `tests`, with validating `TypedDict` loaders for every
  on-disk record. The temporary per-path exemption list is deleted, and a test keeps it from returning. S7c moved the
  checkpoint launcher into the typed `tools/launcher.py`, with an importer-side origin check. S7d reinstalled the chat
  adapter (aggregate `f133dc42…`, bridge not restarted). S7e landed in three batches of six submissions. The slowest step
  in the final batch was 89.3 s (30.2× under the watchdog); `test_check` alone is about 83 s of its group, so a further
  split would be within that module.
- **Owner decisions (2026-10-17, record `workflow-ready/owner-closeout-decisions-20261017.md`):** development targets
  only the current Mac (macOS 26.7.1, arm64), so S11 (CK-22@r3, **accepted** 04:49Z, batch `1792212064912955000`) sets an
  explicit deployment target of 26.0 and keeps shader line tables unconditionally (§7.1 updated); the busy Codex stop is deferred
  with its reproducer, with no Plateia escalation; one bounded live check ran and passed (bridge restart, delivery,
  Codex dialog E); the remaining PF-09 recovery cases are accepted as untested (see the findings ledger). The setup
  hold on dispatch was lifted afterwards (owner, 2026-10-17T15:00:42Z; CK-39): the manager dispatches owner-authorized work through the
  normal review and acceptance workflow, and the deferrals above remain known limitations.

**Readiness items RC-1, RC-2 and RC-3** are all accepted: RC-1 (chat suite in batch CI) by CK-6, RC-2 (eligibility
from patch provenance) by CK-8, RC-3 (quiescence scan matching unrelated commands) by CK-13. See the [findings ledger](live-pilot-findings.md).

## 4. Check recipe

### 4.1 Levels and steps

There is one recipe in `tools/check.py`. Each level selects a subset of one
ordered step list, and runs each selected step exactly once, in that order.
There is no second implementation, and no level re-runs another level's steps.

| # | Step (exact command) | Quick | Fast | Full A | Full B |
| --- | --- | --- | --- | --- | --- |
| 1 | `cargo fmt --all -- --check` (installed) | ✓ | ✓ | ✓ | ✓ |
| 2 | `ruff check` and `ruff format --check` (§6.1 discovery) | ✓ | ✓ | ✓ | ✓ |
| 3 | ShellCheck (§4.4) | ✓ | ✓ | ✓ | ✓ |
| 4 | Doctor (§4.3) | | ✓ | ✓ | ✓ |
| 5 | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` (installed) | | ✓ | ✓ | ✓ |
| 6 | `cargo doc --workspace --no-deps --all-features --locked`, with `RUSTDOCFLAGS=-D warnings` | | ✓ | ✓ | ✓ |
| 7 | `cargo test --workspace --all-features --locked` (installed arguments) | | ✓ | ✓ | ✓ |
| 8 | Python tests in four groups (`unittest-batch`, `unittest-check`, `unittest-bootstrap`, `unittest-lease`), each running `python -m unittest discover -s tests -b -p <module>.py` once per module (CK-36) | | ✓ | ✓ | ✓ |
| 9 | mypy (§6.2) | | ✓ | ✓ | ✓ |
| 10 | `cargo build --workspace --all-features --locked` (installed) | | | ✓ | |
| 11 | `tools/probes.py` (installed) | | | ✓ | |

Notes on the steps and levels:

- **Rust arguments are fixed.** Steps 5–7 and 10 keep the installed
  `--workspace --all-features --locked` arguments, so S3 loses no
  pre-migration coverage.
  - Clippy keeps `--all-targets`.
  - `cargo test` deliberately has no `--all-targets`. That flag would drop
    doc tests, and plain `cargo test` already builds and runs lib, bin,
    integration and doc tests.
- **The native analyzer runs once.** It runs inside the build script, through
  `CK_NATIVE_ANALYZE=1` (§4.5). `check.py` sets the variable for steps 6, 7
  and 10, and not for Clippy.
  - Probed here with Rust 1.91.1 in a fresh target directory: Clippy
    compiles and runs its own build-script unit through `clippy-driver`.
  - `doc` runs a second build-script unit. `test` and `build` reuse that
    unit without rerunning it.
  - Setting the variable only on the shared unit makes the analysis run
    exactly once per run. It also never invalidates Clippy's unit.
- **Spec modes.**
  - Fast runs the Rust specs normally. Any skipped feature case is listed in
    visible output.
  - Full sets `CK_REQUIRE_FULL_SPEC=1` for steps 7 and 10–11, in both
    phases. Before S10 adds Rust specs the variable has no effect, and
    `probes.py` already requires the Metal case.
- **Quick stays compile-free.** ShellCheck is lint, so it joins quick. mypy
  is slower, so it waits for fast, which keeps the record hook fast.
- **Doctor never touches a lock.** Fast includes it so that a wrong tool
  version fails early.
- **Fast is diagnostic only.** It never writes a receipt. Workers may also run
  narrower commands, such as `tools/ck cargo test -p ck-gpu-metal`.

| Command | Level | Steps | `ci.lock` |
| --- | --- | --- | --- |
| `tools/ck check --quick` | 1, the record hook | Quick | Never |
| `tools/ck check --fast` | 2, the worker loop | Fast | Never |
| `tools/ck check` (manual) | full | Full A, later Full B, on the caller's copy | Never (decision 4) |
| `ck batch prepare` | 3, batch | Full A, later Full B, on the frozen candidate | Inherited `CK_CI_LOCK_FD`, as installed |

### 4.2 Full recipe, in two phases

The full recipe has two phases because of the approved test split (decision 3).
In both phases, full batch CI executes the Metal case, and a skip or
unavailable GPU fails it.

- **Phase A** (column "Full A"): now and during the migration. It runs the
  Python executable probes on the binary that step 10 builds.
- **Phase B** (column "Full B"): after S10 shows Rust CLI and GPU coverage
  equivalent in a Phase A batch. S10b removes steps 10 and 11, and nothing
  else. Cargo then builds and runs the binary under test itself, in
  full-spec mode.

### 4.3 Tool installation and version enforcement

A recipe step may require only tools that `tools/bootstrap` installs or
checks. Doctor enforces each version, and a missing tool fails with the
installation command; nothing is silently skipped.

| Tool | Installed by | Enforced |
| --- | --- | --- |
| rustc, Cargo, rustfmt, Clippy, rustdoc | `rustup toolchain install 1.91.1` (rust-toolchain.toml) | Doctor: `rustc 1.91.1` |
| Ruff | `tools/requirements.txt` `ruff==0.14.0` | Doctor (installed) |
| ShellCheck | `tools/requirements.txt` `shellcheck-py==0.11.0.1`, into `.tools/python/bin/shellcheck` | Doctor: `version: 0.11.0`. The recipe uses the hub path, never Homebrew's. |
| mypy | `tools/requirements.txt`: exact pins of mypy and its dependencies. The version is the newest release supporting the venv Python when S5 lands; 2.4.0 today. | Doctor: exact version |
| Clang static analyzer | Xcode, selected via `DEVELOPER_DIR` | Doctor: `xcrun --sdk macosx clang --version` reports Apple clang. Bootstrap cannot install Xcode; it already prints the requirement. |
| Pijul | `tools/bootstrap` | Doctor: 1.0.0-beta.24 (installed) |
| Python | System `python3` for the venv | Doctor: 3.11 or later (installed). mypy sets `python_version = "3.11"` to keep that floor. |

**Xcode is not pinned by version, deliberately.** It is an owner-managed
system application that bootstrap cannot supply. Native warnings stay at
`-Wall -Wextra -Werror`, with no `-Weverything`. The analyzer runs its default
checkers, so a new finding after an Xcode update is a real finding: fix it.
Doctor records the Xcode and clang versions in every log, so a change is
visible.

### 4.4 ShellCheck discovery

`check.py` discovers shell sources itself; it keeps no fixed list.

- **Scope.** Tracked files from `pijul list` in the checked copy, excluding
  `.pipeline/`, `.tools/`, `target/` and `tests/fixtures/legacy/`.
- **Match.** A file is shell if:
  - it ends in `.sh` or `.bash`; or
  - its first line is a `#!` naming `sh`, `bash`, `dash` or `ksh`, directly
    or through `/usr/bin/env`.
- **Run.** `shellcheck --severity=style` on the matches, with no global
  exclusions.
- **Exceptions.** A justified exception is a comment line giving the reason,
  directly above a separate `# shellcheck disable=SCnnnn` directive line. The
  directive applies to the next command.
  - ShellCheck 0.11.0 also accepts a trailing `# reason` inside the
    directive, and both forms were verified here.
  - The separate line is chosen because it is clearer.
- **Coverage guard.** The recipe fails if `tools/bootstrap` is not among the
  matches, so a discovery regression cannot silently drop it.

### 4.5 Clang static analysis

The analyzer runs inside `crates/gpu-metal/build.rs`, so it reuses the exact
argument vector of the real bridge compile. That vector covers the SDK,
`-arch`, `-fobjc-arc`, `-std=c11` and the S1 deployment target, so the
analysis cannot drift from the build.

- **When it runs.** Only when `CK_NATIVE_ANALYZE=1`, declared with
  `cargo::rerun-if-env-changed`. It runs before the object compile:
  `xcrun --sdk macosx clang <bridge args> --analyze -Xclang
  -analyzer-werror -o $OUT_DIR/bridge.analysis`.
- **Who sets it.** `check.py` sets it for steps 6, 7 and 10, in fast and
  full (§4.1 explains why not Clippy).
- **Cost.** Editor and plain `tools/ck cargo` builds skip the analyzer. When
  the variable changes, only the small build script reruns.
- **Failure.** `-analyzer-werror` is the verified failure mechanism. The build
  script panics with the analyzer's diagnostics, which Cargo prints.

### 4.6 Output, diagnostics and timeouts

**Output:**

- Echo each command (installed), and finish with a one-line summary of the
  steps and the result.
- Python tests run with `unittest discover -s tests -b`, once per module in
  four groups (CK-36), without `-v`. A
  passing test's output is suppressed, and a failing test's output is shown
  in full.
- Native compiler and analyzer diagnostics pass through unchanged, never
  replaced by a generic failure.
- Batch runs keep writing their log under `.pipeline/checkpoints/` (installed).
  Manual and fast runs write to the terminal.

**Timeouts:**

| Timeout | Value | Rationale |
| --- | --- | --- |
| `state.lock`, `meta.lock` | 60 s (installed) | The longest critical sections (freeze, publish, flush) are Pijul operations that take seconds. S4 makes the message name the holder. |
| Each CLI or GPU spec case | 30 s | The current `probes.py` value. Normal cases take under a second, but the Metal debug layer slows startup. A hang becomes an ordinary test failure that names the case. |
| Each step of fast and full (watchdog) | 45 min | A hung Pijul, Cargo or GPU process must not hold the CI slot forever. The watchdog stays at least 10× the slowest step, measured in a batch's own log. The slowest step is `unittest`: 127.8 s at S4 (20 min gave 9.4×, so CK-19 r2 set 30 min), then 188.1 s with S12a's tests (9.6× of 30 min), so CK-35 set 45 min (15.1× in the S12a batch, 178.6 s). The single step then reached 225.1 s (12.0×) at S12b. Because the watchdog cannot keep growing and still catch hangs promptly, CK-36 split it into four groups of at most 120 s each; the slowest step in CK-36's batch was 63.5 s (42.5×). |
| Checkpoint lease (§10.4) | 5 s, polling every 50 ms | GC holds an exclusive lease only across one rename. |

**Failure classes.** These are S4 design.

- **Watchdog.** When the watchdog fires, `check.py` kills the step's process
  group, prints the step and its elapsed time, and exits **75**
  (`EX_TEMPFAIL`).
- **Batch infrastructure failure.** When the check exits 75, `batch prepare`
  writes `failed.json` with reason `infrastructure`. It does not split the
  batch or quarantine anyone; members stay queued for the next batch, and
  docs drain as for any other failure.
- **Lock contention.** A busy lock is never a patch failure: "Already
  running" and the lock timeouts stop the command before it records anything.

### 4.7 Manual full checks and build isolation

This section implements decision 4 in S3.

**Lock behaviour:**

- **Manual checks are lock-free.** The trampoline's `check` path stops calling
  `environment.lock`.
- **Batch runs.** `check.py` runs as a batch when it inherits
  `CK_CI_LOCK_FD`. It validates the descriptor by inode, exactly as installed.
  An invalid inherited descriptor fails at once, with the restart command.
- **Manual runs.** Without the variable, the run is manual and touches no
  project lock.

**Build directories.** `check.py` sets both `CARGO_TARGET_DIR` and
`CARGO_BUILD_BUILD_DIR`, to the same directory, for every Cargo and probe
child. It also removes any inherited `CARGO_BUILD_TARGET_DIR`. Environment
variables override `build.target-dir` and `build.build-dir` config. The
directories are:

- **Batch:** `<candidate>/target`, a fresh clone per candidate (installed
  default, now made explicit).
- **Manual full:** `<copy>/target/ck-full/<n>`.
  - The run picks the lowest `n` whose `<n>.lock` it can take with a
    nonblocking `flock`.
  - A lone run reuses slot 0's warm cache, and a concurrent run takes slot 1.
  - No run ever waits.
  - That lock is private to the copy, and is not a project lock.
- **Quick and fast:** the copy's default `target/`, shared with interactive
  `tools/ck cargo`. Same-copy contention there is ordinary Cargo locking,
  which is acceptable.

**Slot lifetime.** The slot is held until every process that can write its
directory has exited, not just until `check.py` exits.

- **Inheritance.** The slot descriptor is opened without close-on-exec and
  passed through `pass_fds` to every step's child, the same way `run()`
  already passes `CK_CI_LOCK_FD`.
  - `flock` belongs to the shared open file description, so Cargo, rustc,
    build scripts and test binaries all inherit it across fork and exec.
  - The lock is released only when the last holder closes it or exits.
- **Orderly exit.** Each step runs in its own process group. On normal
  completion, an error, Ctrl-C, SIGTERM or a watchdog timeout, `check.py`
  kills that group, waits for it, and only then closes its own descriptor.
- **Parent death.** If `check.py` is killed with SIGKILL, the descriptors
  its descendants inherited keep the slot held. A new manual run therefore
  takes the next slot instead of colliding.
  - This was probed on macOS for this plan: a parent took the lock, passed it
    to a `sh` child and its background grandchild, and was killed. A
    nonblocking exclusive attempt was refused until the grandchild exited,
    then succeeded.
- **Batch runs** need nothing new. Their target directory is a per-candidate
  clone, and `CK_CI_LOCK_FD` already flows to descendants as installed.
  Preserving that descriptor is not what protects manual slots; the
  inherited slot descriptor is.
- **Limitation.** A descendant that deliberately closes inherited
  descriptors would drop its share of the slot. Cargo, rustc and Rust's
  `std::process::Command` do not close inherited descriptors.
  - S3's interruption test checks the real recipe, so a toolchain change that
    broke this would fail the test.
  - A Python child that only executes an already-built binary does not write
    to the slot.

The concurrency evidence is §4.8.

**Logs and receipts.** Manual and fast runs never write under
`.pipeline/checkpoints/` or `.pipeline/ci/`, and cannot create or replace a
receipt.

### 4.8 Concurrency note

Separate copies build safely at the same time; no project-wide compiler lock
is needed.

- **Cargo locks its own output.** It takes an exclusive lock on each target
  directory, and a separate one on a separately configured build directory
  ([`layout.rs`][cargo-layout]).
- **The shared Cargo home is safe.** Its locks cover dependency resolution,
  download and mutation, and concurrent source-cache readers are supported
  ([`cache_lock.rs`][cargo-cache-lock]).
- **Native outputs are per build.** Build scripts write to `OUT_DIR`
  ([build-script outputs][cargo-build-scripts]). `build.rs` writes
  `bridge.o`, `libck_metal.a`, `probe.ir` and `probe.metallib` there.
- **A local probe agreed.** Two clean copies built at the same time with
  `--workspace --all-features --locked --offline --jobs 2`.
  - Both returned 0 in about 2.38 s, with overlapping intervals.
  - Both `--metal-smoke` runs verified 64 results.
  - The only waits were on Cargo's package-cache lock.
  - Evidence: `.pipeline/coordination/build-concurrency-2026-10-14/`.

Concurrent runs share CPU, memory and GPU. If agents overload the machine,
lower `CARGO_BUILD_JOBS` in the `tools/ck` environment rather than widening
a lock.

[cargo-layout]: https://github.com/rust-lang/cargo/blob/ea2d97820c16195b0ca3fadb4319fe512c199a43/src/cargo/core/compiler/layout.rs
[cargo-cache-lock]: https://github.com/rust-lang/cargo/blob/ea2d97820c16195b0ca3fadb4319fe512c199a43/src/cargo/util/cache_lock.rs
[cargo-build-scripts]: https://doc.rust-lang.org/cargo/reference/build-scripts.html#outputs-of-the-build-script

## 5. Locks and diagnostics

The gate's protocol stays. No component may reintroduce a "state, then CI"
order (gate §9 and §10).

| Operation | Locks, in order |
| --- | --- |
| `batch prepare` | `ci` throughout; `state` and `meta` for the freeze; `state` again for each state write and the record |
| `batch cancel` of a frozen or validating batch; interruption recovery | `ci`, then `state` |
| `batch publish`, integration verdicts, the `submit` manifest write | `state`, then `meta` |
| `docs` capture, flush and resolve | `state` |
| `gc` plan and renames (§10) | `state`, then `meta`; checkpoint leases are only tried, never awaited |
| Batch-run full check | The batch's inherited `ci` descriptor |
| Manual full check, fast, quick, `tools/ck cargo`, editor | None (manual full after S3) |
| Controller lifetime, and `worker`/`sync` sources | A shared checkpoint lease (§10.4), after S12 |
| `status`, `issue list` | None |

**Holder diagnostics (S4).** After acquiring a project lock, write
`{"pid", "command", "started"}` into the existing lock file in place:
truncate, write, flush, keeping the same inode.

- The kernel lock remains the only proof of ownership; the metadata is a
  hint.
- **Timeout and busy messages** read the metadata. The 60 s timeout says, for
  example, "state.lock held by pid 123 (`ck batch publish`, 41 s)". "Already
  running" names the batch id and pid.
- **Stale metadata.** If the metadata names a dead pid while the lock is held,
  the message says so. Descriptor inheritance means a child may hold the lock
  after its parent exits.

## 6. Python lint and types

### 6.1 Configuration (S6)

- **Rules.** Extend Ruff `select` with
  `W, B, UP, SIM, RUF, PTH, PL, C4, PIE, RET, PGH, ANN`, keeping
  `E4, E7, E9, F, I`.
  - The blanket `S` group stays off. Subprocess orchestration is the
    project's purpose, and `S603`/`S607` would flag every call.
  - `PLW1510` stays on. Code that inspects a return code says `check=False`
    explicitly, rather than switching to `check=True`.
- **Discovery.**
  - Add `tools/ck` and `tools/ck-chat` through `extend-include`, so a bare
    `ruff check` covers the extensionless entry points.
  - `extend-exclude` `.pipeline`, `.tools` and `target`, so historical clones
    and installed packages are never linted.
- **The historical fixture.** One `per-file-ignores` entry for
  `tests/fixtures/legacy/**` turns off only the new groups. The reason in the
  configuration reads "byte-pinned pre-gate controller; `test_gate` asserts
  its SHA-256".
  - The existing rules and the format check still run on it, and pass
    unchanged.
  - mypy excludes the same path for the same reason.
  - No other test file is excluded.
- **Suppression.** Use `# noqa: CODE  # reason` only. Global exclusions live
  in `pyproject.toml` with a reason.

### 6.2 Strict typing (S7)

- **mypy configuration.** Pin as in §4.3. Set `strict = true`,
  `mypy_path = "tools"` and `files` covering `tools`, the entry points and
  `tests`.
- **Entry points.** `tools/ck` and `tools/ck-chat` are checked as scripts. If
  mypy cannot check an extensionless file directly, the entry point becomes a
  three-line shim around a typed module. A spec then proves that a type error
  in executed entry-point logic fails mypy.
- **The trampoline.** The checkpoint-pinned launcher string inside `tools/ck`
  is checked by moving its body into a typed module imported from the
  checkpoint. The origin check is unchanged.
- **Typed boundaries.** Each on-disk record has a loader that validates
  required fields and types, preserves unknown additive fields, and returns a
  `TypedDict`. There are no broad `cast` calls and no `Any` beyond the JSON
  parse itself. The records:
  - `FileEntry` and `Fingerprint`;
  - schema-1 accepted state, manifests, successful CI receipts, failed reports
    (which can lack `kind`) and docs receipts (`inherits` instead of a CI
    payload). These only need to load for display and audit;
  - schema-2 records:
    - the `accepted-v2.json` pointer (`receipt`, `previous`,
      `superseded_legacy`, `integration`) and the tombstone;
    - manifests with issues, provenance and authors;
    - batch state files;
    - docs entries, the journal and docs receipts;
    - bootstrap records;
    - `ck-provenance/1` payloads.

  Every valid existing on-disk form must load. Shapes are never falsified to
  satisfy mypy.
- **Errors.** User-facing failures name the next action: the command that
  inspects the state, a valid retry, or the condition to resolve. No error
  suggests deleting work as the default recovery.
- **Tests.** The existing 182 workflow tests keep their assertions, except the
  CI-slot assertions that decision 4 changes in S3. New behaviour gets new
  cases.

### 6.3 Rollout (S7)

The 642 `ANN` findings and mypy strictness land module by module, behind a
temporary per-path list in `pyproject.toml`. That list covers both Ruff `ANN`
ignores and mypy `ignore_errors`, and its reason reads "S7 migration pending".

Each S7 submission removes its modules from that list. The slice ends when the
list is empty and both the list and its reason are deleted. The order:

1. **S7a: gate core.** `pointer`, `locks`, `meta`, `reviews`, `provenance`.
2. **S7b: gate workflows.** `batch`, `docsqueue`, `bootstrap_core`, `cli`.
3. **S7c: the legacy pipeline and entry points.** `tools/*.py` (pipeline,
   environment, check, probes) and the entry points.
4. **S7d: chat.** `tools/chat` and `tools/ck-chat`.
5. **S7e: tests.** All of `tests/`, excluding the fixture.

## 7. Rust, native, shaders and editor

### 7.1 Deployment target (S1)

- **Resolve the target once.** Read it from `MACOSX_DEPLOYMENT_TARGET`;
  otherwise use `$RUSTC --print deployment-target --target $TARGET`, which
  prints `MACOSX_DEPLOYMENT_TARGET=11.0` on 1.91.1. Never guess from the host
  or SDK.
- **Pass it everywhere.** Use `-mmacosx-version-min` for the Objective-C
  compile and the analyzer, and the matching Metal options for shader
  compilation. The installed `rerun-if-env-changed` directives stay.
- **Make link warnings fatal.** Make `linker_messages` a denying workspace lint
  for macOS targets. Prove that a deliberate target mismatch fails a link used
  by the full recipe; a warning with exit 0 is not enough.
- **Paths.** Pass paths as `Path`/`OsStr`, removing the UTF-8 unwraps. This is
  correct by construction, so there is no dedicated encoding test.

This establishes consistent build metadata. It does not claim runtime testing
on older macOS.

**Superseded default (2026-10-17, S11 / CK-22@r3).** The owner requires only the current Mac, so `.cargo/config.toml`
now forces `MACOSX_DEPLOYMENT_TARGET=26.0` (Cargo `[env]`, `force = true`). The resolution order above still applies;
the variable is now always set, so rustc's 11.0 default is no longer used. Shader line tables need 14.0 or later, so S11's
debug assertions are unconditional.

### 7.2 Editor (S2)

In `.vscode/settings.json`:

- **Rustup proxies.** Point rust-analyzer's `CARGO` and `RUSTC` at the
  proxies in `/opt/homebrew/opt/rustup/bin`, through
  `rust-analyzer.server.extraEnv`.
- **Clippy.** Set `check.command` to Clippy.
- **Separate target.** Set `cargo.targetDir: true`, so editor builds use their
  own directory and do not contend with terminal builds.

Confirm which binaries are invoked in the rust-analyzer log. The rules:

- **No launcher.** There are no launcher scripts and no wrapper around the
  editor's check command.
- **Homebrew Rust stays.** Do not remove it; behaviour is identical with or
  without it.
- **Global settings untouched.** Xcode selection stays project-local through
  `tools/ck`. Global shell configuration is left alone.

### 7.3 Rust lints and rustdoc (S8)

**Clippy:**

- Add the pedantic group at a lower priority.
- Add these rules: `undocumented_unsafe_blocks`,
  `multiple_unsafe_ops_per_block`, `allow_attributes`,
  `allow_attributes_without_reason`, `unwrap_used`, `dbg_macro` and `todo`.
- Leave `expect_used` and blanket `as_conversions` off. Use checked conversions
  where the enabled cast lints require them.
- Allow unwraps in tests through `clippy.toml`
  (`allow-unwrap-in-tests = true`), not scattered suppressions.

**Rustc lints:** `missing_docs`, `unreachable_pub` and
`unused_qualifications`.

**Rustdoc:** `cargo doc --workspace --no-deps --locked` with
`RUSTDOCFLAGS=-D warnings`, including `broken_intra_doc_links`. It runs in
fast; `cargo test` is not the documentation gate.

**Suppressions:** `#[expect(lint, reason = "...")]`, never `allow`. Public
API gets documentation, and every unsafe block gets a safety explanation.

**FFI error parsing.** Replace the error-string scan with
`CStr::from_bytes_until_nul` over a byte buffer. A buffer without a
terminator returns a bounded fallback error, never an unbounded read.
Pointers are cast only at the FFI boundary.

Fix the 14 measured findings. Do not suppress them to reach zero.

### 7.4 Shaders and profiles (S11)

**Shaders:**

- In debug-info builds, compile MSL with `-frecord-sources` and
  `-gline-tables-only`.
- Release shader artifacts carry no debug payload unless the profiling
  profile asks for it.
- A spec reads both metallibs' build settings to prove the difference.

**Profiles:**

- Add a `profiling` profile that inherits `release` with
  `debug = "line-tables-only"`, for Instruments.
- Workspace dev optimization stays at zero.

## 8. Rust executable and GPU specifications (S10)

- **CLI spec.** Add `apps/codekarl/tests/cli_spec.rs`, driven by
  `specs/bootstrap.json`. It locates the binary through
  `CARGO_BIN_EXE_codekarl`, and reports every failing case in one run. Each
  case has a 30 s timeout.
- **Dependency.** Add `serde_json` as a dev dependency, pinned through
  `Cargo.lock`. Runtime dependencies are unchanged, and the dependency
  wording in the docs is corrected.
- **The Metal case.** It is marked `requires: ["metal"]`.
  - Outside full-spec mode, a feature-limited run lists the cases it skipped
    in visible Cargo output.
  - Under `CK_REQUIRE_FULL_SPEC=1`, any skip, missing compiler, unavailable
    GPU or incomplete run fails.
- **Validation.** `MTL_DEBUG_LAYER=1` and `MTL_SHADER_VALIDATION=1` are set
  before tests spawn, and before any in-process device exists.
  - CLI stderr comparison ignores only the two recognized validation startup
    notices, including timestamp-prefixed forms. All other stderr is kept and
    tested.
- **Native GPU test.** It creates a `Device`, runs the probe and verifies all
  64 values. Each non-`Send` device stays inside its test function.
- **Probe removal (S10b).** After Phase A passes with both runners in one
  batch, remove `tools/probes.py`, the `cargo build` step and their
  references (Phase B). Python stays the language for workflow specs and
  future exploratory probes.

## 9. Hashing and large assets

**Streaming hashing (S9).** Hash files in bounded chunks, in both
`tools/pipeline.py` and the trampoline's `file_map`.

- Fingerprints and mode semantics stay byte-identical to today's.
- The before-and-after validation checks are kept.
- There is no mtime cache, and mutable captures are never trusted to skip
  reads.

**Large assets: deferred (§14).** Before committing substantial binaries,
benchmark representative files under the pinned Pijul version, then choose a
storage approach with the owner.

## 10. Garbage collection and retention

This is design for S12. Nothing is collected before S12 is accepted and its
first real dry run on this hub has been reviewed.

**Scope of the first version.** GC removes only state whose writers and
readers are enrolled in the lease protocol, plus trash:

- build output inside checkpoints that self-enroll (`target/` of a checkpoint
  that contains `tools/gate/lease.py`, §10.4);
- checkpoints published after S12 that are no longer needed.

**Report-only.** These are listed by the dry run with their size and the
reason, and are never removed:

- every code and docs capture;
- all checkpoint source published before S12;
- build output written before the protocol: the `target/` of every
  checkpoint without `tools/gate/lease.py`, and the legacy
  `.pipeline/target` cache.

The lifetime of a pre-protocol writer cannot be proved. A terminal marker
does not show that every CI descendant has exited, and a surviving Cargo or
build-script process writes by absolute path. It can reopen or recreate the
original `target/` after a rename, so moving it into trash is not safe.

This is the deliberately conservative choice. Reclaiming the report-only
items is deferred (§14), not required by decision 6.

### 10.1 Commands

- **`tools/ck gc`** is a dry run. It prints each candidate with its path,
  kind, size and reason, and each protected item with the rule that protects
  it. It deletes nothing and writes nothing.
- **`tools/ck gc --apply`** takes the locks itself, then recomputes the whole
  plan from scratch. It never reuses a dry run's output.

GC runs inside the checkpoint-pinned controller and requires a schema-2
pointer. It takes `state.lock`, then `meta.lock`, for planning and renames.

- It never takes or waits for `ci.lock`.
- It never waits on review.
- It never touches a caller's copy, so manual validation stays independent.

### 10.2 What GC may remove

The list is closed; nothing else is ever removed. Every candidate has two
distinct objects:

- the **lease object**, which GC locks;
- the **deletion object**, which GC renames into trash.

| Kind | Deletion object | Lease object | Collectible when |
| --- | --- | --- | --- |
| Superseded checkpoint | `.pipeline/checkpoints/<name>/`: `docs-D…`, `restore-…`, and batch candidates `<bid>-combined`, `<bid>-split-<i>`, `<bid>-split-final` | The same directory | It is lease-capable (rule 7), unprotected (§10.3), outside the window (rule 8), and its exclusive lease is free |
| Build output | `.pipeline/checkpoints/<name>/target/` | The enclosing checkpoint directory `.pipeline/checkpoints/<name>/`, never `target/` itself | The checkpoint contains `tools/gate/lease.py`, so every check run in it self-enrolls (§10.4). It is not in a non-terminal batch, and its exclusive lease is free. `target/` is ignored and outside the fingerprint, so the checkpoint still verifies afterwards. |
| Trash | `.pipeline/trash/*` | None needed | Always. These are leftovers from an interrupted `--apply`, and no reader can reach them. |

**Never removed:**

- **All captures.** These include:
  - code captures in `.pipeline/submissions/<id>/`;
  - docs captures in `.pipeline/docs/captures/<did>/`;
  - `<did>-resolve` copies.

  The reasons:
  - The bootstrap helper runs from `.pipeline/submissions/<ID>`. It checks
    its manifest fingerprint against that capture and imports
    `bootstrap_core` from it, and it holds the recovery entry point.
  - `Batch.eligible` treats submissions as accepted or dead by submission
    id. It then opens and fingerprints every other manifest's capture.
  - A live manifest's patches can all be accepted already through another
    submission, so "all hashes accepted" does not mean "published".
  - Review and recovery evidence names captures.

  Keeping every capture keeps every capture-based source check possible.
- **Checkpoint source published before S12** (rule 7).
- **Pre-protocol build output** (rule 7): the `target/` of every checkpoint
  without `tools/gate/lease.py`, and the legacy `.pipeline/target` cache.
  These are report-only.
- **Records:**
  - any JSON record, log or receipt;
  - the docs journal and docs entries;
  - `accepted-v2.json` and the `accepted.json` tombstone;
  - `gate.json`, the queue, quarantine and batch directories;
  - `.pipeline/bootstrap/`.
- **Other areas:**
  - `../codekarl-meta`;
  - `.pipeline/workers/`;
  - `coordination/` and the chat files;
  - `review-handoff.md`;
  - lock files and the signing identity;
  - `.tools/`.

A receipt whose `repo` or `candidate` was collected remains a valid record of
what was validated, and displays as "collected". Re-running the source check
against a collected checkpoint is no longer possible. A published state's
patches remain in every later accepted checkpoint, and its source stays
reproducible from them and the receipt's fingerprint.

### 10.3 Protected set

GC computes the protected set under its locks, from the gate's own records.
Paths named in metadata are used only to protect, never as deletion targets.

1. **The current pointer's repository.** It is read under `state.lock`. Only
   its `target/` can be a candidate, and see §10.4 for what happens to it in
   practice.
2. **Non-terminal batches.** A non-terminal batch has a directory without
   `published`, `failed`, `rejected` or `cancelled`. For each, GC protects
   every `<bid>-*` checkpoint, including its `target/`, and its base.
   - Docs queue while a batch is active, and publish requires the current
     pointer digest to equal `base_pointer_sha256`. So the base is the current
     repository.
   - Protection lasts through any review delay. CI and review run without
     `state.lock`, so they are protected by reference, not by lock.
   - A terminal marker does not prove that every writer has exited.
     `failed.json` is written while `batch prepare` still holds `ci.lock`,
     and a CI descendant can outlive its immediate parent. So rule 2 ends at
     the terminal marker, and the candidate lease (§10.4) covers what
     follows.
3. **All captures** (§10.2).
4. **Leased checkpoints** (§10.4). They are skipped and reported "in use".
5. **The recovery chain.** See the recovery conditions below.
6. **The bootstrap checkpoint.** `bootstrap-<bid>` predates S12, so rule 7
   keeps both its source and its `target/`.
7. **Pre-protocol checkpoints, permanently.** A checkpoint without
   `tools/gate/lease.py` predates S12, so its controller code takes no lease.
   - Its source directory is never collected. This covers every legacy
     `<ns>-<hash>`, `bootstrap-…`, and pre-S12 `docs-D…` or candidate
     checkpoint.
   - Its `target/` is never collected either. Its check runs did not
     enroll, so no lease can show that their descendants have exited. The
     legacy `.pipeline/target` cache is kept on the same ground.
   - The rule fails safe: if a later refactor moves that file, GC retains
     more, not less.
8. **The retention window.** This is convenience, not correctness.
   - GC keeps every checkpoint directory created in the last 7 days, using
     the `time_ns` embedded in every checkpoint name, plus the 3 newest of
     each kind.
   - This keeps recent states inspectable. It also keeps in-progress
     checkpoints whose creator died before referencing them; those have no
     reader to protect.
   - Correctness depends only on rules 1–7.

**Recovery conditions.** In any of these cases `--apply` refuses entirely,
names the recovery command, and still dry-runs:

- the pointer is missing, or is a hold or tombstone;
- a batch has `publishing.json` but no `published.json`;
- a bootstrap directory has neither `published.json` nor `restored.json`;
- a docs journal record has entries that are not all published;
- any record that defines the protected set fails to parse.

Those cases need the docs-descendant chain that recovery walks. Refusing
avoids reconstructing that chain.

### 10.4 Readers and race prevention

GC protects **enrolled readers** only:

- the S12 controller and its children;
- `worker` and `sync` sources;
- the gate's own batch and recovery flows, through §10.3.

Some processes are not enrolled and are not protected: commands someone
starts by hand inside `.pipeline/checkpoints/<name>`, such as a shell, an
editor, `cargo` or `pijul` there. Checkpoints are not working directories
(`AGENTS.md`: do not edit `.pipeline/checkpoints/`). The only sanctioned Cargo
runs inside checkpoints are batch and bootstrap candidates. Rules 2 and 6
and the candidate lease below cover those.

| Reader | What it reads | Protection |
| --- | --- | --- |
| **Checkpoint-pinned controller** (see below) | Its own checkpoint, for the command's whole lifetime, including its children | A shared lease that its children inherit. Pre-lease source is retained (rule 7). |
| `worker` | Clones the pointer's repository, without a project lock | A shared lease on the source, inherited by the `pijul clone` child. If the source was collected before the lease, it re-reads the pointer and retries up to 3 times. A failed clone removes only the destination it created. |
| `sync` | Pulls from the pointer's repository, without a project lock | The same lease, inherited by the `pijul pull` child. Pull is idempotent, because accepted hashes only grow, so a retry is safe. |
| `status`, `issue list` | The pointer and the meta repository | Nothing GC removes |
| `submit`, reviewers, `eligible`, the bootstrap helper | Captures | Never collected (rule 3) |
| `batch prepare`: CI under `ci` only | The base, member captures, candidates and their `target/` | Rules 1–3 while non-terminal. Afterwards: the candidate's self-enrolled lease, which every CI descendant inherits, or rule 7 for a pre-protocol candidate. |
| Integration review wait | The candidate and CI receipt | Rule 2, through `validated` and `approved` |
| `batch publish`, integration verdicts | The candidate, receipt, captures and reviews | Excluded by `state.lock`, and rules 2 and 3 |
| Publish recovery, journal recovery, bootstrap recovery: `is_docs_descendant` walks | Every checkpoint on the chain | `--apply` refuses while any recovery is possible |
| `docs` capture, flush and resolve | The current checkpoint, captures and resolve copies | Excluded by `state.lock`, and rules 1 and 3 |
| Manual full check, fast, Cargo and editor | The caller's copy only | Not a GC target |
| The metadata repository | — | Never a GC target |

**The checkpoint-pinned controller.**

1. `tools/ck` reads the pointer.
2. It verifies the checkpoint with `pijul status`, `pijul log` and `file_map`.
3. It `exec`s a launcher with `sys.path` set to `<checkpoint>/tools`.
4. The launcher imports `environment`, `pipeline` and `gate.cli`, and
   origin-checks them.
5. Handlers then import lazily: `pipeline`, `check`, `provenance`, `batch`,
   `docsqueue`, `locks` and `pointer`.

The controller lives as long as its command. For `batch prepare` that
includes the whole CI run, during which it holds no `state.lock`. Neither
project locks nor a time interval can show that it has finished.

**The lease (S12).** This is a per-checkpoint `flock` on the checkpoint
directory's own descriptor. It needs no new file, so checkpoint cleanliness
and fingerprints are unchanged.

- **The lease object** is always the checkpoint directory
  `.pipeline/checkpoints/<name>`, even when the deletion object is its
  `target/`.
- **Verified on macOS for this plan:**
  - an exclusive lock is refused while another descriptor holds a shared one;
  - a renamed directory keeps its inode;
  - a lock inherited by a child and a grandchild stays held after the parent
    is killed, and is released when the last descendant exits.

How each party uses it:

- **Controller.** First thing in `gate.cli.main`, before dispatch and before
  any project lock:
  1. Open the directory that contains its real `tools` path.
  2. Take `LOCK_SH`, polling nonblocking for up to 5 s.
  3. Confirm that `stat(path).st_ino == fstat(fd).st_ino`.
  4. On failure, exit with "controller checkpoint was collected; rerun the
     command" before touching any state.
- **Inheritance.** Every lease descriptor is inheritable. S12 routes every
  subprocess the controller starts through one spawn helper, including
  `pipeline.pijul`, the check runs, `ssh-agent` and `git`. The helper adds
  all held lease descriptors to `pass_fds`. Today `subprocess.run` closes
  them by default.
  - A Pijul clone, pull, status or log child that outlives a killed parent
    therefore keeps the lease until it exits.
  - On orderly exit or interruption, the controller kills and reaps its
    children's process group before closing its own descriptor, as in §4.7.
  - A child that deliberately closes inherited descriptors would drop its
    share. Pijul does not; the S12 interruption test proves this for the
    real binary.
- **Candidates self-enroll.** The candidate's own `tools/check.py` takes
  the lease, not the controller that launched it.
  - **Taking it.** As its first action, before any subprocess, `check.py`
    opens its own `ROOT` directory and takes `LOCK_SH`. It confirms
    `stat(ROOT).st_ino == fstat(fd).st_ino`. On failure it exits nonzero
    before running any step.
  - **Passing it on.** `run()` adds that descriptor to every step's
    `pass_fds`, beside `CK_CI_LOCK_FD`. Cargo, rustc, build scripts, test
    binaries and probes therefore all hold the candidate lease.
  - **Manual runs** also lease their copy. That is harmless, because GC
    never targets copies.
- **Why no transition gap.** Enrollment depends only on the code in the
  candidate, not on the controller's version.
  - A pre-S12 controller still runs `<candidate>/tools/check.py`. That
    includes the batch that installs S12, whose `_full_check` is the
    installed code.
  - Any candidate containing S12 therefore enrolls. S12 lands `lease.py` and
    this `check.py` change in one submission, so `lease.py` marks exactly
    the checkpoints whose checks enroll.
- **Before the lease.** The only process in the candidate is `check.py`
  itself, which has started no writer yet.
  - The batch is still non-terminal (rule 2). The controller records a
    result only after `check.py` returns.
  - If the controller dies, interruption recovery cannot mark the batch
    `interrupted`. It needs `ci.lock` nonblocking, and `check.py` still
    holds the inherited CI descriptor.
- **After the lease.** Every writer that `check.py` starts holds the lease.
  GC skips the candidate, and its `target/`, while any of them is alive. That
  holds even after a terminal marker and after `batch prepare` has released
  its own `ci.lock`.
- **Pre-protocol candidates.** These are the bootstrap checkpoint and legacy
  candidates, whose `check.py` does not enroll.
  - There is no lease to test, so their `target/` stays report-only (rule
    7).
  - Their terminal markers are not treated as proof that no writer is
    alive.
- **`worker` and `sync`.** They lease their source in the same way, before
  starting the Pijul child, and pass that lease to it. The source is the
  pointer's repository, which can differ from the controller's checkpoint
  if the pointer advanced meanwhile.
- **GC.** For each candidate:
  1. Open the **lease object**.
  2. Try `LOCK_EX | LOCK_NB`. If the lease is held, skip the candidate and
     report it "in use".
  3. Rename the **deletion object** into trash while holding the exclusive
     lease.
  4. Release the lease.

**GC and its own checkpoint.** GC is itself a controller, so it holds a
shared lease on the current checkpoint while it runs.

- **What happens.** Its own exclusive attempt on that directory uses a
  separate descriptor, so it is refused. `flock` conflicts between open file
  descriptions even within one process; this was verified.
- **The result.** The current checkpoint's `target/` is always reported "in
  use by this gc" and is never collected by that run. It becomes collectible
  in a later run, after the pointer has moved on.
- The same happens for any checkpoint that a still-running command was
  launched from, for example a long `batch prepare`.

**Why the gaps are safe:**

- **Before the lease.** The gap is verify, `exec`, the initial imports and
  `cli.main`'s lease acquisition. The rename is atomic, so a reader there
  sees either the whole directory or none of it.
  - A missing directory fails `pijul`, the import, the launcher's origin
    check (which also rejects a same-named module found elsewhere on
    `sys.path`), or the lease's open and inode check.
  - Before the lease, the launcher only sets environment variables and
    starts the signing agent, so each failure happens before any project
    state changes.
- **After the lease**, lazy imports cannot lose their files, and inherited
  descriptors cover children that outlive the controller.
- **No deadlock.** GC never waits for a lease. A controller takes its lease
  before any project lock, so it never waits on a lease while holding one.

**Rollout.** No process scan is used as an upgrade barrier. A stale
trampoline may have read an old pointer but not yet `exec`ed, and its argv
names only `ck`, so no scan is race-free.

- **Pre-S12 controllers.** These run code from pre-S12 checkpoints, which
  take no lease. Rule 7 never collects those checkpoints' source, so any
  such controller, started at any time, keeps its files. Retention age is not
  relied on.
- **Stale trampolines.** An old `tools/ck` in an unsynced copy reads the
  pointer and verifies the checkpoint itself, then `exec`s its own launcher
  string.
  - That launcher imports `gate.cli` from the selected checkpoint and calls
    `cli.main(root, hub)`. A post-S12 checkpoint therefore takes its lease
    whatever the trampoline's age.
  - A post-S12 checkpoint collected in its pre-lease gap fails as described
    above.
- **What S12 and later changes must keep:**
  - pointer schema 2 (the installed `pointer()` rejects only the hold and
    tombstone schemas);
  - the `cli.main(root, hub)` entry;
  - `lease.py` in its place;
  - a launcher that does nothing before `cli.main` beyond environment setup
    and the signing agent.

  A future change that breaks any of these needs its own rollout review.

### 10.5 Apply mechanism and safety

1. **Locks and plan.** Take `state.lock`, then `meta.lock`. Recompute the plan
   and check the recovery conditions.
2. **Rename.** For each candidate:
   1. Validate both paths.
   2. Take the exclusive lease on the lease object.
   3. `rename` the deletion object into
      `.pipeline/trash/<ns>-<kind>-<name>`.
   4. Release the lease.
3. **Release the locks.**
4. **Delete.** Remove the trash entries. No reference or enrolled reader can
   reach them.

**Path checks:**

- Every path is built from a directory listing, never from a metadata
  string:
  - `<hub>/.pipeline/checkpoints/<name>`, optionally `/target`;
  - `<hub>/.pipeline/trash/<entry>`.
- `name` must match the checkpoint naming patterns.
- `lstat` must show a real directory, not a symlink, for both the lease
  object and the deletion object.
- The `realpath` of each must be inside the hub's `.pipeline`.
- Trash must be on the same device (`st_dev`), so the rename is atomic.

**Deletion:**

- `shutil.rmtree` is used only when `shutil.rmtree.avoids_symlink_attacks` is
  true; otherwise GC refuses.
- `ENOENT` is tolerated, so two cleanups of the same trash are harmless.

**Interruption.** A crash between rename and deletion leaves trash. The next
run reports it. `--apply` removes it first, and needs no lock to do so.

### 10.6 Defaults and tradeoffs

- **All captures are kept.** They total about 25 MB today.
  - Dead captures hold the only copy of their patches.
  - Live captures, and the bootstrap helper's capture, are executed or
    re-verified.
  - Collecting captures would need its own design, against `eligible`, the
    bootstrap helper and review evidence (deferred, §14).
- **Everything existing today is retained.** The first `--apply` reclaims
  essentially nothing, apart from any trash. On today's figures it keeps
  about 103 MB:
  - 47 MB of pre-S12 checkpoint source;
  - 14 MB of pre-protocol checkpoint `target/`;
  - the 17 MB legacy cache;
  - 25 MB of captures.

  The dry run reports all of it with sizes. That is the stated tradeoff for
  never deleting under a writer whose lifetime cannot be proved.
- **Savings start with S12.** Checkpoints published after S12, and the build
  output of their enrolled checks, become collectible once they are terminal,
  outside the window and unleased. Most of the future growth is there.
- **The window is 7 days plus the 3 newest of each kind.** That is roughly a
  working week of rollback inspection.
- **Workers are never automatic targets.** The dry run lists workers that are
  clean and fully accepted, for the owner to delete by hand.
- **Logs and JSON are kept.** They total about 1 MB today.

## 11. Agent guidance and documentation

**S0, now: fix the stale record line.** Replace "Record with `tools/ck pijul
record`" in `AGENTS.md` with `ck record`. This is a gated code submission,
because `AGENTS.md` is executable Markdown.

**S13: final guidance**, once the commands are settled.

- **`AGENTS.md`:**
  - the check levels and when to use each;
  - the distinction between checked locally, submitted and accepted;
  - the suppression and comment policy;
  - actionable recovery for lock, dirty-copy, quarantine, stale-tooling,
    malformed-state and collected-checkpoint errors.

  Keep it short and link details.
- **Pijul equivalents:** status, diff, log, change, credit, add, move, remove
  and reset.
  - `pijul remove` untracks a path but keeps the file.
  - A plain filesystem move shows as a deletion plus an untracked file.
  - Submitted and accepted patches are never rewritten.
- **`CLAUDE.md`:** a regular file containing `@AGENTS.md`, without claiming
  that Claude cannot read `AGENTS.md`.
- **Ordinary docs**, published through `tools/ck docs` with each slice:
  - `README.md` and `docs/tooling.md`;
  - gate plan §7.2 and the §9 lock table, which still list standalone checks
    as `ci.lock` holders; this update ships with S3.
- **One concern per patch.** Use separate submissions for independent work;
  the splitter isolates submission groups, not single patches.

## 12. Implementation slices

**All slices below are accepted** (§3.2 lists their batches). The table keeps the original contracts; S1's 11.0 default
was later superseded by S11's explicit 26.0 target.

Every slice is one gated submission, or a few, implemented in an isolated
worker with opposite-brand review. Independent reviewed submissions may share
a batch; no slice requires a standalone full CI run. Each new check is proved
by §13.1 before its slice is accepted.

| Slice | Content | Depends on | Acceptance (observable) |
| --- | --- | --- | --- |
| **G1** | **Done** (§3.1): `ck sync` pulls the live docs queue (gate plan §8.2, §8.4) | — | Accepted in batch `1792036793934196000` |
| **L1** | **Done** (§3.1, gate plan §2.4): separate current-role readiness from attributable work history, so an owner's in-session model switch is handled and the reconciliation is recorded | — (precedes the pilot) | A spec approved by an opposite-brand review comes first. A session whose current pair is off class is still refused. Earlier off-class work keeps its own model in authorship records. Mixed-family and unknown histories still cannot satisfy opposite-brand review. The 182 tests pass. |
| **S0** | `AGENTS.md` record line (§11) | — | `AGENTS.md` names `ck record`; no other line changes |
| **S1** | Deployment target and fatal linker messages (§7.1) | — | Native object, Metal library and Rust link agree on 11.0 by default and under an explicit override (`vtool -show-build`/`otool -l`). A deliberate mismatch fails the full recipe. |
| **S2** | Editor configuration (§7.2) | — | The rust-analyzer log shows rustup-proxy Cargo and rustc, Clippy diagnostics and a separate target directory. No launcher script. |
| **S3** | Check levels: `--fast`, buffered output, manual full off `ci.lock`, build isolation, gate plan §7.2/§9 docs (§4.1, §4.2 Phase A, §4.6 output, §4.7) | — | Every §13.2 manual-independence case passes. A second `batch prepare` is still rejected. Fast runs in two workers while a batch holds the slot. A passing suite prints no per-test lines; a failing one prints its output. The 182 tests pass, with only CI-slot assertions changed. |
| **S4** | Lock holder metadata, step watchdog, infrastructure failure class (§4.6, §5) | S3 | A state-lock timeout names the holder's pid, command and age. A hung step exits 75. `batch prepare` records `infrastructure` without splitting or quarantining, and the members are re-eligible. |
| **S5** | Tools: `shellcheck-py`, mypy pins, doctor enforcement, ShellCheck discovery in quick, `CK_NATIVE_ANALYZE` in `build.rs` (§4.3–§4.5) | S1, S3 | A fresh `tools/bootstrap` installs every tool. Doctor fails on a wrong version. A controlled shell defect in `tools/bootstrap` and a controlled analyzer finding in `bridge.m` each fail the recipe. The discovery guard fails if `tools/bootstrap` is not covered. |
| **S6** | Ruff configuration, discovery and fixture exception, and the 139 non-`ANN` fixes (§6.1) | — | Bare `ruff check` covers `tools/ck` and `tools/ck-chat`, and skips `.pipeline` and `.tools`. Zero non-`ANN` findings, with every remaining `noqa` coded and explained. Error handling is unchanged, and the 182 tests pass. May split by area into separate submissions. |
| **S7a–e** | `ANN` and strict mypy with typed boundaries (§6.2, §6.3) | S5, S6 (S7c also S9) | Each submission empties its part of the temporary list. Every legacy and v2 record variant loads, in tests. A deliberate type error in the executed entry point fails. At the end of S7e the list and its reason are gone. |
| **S8** | Clippy pedantic and safety rules, rustc lints, rustdoc in fast, bounded FFI parsing (§7.3) | S1, S3 | Zero findings, with every `expect` explained. A broken intra-doc link and an undocumented unsafe block each fail fast. A nonterminated error buffer yields the bounded error. |
| **S9** | Streaming hashing (§9) | — | Fingerprints are byte-identical across empty, small and larger-than-chunk files. Peak memory stays bounded for a large file. |
| **S10** | Rust CLI and GPU specs (§8) | S3, S8 | A Phase A batch passes with both runners. Multiple failures are reported. Binaries are found under other target directories and profiles. Full-spec mode fails on a skip. Validation is active before device creation. |
| **S10b** | Remove `tools/probes.py` and the `cargo build` step (Phase B) | S10 accepted | The full recipe matches §4.2 Phase B, and Metal still runs in batch CI. |
| **S11** | Debug shaders and profiling profile (§7.4) | S1 | Debug metallibs carry the source and line tables; release ones carry neither. `cargo build --profile profiling` works. |
| **S12** | GC: checkpoint leases inherited by every controller child, candidate self-enrollment in `check.py` (landed with `lease.py`), `worker`/`sync` source leases and retry, `ck gc` (§10) | S3 | Every §13.3 case passes. A real dry run against this hub is reviewed before the first `--apply`. |
| **S13** | Final guidance and docs (§11) | S3, S5, S10b, S12 | `AGENTS.md`, `CLAUDE.md` and `docs/tooling.md` match the shipped commands. |

**Order.** G1 and L1 are done. L1 was the pilot blocker's fix, chosen by the
owner. Live pilot execution is complete, with open workflow findings (§3.2).
The owner's `ck-workflow-ready` goal now authorizes this sequence. First come
the PF repairs and RC-1 (the chat suite in batch CI), then the batches below.
The order is:

1. **First batch:**
   - S3, check levels and manual full checks off `ci.lock`;
   - S0, the small `AGENTS.md` correction.
2. **Second batch:** S1, S2, S9 and S6. These are independent and may share
   that batch.
3. S4, S5 and S8.
4. S7a–e, S10, S11 and S12.
5. S10b and S13.

Each batch is reviewable on its own, and no slice needs a standalone full CI
run.

**Shared files.** S1, S5 and S11 all edit `build.rs`, so they go in that
order to avoid conflicts. S7c follows S9 because both edit
`tools/pipeline.py`.

## 13. Verification

### 13.1 Negative proofs

Each new check gets a controlled violation in a disposable worker. The
violation must fail the applicable recipe for the intended reason, and is
removed before submission. Record the check, the violation, the command, the
exit status and the diagnostic. The violations:

- an analyzer finding;
- a shell defect in `tools/bootstrap`;
- a broken rustdoc link;
- a type error;
- a wrong deployment target;
- each new Ruff group and Clippy rule.

A check that only warns has not been proved.

### 13.2 Behaviour

**Batches:**

- Two independent submissions pass in one combined run.
- Later submissions stay queued, and dependency groups stay intact.
- A failed batch isolates unrelated work, without blindly combining
  separately passing states.
- Contention and timeouts never count as patch failures.

**Manual independence (decision 4).** Hold a batch validation open, then
start a manual full check.

- It starts at once, without `ci.lock`.
- Its output, log and any evidence stay separate.
- It cannot create a receipt.
- Repeat with the same inherited `CARGO_TARGET_DIR`, then
  `CARGO_BUILD_BUILD_DIR`, then each config equivalent. Neither run waits on
  the other's build directory, or overwrites its artifacts.
- Two concurrent manual runs take different slots.
- **Interruption.** Kill a manual run's `check.py` with SIGKILL while its
  Cargo build or test step is running, then start another manual run.
  - The new run takes a different slot.
  - Slot 0's lock stays held until the orphaned Cargo descendants exit.
  - After they exit, a later run reuses slot 0.
  - Ctrl-C or the watchdog kill and reap the step's process group before the
    slot is released.
- Waits on the package-cache lock are acceptable.

**Descriptors.** Invalid inherited descriptors fail at once. Children keep CI
ownership after the parent is interrupted.

**Docs.** Docs publish during a manual check, queue during a batch, and flush
after it.

**Compatibility.**

- The 182 workflow tests pass, with only CI-slot assertions changed.
- All legacy record variants load beside the v2 pointer.
- Subprocess failure handling keeps its meaning.

### 13.3 GC

- **Dry runs** delete and write nothing.
- **`--apply`:**
  - never removes the current checkpoint's source, anything of a
    non-terminal batch (including one validating without `state.lock`), any
    code or docs capture (live, dead, published or the bootstrap helper's),
    a resolve copy, or any JSON or log;
  - after a full `--apply`, the bootstrap helper and `batch prepare`'s
    `eligible` scan still run from their captures;
  - refuses under each recovery condition.
- **Controllers:**
  - A leased checkpoint is skipped, and so is its `target/`: the lease is
    taken on the checkpoint directory, not on `target/`.
  - GC's own current checkpoint `target/` is reported "in use by this gc"
    and kept. A later run, after the pointer moves, collects it.
  - A controller blocked between its imports and a lazy import, while GC runs
    (simulated with a test hook), completes after GC.
  - A controller whose checkpoint is renamed before its lease exits with the
    rerun message, and changes no state.
  - A checkpoint without `tools/gate/lease.py` keeps both its source and its
    `target/`, and the legacy `.pipeline/target` is kept. All are reported
    with their sizes, but never renamed.
  - A stale trampoline, from a copy synced before S12, runs a post-S12
    checkpoint and takes its lease.
- **Readers:** `worker` and `sync` racing a collection either succeed from the
  current pointer, or fail cleanly with no half-written destination.
- **Inherited leases.** Kill a controller with SIGKILL while its Pijul child
  is reading the checkpoint (a `worker` clone, a `sync` pull, and a
  controller `pijul log`, each held open with a test hook).
  - GC reports that checkpoint "in use" until the child exits.
  - GC collects it only afterwards.
- **Path safety:** symlinked, escaping or malformed paths are refused.
- **Surviving CI descendants.** A candidate's check fails while a test-hook
  descendant keeps writing to the candidate's `target/`.
  - The controller records `failed` and closes its own `ci.lock` descriptor. The
    descendant inherited `CK_CI_LOCK_FD`, so the flock stays held until it exits
    (no next batch while a stray process still writes into a candidate).
  - GC reports the candidate "in use" and keeps it, together with its
    `target/`, until the descendant exits.
  - Only then is it collectible.
- **Enrollment under an old controller.** Run an S12 candidate's `check.py`
  through the installed, pre-S12 `_full_check`.
  - The candidate lease is held from `check.py`'s first step to the last
    descendant's exit.
  - A candidate whose directory was renamed before `check.py` starts makes
    it exit nonzero before any step runs.
- **Interruption:** killing GC between rename and deletion leaves trash, and
  the next run finishes it. A trash entry that a writer is still filling is
  retried later, not reported as a failure.
- **After collection**, all of these still work:
  - prepare, review and publish;
  - docs;
  - `worker`, `sync` and `status`;
  - `submit --supersedes`.

  Receipts still load, and show collected paths.

### 13.4 Completion report

Delivered as `.pipeline/coordination/workflow-ready/final-readiness-report-20261017.md`. The report lists:

- the accepted batches and receipt paths;
- the negative proofs;
- every global lint exclusion, with its reason;
- the deferred decisions (§14).

It does not claim older-macOS runtime coverage, or legacy audit links that
were never established.

## 14. Concern accounting and deferred topics

Every slice named in the table is accepted (§3.2), so each disposition below is complete.

| Original concern | Disposition |
| --- | --- |
| Native object targets macOS 26.0 while Rust links for 11.0 | S1 |
| rust-analyzer invokes Homebrew Cargo | S2 |
| Full checks, docs and acceptance share one fail-fast lock | **Done by the gate**: `ci`, `state` and `meta` locks, with docs queued. Manual checks come off the slot in S3. Holder diagnostics are S4. |
| Successful tests print apparent failures | S3 |
| Bare Ruff misses `tools/ck`; Python state is untyped | S6 and S7 |
| Extra lint findings (now 872 Ruff, 14 Rust) | S6, S7 and S8, with no blanket suppression |
| Cargo runs zero Rust tests | S10 |
| The Python probe assumes a debug executable path | S10 (`CARGO_BIN_EXE_codekarl`), then S10b removes the runner |
| Unsafe error-string scan | S8 |
| Native analyzer findings can exit successfully | S5 (`-analyzer-werror`, verified) |
| Shell scripts unchecked | S5 (ShellCheck discovery covers `tools/bootstrap`) |
| No retention policy | S12 |
| `ck sync` omits live queued docs (verified by probe, §3) | **Done**: G1 (§3.1) |
| Threaded `mock.patch` leaks fixture authors in `test_c05` | **Done**: provenance repair (§3.1) |
| An in-session model switch permanently fails `ck session` (§3.2) | **Done**: L1 (§3.1) |
| Mixed-family or unknown histories are not checked against the current class | **Done**: PF-12 by CK-9 (batch `1792115313013817000`, 2026-10-16) |
| Stale queued notices reach a new session (§3.2) | Consolidated pilot PF-08; S13 guidance; shared transport work has separate ownership |
| Unknown chat bracket kinds leave the outstanding view (F7) | Consolidated pilot PF-04; reproduced offline during analysis, not exercised live |
| Pilot findings D1 and both O4 findings, plus O1–O5 (§3.2) | See the single PF-01–PF-12 ledger in docs/archive/live-pilot-findings.md; verified facts, hypotheses and proposals are distinguished |
| Repeated whole-file hashing | S9 |
| Receipts lack a parent chain | **Done by the gate**: the v2 pointer has `receipt` and `previous`, and the legacy boundary is `superseded_legacy` |
| Agent instructions and Git/Pijul mappings | S0 and S13 |
| `ck accept` and the docs-lane lock model | **Done by the gate**: batch prepare and publish, and the docs queue |
| Splitter granularity wording | S13 |
| Debug shader information; profiling profile | S11 |
| `REVIEW-HANDOFF.md` handling | **Done**: relocated to `.pipeline/review-handoff.md`. No slice touches it. |

**Deferred by the owner:**

- **Rust toolchain upgrade.** Keep 1.91.1, and review an upgrade separately.
- **Removing Homebrew Rust.** The project is correct either way.
- **Blanket dependency `opt-level = 3`.** Wait until real dependencies and
  measurements justify it.
- **Large-asset storage.** Benchmark first, then decide with the owner.
- **Deleting captures, pre-protocol state and workers.** Code and docs
  captures, published or dead, pre-S12 checkpoint source, pre-protocol
  `target/` and the legacy cache are all kept by the first GC. Reclaiming
  them would be a separate, owner-driven design and command, reviewed against
  `eligible`, the bootstrap helper and review evidence.
- **Older macOS support and testing** (owner, 2026-10-17: development targets the current Mac only; S11 sets 26.0).

**Deferred by the owner on 2026-10-17** (`.pipeline/coordination/workflow-ready/owner-closeout-decisions-20261017.md`;
known limitations, not open work):

- **Stopping a busy Codex turn through chat** (pilot D2): reproducer kept, Plateia escalation and fix deferred; manual
  interruption is the fallback.
- **Remaining live recovery coverage** (PF-09): IRC outage and outbox replay, restart recovery beyond the 2026-10-17 smoke
  check, reminder and dead-letter timing, phone push. See `docs/archive/live-pilot-findings.md`.

## 15. Sources

- `docs/review-gate-plan.md`, which is authoritative for publication, locks,
  docs and pointers.
- [Clang Static Analyzer](https://clang.llvm.org/docs/ClangStaticAnalyzer.html)
  and [ShellCheck](https://github.com/koalaman/shellcheck).
- [Rust macOS deployment target](https://doc.rust-lang.org/rustc/platform-support/apple-darwin.html),
  [rust-analyzer configuration](https://rust-analyzer.github.io/book/configuration.html)
  and [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html).
- Cargo 1.91.1 locking ([layout][cargo-layout] and
  [cache lock][cargo-cache-lock]) and
  [build-script outputs][cargo-build-scripts].
- [Claude Code and AGENTS.md](https://code.claude.com/docs/en/memory#agents-md)
  and [Pijul's FAQ on large files](https://pijul.org/faq/).
- Local evidence:
  - `.pipeline/coordination/build-concurrency-2026-10-14/`;
  - the bootstrap CI log `.pipeline/checkpoints/bootstrap-1792012417088242000.log`;
  - `.pipeline/coordination/gate-install-result.md`;
  - `.pipeline/coordination/copilot-provenance-repair-result.md` and
    `.pipeline/coordination/g1-reviewed-fix-result.md`;
  - `.pipeline/coordination/live-pilot-preflight.md` and
    `.pipeline/coordination/live-pilot-progress.md`.
