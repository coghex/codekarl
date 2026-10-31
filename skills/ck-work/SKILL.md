---
name: ck-work
description: Implement an assigned Codekarl task in an isolated Pijul worker copy, validate it and hand its immutable submission to opposite-brand review. Only for Codekarl workers.
---

# Codekarl work

Read `~/work/codekarl/skills/AGENT-POLICY.md` at entry and
after resumption. Work is Class B at high effort; resolve the shared model class.
Use `ck-chat` and your bound `ck-worker-N` identity. Claim an authorized task
before acting. With no assignment, report readiness and wait.

After a wake or resumption, reconcile before acting: your own handoff or report, `tools/ck status`,
`tools/ck issue show`, accepted receipts and `ck-chat inbox`. A notice, IRC wake or cmux pointer
points to work; it is not proof the work is current or unfinished, and a notice about work already
accepted or finished produces no new work. Resume an acknowledged but unfinished task once, from its
recorded state, without abandoning or restarting it. Use only your own bound identity
(`pchat agent who --json`); never inherit another session's identity or claims, or register a new
identity to work around a retired registry row: report that instead.

Chat commands always use the root `~/work/codekarl/tools/ck-chat` (the accepted,
installed version), never a worker copy's `tools/ck-chat`, which may predate accepted chat changes;
project commands (`tools/ck check`, `record`, `submit`) use the copy's own `tools/ck`.

Read the task handoff, specification, dependencies and local AGENTS.md. Confirm
the worker path is isolated under the Codekarl hub. Do not restart your session
to change directories; use explicit workdirs. Reconcile unexpected worker state
instead of resetting it.

Implement the assigned behavior and add meaningful real-Pijul specs. Run the planned check,
`tools/ck check --changed` (add groups with `--group ID`), before recording, and report its plan:
the selected groups and why, with every unselected group named. The batch runs the full level
(every group, Metal and native windows included) once, so do not repeat it unless the task asks
for it. Return unresolved design to Class S. Skills, commands, tests and other executable Markdown are code.

Ordinary docs use `tools/ck docs --from COPY -m MESSAGE
PATHS`. Code uses `tools/ck record -m MESSAGE --issue CK-N PATHS`; raw Pijul
records are not accepted as known provenance.
Inspect changed hooks before approving them.

Submit with `tools/ck submit --issue CK-N[,CK-M] --spec 'behavior and tests'`.
Captures include prerequisites; report every included task. Never edit a
capture, manifest, queue, accepted pointer or receipt. Report task ID,
submission ID, patch hashes, queue-manifest SHA-256, author sessions/families
and validation.

For review findings, keep the same worker copy, record fixes, and submit a
dependent revision with `--supersedes`. Preserve earlier evidence and obtain a
fresh review. Workers do not run `ck batch publish` or edit acceptance state.
Report the result as awaiting review until a real acceptance receipt exists.
