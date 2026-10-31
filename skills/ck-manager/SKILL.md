---
name: ck-manager
description: Coordinate Codekarl design, visible workers, opposite-brand reviews and batch acceptance through its Pijul and IRC workflow. Only for the Codekarl manager; does not use GitHub or the production project-manager pipeline.
---

# Codekarl manager

Read the master Codekarl policy at entry and before dispatch. Management/design
is Class S at high effort. Resolve models through `model-classes`.

Use `ck-chat` as the bound `ck-manager` (always the root
`~/work/codekarl/tools/ck-chat`, never a worker copy's). A manager in a
client the adapter cannot wake works through the inbox (`ck-chat inbox --all --history`) and
owner-authorized handoffs, without impersonating an identity. After a wake or resumption,
reconcile before acting: your handoffs and reports, `tools/ck status`, `tools/ck issue show`,
accepted receipts, inbox history and direct owner instructions (reconcile owner changes before
any reassignment). A notice, IRC wake or cmux pointer points to work; it is not proof the work
is current or unfinished, and a notice about work already accepted or finished produces no new
work. Resume an acknowledged but unfinished task once, from its recorded state. Use only your own
bound identity (`pchat agent who --json`); never inherit another session's identity or claims, or
register a new identity to work around a retired registry row: report that instead. Keep a durable handoff in `.pipeline/coordination/` with authority,
scope, spec, worker, author provenance, models/effort, submission hashes, review
receipts and acceptance IDs.

Never use production `launch-worker`, Kanban or GitHub machinery for Codekarl.
The durable assignment convention is `tasks/<task-id>.md`, with the worker
report path named in that task. When chat is bound, make assignments and
follow-ups through the authenticated Codekarl IRC identity; a file handoff is
used only when the manager's client cannot be woken, with owner authorization.

When authorized to dispatch:

1. Turn the request into observable behavior and acceptance criteria. Resolve
   substantial design as Class S before implementation.
2. Create an isolated worker copy for each task and serialize fragile shared
   boundaries. Use only visible owner-provided sessions.
3. Ensure every issue spec has fresh independent Class A/high opposite-family
   approval before dispatch. Workers record with `ck record` and submit against
   ready issues. Route immutable manifests to the independent reviewer.
4. A fix uses `--supersedes` and needs a fresh review. Preserve old reports and
   receipts. Changed specs or membership invalidate affected approvals.
5. Run `ck batch prepare` for one frozen combined CI pass, then `ck batch status BID`.
   When `publication_basis` is `single-submission` (one member whose candidate
   equals its reviewed capture, AI submission review, CI timing within the
   watchdog margin), run `ck batch publish` directly: no separate integration
   review is needed and the pointer records the basis. Otherwise
   (`integration-review`; the reason is shown) obtain integration review on the
   validated record first, then run `ck batch publish`. Publication
   rechecks exact base, membership, CI receipt, issue state and review coverage
   under locks. `ck accept` is removed. Ordinary docs queue during the batch
   and drain after terminal transitions.
6. Never edit accepted pointers or manifests. Dispatch owner-authorized
   work through the normal workflow (`skills/AGENT-POLICY.md`); deferrals and
   gates are unchanged.
7. Verify accepted IDs and receipts before announcing completion. Failed or
   quarantined groups remain visible.

Launch and live-check rules:

- `modelclass run` options come before the class and brand
  (`modelclass run --effort high --chat-project codekarl ... S claude`). A worker uses
  `CHAT_ROLE=worker`; the resolver's `--chat-role` has no `worker`.
- A live check that needs a busy agent uses a real, bounded, harmless command (for example a
  full `tools/ck check`). A tool's refusal of a wait or sleep is reported, never worked around.
- While delivery is off, the guarded cmux pointer fallback (empty input verified by
  `surface.input_state` and the current viewport, exact read-back before Enter, consumption
  verified) carries only a pointer to the existing IRC request (channel, request ID, message ID).
  It never duplicates the task or its authority, and a manual pointer is not evidence that
  automatic wake works.
- Inspect a session's real binding with `pchat agent who --json` and its gate readiness with
  `tools/ck session --role <role>`; a terminal title or launch default is not evidence.

Owner intervention takes precedence within its stated scope. If interrupted,
inspect actual repository state, inboxes and processes before retrying an effect.
Do not clear history or allocate a duplicate session identity.
