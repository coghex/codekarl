# Tooling decisions and recovery

## What Pijul buys us

Pijul records changes with identities and explicit dependencies. Independent
changes can commute without being rewritten to new change identities after each
integration. It can still report conflicts, and changes without textual
conflicts can still interact incorrectly. Combined-state checks establish an
accepted result.

This project uses local Pijul repositories only. It pins beta.24 and tests the
CLI behaviors it relies on. Upgrading Pijul is a deliberate tooling change.
Use its default Cargo features when installing; beta.24's no-default-features
build failed in its log implementation during setup.

## The review-gated acceptance transaction

`ck issue new` creates a versioned issue specification in the sibling metadata
repository. A spec must receive fresh, independent Class A/high review from the
opposite model family before its issue is ready. Contract or acceptance
criterion edits create a new revision and make the old receipt stale.

`ck record -m MESSAGE --issue CK-N[,CK-M]` records code with immutable,
session-verified provenance. Once activated, `ck pijul record` is disabled;
raw Pijul changes without provenance are treated as unknown authorship and need
both reviewer families.

`ck submit --issue CK-N --spec DESCRIPTION` captures a clean worker state and
binds its issue revisions, patch closure, files, authors and fingerprint in a
manifest. Reviews bind that exact manifest and fingerprint. A revision must
explicitly supersede its predecessor; previous approval does not transfer to
changed code. Unknown, human or mixed-family authorship needs approval from
both families, each by a reviewer independent of every author.

`ck batch prepare` freezes only eligible submissions, builds their combined
candidate, and runs the full checks once. Independent groups may be split after
a failed combined run; each passing subset is a new candidate. The CI lock is
released while the validated batch waits for integration review. Changed
membership or code needs fresh CI and integration approval. `ck batch publish`
rechecks the base, manifests, captures, reviews and receipts under the state
and metadata locks before atomically replacing the accepted pointer.

**Single-submission batches (CK-38).** If the batch has exactly one member and its candidate is byte-identical to the
approved capture, `ck batch publish` needs no separate integration review. Under the publication locks it requires the
member to be eligible with fresh AI submission coverage (an owner sign-off alone does not count), the base and patches to
match, CI to have passed, and the CI log, bound at prepare, to show every step at least 10 times inside the run's own
watchdog. Otherwise it refuses and asks for an integration review. The pointer records `basis: single-submission` with the
submission receipts and the timing evidence. `ck batch status BID` shows the expected basis beforehand; it is advisory.

Only batch validation holds the project CI lock, so one batch validates at a time;
a manual `ck check` holds no project lock, and a manual full check builds in its
own `target/ck-full/<n>` slot.
Cargo may use several compiler threads within a check. This is a local trusted-code workflow, not a
sandbox for hostile patches. Code, tests and build scripts execute as your
user. Integrity checks prevent accidental state drift, not intentional bypass
by an actor who can edit the gate or its state directory.

## Check levels and timeouts

The checks are a catalog of groups in `tools/check.py` (CK-77, modelled on the owner's moskophoros validation). Each group has an id, a
category (`floor`, `affected`, `optional`), its exact commands and the paths that affect it, and a class: plain, GPU, or taking the
desktop's focus (it opens real windows). A group's class covers every command it starts, nested checks included.

- **`ck check --changed`** (the planned check, used while working and before recording): compares the copy with the accepted base and
  runs the floor groups plus every affected group a changed path matches, printing every group as run or skip with the reason. An
  unclaimed path or a change set it cannot compute runs every affected group (it fails wide); a new untracked directory counts as touching
  the directory itself. `--group ID` adds a group, `--plan` prints the plan without running it. The window groups of one backend are
  selected only by that backend's own paths, so work on one backend never opens another's windows. A planned run calls itself
  `recipe planned` and is never batch evidence.
- **Guards that read other files** (CK-84, from the milestone 3.5 guide): a test or build step that reads files outside its own group's
  paths, such as source scans, `unsafe` and symbol guards, hook-isolation scans, or native test support compiled by another crate, is
  listed in the catalog's guard inventory, and the files it reads select its group. Static guards that need no GPU or window run in their
  own plain groups (`metal.isolation`, `vulkan.isolation`, `unittest-catalog`), so an application edit runs them without opening a Metal
  or native window. A catalog test fails when a guard reads an unregistered input.
- **`ck check`** (full): every group once, with `CK_REQUIRE_FULL_SPEC=1` for the all-features Rust groups so a skipped or unavailable
  Metal case fails. **The batch always runs this level**, so everything, Metal and native windows included, runs once per batch.
- `--quick` runs formatting, Ruff and ShellCheck; `--fast` runs the full level's groups without the build slot and without
  `CK_REQUIRE_FULL_SPEC`.

Each step has a 45-minute watchdog, kept at least 10 times the slowest step measured in a batch log; when it fires the check kills the
step's process group and exits 75, which the gate treats as an infrastructure failure rather than a patch failure. The exact catalog
(groups, paths, commands, classes) is in `tools/check.py` and its worker report,
`.pipeline/coordination/runtime-m3/ck77-worker-result.md`.

## Garbage collection

`ck gc` is a dry run that classifies every checkpoint, build output directory,
capture and record as a candidate, in use, protected or report-only, and
removes nothing. `ck gc --apply` recomputes that plan under the state and
metadata locks, takes each candidate's exclusive checkpoint lease, moves it to
`.pipeline/trash/` and then deletes it. It removes only build output and
superseded lease-capable checkpoints outside the retention window, and refuses
while any recovery condition exists (an unfinished publication, an unreadable
record), naming the command that resolves it. Captures, records, the current
checkpoint and pre-lease checkpoints are never removed; worker copies are never
listed. On this hub only the manager or owner runs it, and `--apply` only after
a reviewed dry run.

## Failed batches

Read `ck batch status BID`, the candidate log and the structural conflict
report. A failed subset is quarantined. A dependent corrective submission must
explicitly supersede the affected live submission and include its prerequisite
patches. It receives new review and full validation. Independent submissions
and their approvals remain eligible when their exact manifests are unchanged.

An integration rejection is recorded with `ck review batch BID --verdict
rejected --report FILE --blame CK-SUBMISSION...`. Only blamed submissions are
quarantined; docs queued during the batch drain onto the unchanged accepted
code. Cancellation is explicit with `ck batch cancel BID --reason REASON`.
Nothing publishes after validation alone. The removed `ck accept` command
refuses and directs the operator to prepare, review and publish.

## Ordinary documentation

`ck docs --from COPY -m MESSAGE README.md docs/example.md` records ordinary
documentation as a real Pijul patch. It publishes immediately when no code
batch is active and otherwise queues the patch. After code publication, queued
documentation drains in FIFO order onto the newly accepted code. If validation
fails, a batch is cancelled or integration rejects it, queued docs drain onto
the unchanged code.

Conflicted and dependency-blocked docs remain visible in the queue and are
never dropped. `ck docs resolve DID` creates a working copy with the recorded
conflict. Resolve the documentation there, then run `ck docs resolve DID
--record -m MESSAGE`; the resulting resolution is queued with its original
patch as one group. `ck sync` updates a clean working copy; it refuses to
discard unaccepted code changes.

## Interrupted commands and recovery

Batch prepare, review and publish are separate durable operations. Before the
publication intent, an interruption publishes nothing. After publication
intent, rerun the same publish command to verify the recorded state and finish
publication; never edit an accepted pointer by hand. The docs journal records
pointer ancestry so recovery preserves docs already published after a code
checkpoint.

Bootstrap uses the reviewed helper in its immutable submission capture. It
freezes the legacy pointer and queue, installs a hold tombstone, and runs the
full candidate check without publishing. The helper then waits for integration
evidence. Publish verifies the saved base, candidate, CI receipt, manifests,
review evidence and helper bytes under the publication locks before writing
the final tombstone and v2 pointer. The final tombstone prevents old controller
copies from writing state after activation.

If bootstrap validation fails, an interrupted prepare is recovered, or the
operator cancels, the helper restores the saved legacy pointer and drains docs
captured during the hold. A crash during restore is resumed by `cancel` or the
next `prepare`. A crash after the final tombstone is resumed by the same
publish command, which re-verifies the candidate and evidence before installing
the v2 pointer. A hold, tombstone or inconsistent pointer is a stop condition;
do not edit it manually.

The CI lock descriptor is inherited by validation subprocesses, so interrupting
their parent does not admit another CI job while checks are still running. The
kernel releases the lock after its last holder exits; there is no stale PID
lock to delete. These guarantees cover process interruption, not power loss or
damaged storage. Keep the accepted pointer, referenced checkpoints, receipts,
queue captures and Pijul databases in backups, together with the sibling metadata
repository `~/work/codekarl-meta` (issues, specs and review receipts); see the README.

## Metal and Cargo

The boundary is `Rust -> extern "C" -> Objective-C -> Apple's Metal framework`.
Objective-C uses ARC and autorelease pools. The opaque retained context has one
Rust owner; Drop releases it. The wrapper is neither Send nor Sync. The
bootstrap probe waits for completion before returning CPU-visible results. An
asynchronous renderer needs a separately designed resource-lifetime model.

The GPU crate's build.rs invokes Apple's clang to compile bridge.m into a
static archive, then Apple's Metal compiler to produce IR and a .metallib.
Cargo embeds that compiled library into the executable with include_bytes!.
Loading it and creating a Metal pipeline at runtime can still involve
device-specific driver work; the application does not compile MSL source at
runtime.

The Rust toolchain is pinned. Xcode and the SDK are reported in each validation
log but are not vendored or fully pinned. The build targets the native Mac's
architecture; cross-compilation is not configured. Development targets the current Mac
only (owner decision, 2026-10-17): `.cargo/config.toml` forces
`MACOSX_DEPLOYMENT_TARGET=26.0` for every Cargo build, and the GPU crate's build
script passes that one target to the native bridge, the shaders and the Rust
link. An exported shell value cannot change it. Older macOS versions are not
supported or tested.

**Shader debug information and profiling.** Shader debug information follows
Cargo's debug-info setting: dev and test builds embed the shader source and line
tables in the metallib (Xcode's Metal debugger and Instruments can show source
lines), and release builds carry neither. `cargo build --profile profiling` is a
release build with line tables for Rust and shaders, for Instruments. The
`shader_debug` test checks the embedded metallib in each case.

References: [Cargo workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html),
[Cargo build scripts](https://doc.rust-lang.org/cargo/reference/build-scripts.html),
[Apple's precompiled shader libraries](https://developer.apple.com/documentation/metal/building-a-shader-library-by-precompiling-source-files),
[Pijul manual](https://pijul.org/manual/).
