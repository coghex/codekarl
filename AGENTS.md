# Codekarl working agreements

This is a local Rust/Metal learning project using Pijul. Do not initialize Git,
create GitHub artifacts, invoke the production Kanban pipeline, or introduce a CI
daemon. The owner deliberately chose patch batches and on-demand acceptance.
One exception: the owner keeps a one-way, read-only mirror of accepted state on
the `master` branch of github.com/coghex/codekarl, driven only by the owner's
untracked `.pipeline/hooks/post-publish` hook, which `tools/ck` starts after a
command advances the accepted pointer. Agents never take work from, push to or
open issues on it.

Before Codekarl agent work, read `skills/AGENT-POLICY.md`, the master role and
opposite-brand review policy. Management/design is Class S, implementation B,
review A, all at high effort. Resolve current models from the shared model-class
configuration. Every issue spec and submission needs fresh independent review;
CI alone does not authorize publication. The setup hold is lifted: the manager
dispatches owner-authorized work through the normal workflow (see
`skills/AGENT-POLICY.md`); known deferrals and gates are unchanged.

Use `tools/ck` from the working copy. It provides the pinned Rust toolchain, Xcode
selection, and project-local Pijul signing identity without changing global shell
settings. `tools/ck worker <name>` creates an independent editable copy from the
accepted checkpoint. Do not edit `.pipeline/checkpoints/` or submission captures.

Before changing code, state the behavior and add meaningful spec coverage where
needed. `specs/` contains executable contracts; `tests/` exercises the acceptance
workflow with real Pijul repositories; the Rust CLI spec
(`apps/codekarl/tests/cli_spec.rs`, reading `specs/bootstrap.json`) and the native GPU
test (`crates/gpu-metal/tests/gpu_probe.rs`) exercise the built program inside
`cargo test`. Run `tools/ck check --changed` (the planned check) before you record: it runs the
floor checks and only the groups your change affects, and prints for every group whether it ran
and why. Name more with `--group ID`. Never skip a group silently: a planned check lists each
unselected group and its reason, and the batch always runs every group (Metal and the native
window tests included) once, as the full level (`tools/ck check`).

Record code with `tools/ck record -m "message" --issue CK-N [paths]`, which adds
provenance; a raw `pijul record` outside `ck` produces patches of unknown
authorship. Add new files first with `tools/ck pijul add --recursive <paths>`. The
approved record hook runs formatting and Python lint checks. If `pijul.toml`
changes, inspect its hooks before explicitly reapproving them with
`tools/ck pijul hooks approve`.

After activation, record code with `tools/ck record -m "message" --issue CK-N`
and submit with `tools/ck submit --issue CK-N --spec "behavior and tests"`.
`tools/ck batch prepare` freezes eligible reviewed changes and runs combined CI;
an independent integration review must precede `tools/ck batch publish`, except
for a single-submission batch (one member whose candidate equals its reviewed
capture, AI-reviewed, CI timing within the watchdog margin), which publishes on
that submission review alone: `tools/ck batch status BID` shows the
`publication_basis` (`single-submission` or `integration-review`, with the
reason). The publisher rechecks exact state before writing the v2 pointer. `ck accept` is
removed. Never hand-edit accepted pointers, manifests or receipts. `tools/ck sync`
updates a clean working copy after publication.

Ordinary documentation is ungated. Publish README.md and docs/**/*.md using
`tools/ck docs -m "message" <paths...>` as soon as ready. This is the Pijul
equivalent of the owner's docs-push policy; do not use the Git-only docs-push
command here. Keep executable Markdown (including this file, skills, commands,
and any documents consumed by tests) outside docs/ and pass it through the code
gate. The docs lane must never change code. For a stale docs copy, sync before
editing again; do not overwrite newer accepted text.

`docs/archive/` is history: closed records, finished plans and review verdicts. Read it only when asked
or when tracing why something was decided, and never treat it as current behavior. `.rgignore` hides it
and the guide's reports from search (`rg --no-ignore` reaches them). Move a document there with
`tools/ck docs` once its work is accepted. Never list docs paths in `.ignore`: Pijul would stop tracking
them.

## Check levels

The checks are a catalog of groups in `tools/check.py`: each has an id, a category and the paths that
affect it. **Floor** groups always run (fmt, ruff, ShellCheck, the doctor, Clippy, rustdoc, mypy; none
opens a window). **Affected** groups run when a changed path matches their patterns: the Rust tests of
the backend-neutral crates (`rust.core`) and of the application without a real window (`rust.app`,
`rust.app-default`), the Metal tests that need the GPU but no window (`metal.gpu`), the native tests
that open real windows with keyboard and mouse input ignored (`native.window`, the presenter's `metal.window-*` groups, `glfw.window`, `vulkan.window`, `render2d.vulkan-window` and `vulkan.app`), except for explicitly named focus cases (`glfw.window-focus`), each Python test group and the
chat CI. **Optional** groups run only when named. Docs and editor files are claimed by the doctor group.

- `tools/ck check --changed` is the planned check: it compares your copy with the accepted base
  (recorded and unrecorded changes, additions, deletions, both paths of a move, mode changes), selects
  the floor groups and every affected group a changed path matches, and prints each group as run or
  skip with the reason. A changed path no group claims, or a change set that cannot be computed, selects
  every affected group (it fails wide). A new untracked directory counts as touching the directory itself
  (it is not walked): it selects the groups with a pattern inside it or a directory pattern containing it,
  and fails wide unless it lies wholly inside one group's directory pattern. `--group ID` adds a group by name (repeatable; alone it plans
  without a change set), and `--plan` prints the plan and runs nothing. A planned run names itself
  `recipe planned` and builds in its own `target/ck-full/<n>` slot; it is never batch evidence.
- With no flag the **full** level runs every group once. It sets `CK_REQUIRE_FULL_SPEC=1` for the
  all-features Rust groups, so the Metal case must run (a skip or an unavailable GPU fails), and it
  builds in its own `target/ck-full/<n>` slot. The batch runs and records this level, so everything,
  Metal and native windows included, runs once per batch; workers and reviewers need not repeat it,
  but anyone may.
- `--quick` runs fmt, ruff and ShellCheck (the record hook); `--fast` runs the same groups as the full
  level without the build slot and without `CK_REQUIRE_FULL_SPEC`.

A manual run holds no project lock; only batch validation holds `ci.lock`. Exit status: 0 pass, 1
failure, 2 a refused command line, 75 when a step outlived the 45-minute watchdog (an infrastructure
failure, not a patch failure). Report the planned check's plan (the selected groups and why) with your
result; a Metal or window group that the plan did not select is covered by the batch.

## Checked locally, submitted, accepted

Checked locally means the planned check (`tools/ck check --changed`) passed in your copy. Submitted means
`tools/ck submit` captured your recorded state for independent review. Accepted means
`tools/ck batch publish` moved the accepted pointer after combined CI and integration
review (for a single-submission batch, its submission review); `tools/ck status` lists the accepted submissions and `tools/ck sync` brings a
clean copy up to date. Only accepted work is project state: never call local or
submitted work done. `tools/ck review ... --owner` records the owner's sign-off and is
for the owner only.

## Suppressions and comments

A suppression is the exception and carries its reason on the same line: Python
`# noqa: CODE  # reason` (a bare or unexplained `noqa` fails
`tests/test_lint_policy.py`); Rust `#[expect(lint, reason = "...")]`, never
`#[allow]`; shell: a comment line giving the reason directly above a separate
`# shellcheck disable=SCnnnn` line (the recipe runs `shellcheck --severity=style` with
no global exclusions). Global exclusions and ignores in `pyproject.toml` carry a
comment giving their reason. Comments explain why, not what.

## When something fails

- Lock busy (`Already running`, `Timed out waiting for state.lock`): the message names
  the holder's pid and command. Wait, or inspect a batch with `tools/ck batch status BID`.
  Never delete a lock file: a child that inherited the descriptor can hold the lock
  after its parent exits.
- Dirty copy (`Record or remove pending changes in ...`): inspect with
  `tools/ck pijul status` and `tools/ck pijul diff`; keep work with
  `tools/ck record -m "message" --issue CK-N`. Discard a change only deliberately, with
  `tools/ck pijul reset <path>`.
- Quarantined submission (`Closure contains quarantined submission ...`): read
  `tools/ck batch status BID` and resubmit with
  `tools/ck submit --issue CK-N --supersedes ID`. `ck retry` is retired.
- Stale tooling: `tools/ck doctor` names the expected and found tool versions and the
  install command (`tools/bootstrap`); install, then rerun the check.
- Malformed or unfinished state: `tools/ck meta recover` repairs the metadata
  repository; `tools/ck gc --apply` refuses, naming the recovery command
  (`tools/ck batch publish BID`, `tools/ck docs flush`, the bootstrap publish recovery),
  when a record is unreadable or a publication is unfinished. `Accepted checkpoint was
  edited` means restore it from backup and do not sync.
- Collected checkpoint (`controller checkpoint was collected; rerun the command`):
  rerun the same `tools/ck` command. `tools/ck batch status BID` shows a collected
  candidate as `collected`; its receipt stays valid evidence.

## Garbage collection

`tools/ck gc` is a dry run: gc itself deletes and writes nothing. Read its report first.
On this hub only the manager (or owner) runs `tools/ck gc`, the dry run and `--apply`,
and runs `--apply` only after a reviewed dry run. Workers and reviewers use it only in
disposable fixture hubs (a temporary hub in a test), never against this hub. `--apply`
removes only build output (`target/`) and superseded lease-capable checkpoints outside
the retention window (7 days plus the 3 newest of each kind), and trash left by an
interrupted run; it refuses while a recovery condition exists. Captures, records, the
current checkpoint's source and retained checkpoints are listed as protected, and
checkpoints from before the lease protocol (with their build output) as report-only;
none of them is ever removed. Worker copies are neither listed nor collected.

## Pijul equivalents

Use `tools/ck pijul <command>`: `status` (what changed), `diff` (the changes), `log`
(history), `change HASH` (one patch), `credit FILE` (who wrote each line), `add` (track a
new file), `move OLD NEW` (rename and keep history), `remove PATH` (untrack; the file
stays on disk), `reset [PATH]` (discard unrecorded changes). A plain filesystem move
shows as a deletion plus an untracked file: use `move`. Submitted and accepted patches
are never rewritten; fix forward with a new patch (`tools/ck submit --supersedes`).

## One concern per patch

Keep one concern per patch and per submission; independent work gets separate
submissions. More detail: `docs/tooling.md` and `skills/ck-work/SKILL.md`.

## Native Metal and architecture

Native Metal belongs in crates/gpu-metal: a small C header, Objective-C/ARC
implementation, and an ownership-safe Rust wrapper. Compile .metal source in
build.rs. Keep GPU infrastructure independent of future render-2d/render-3d,
resources, simulation/core, and game crates; create those as they gain real code.
Do not add a window, ECS, renderer abstraction, or map representation until its
design has been discussed with the owner.
