---
name: ck-review
description: Independently review an immutable Codekarl Pijul submission with opposite-brand provenance and an exact-scope verdict. Only for Codekarl reviewers; does not accept patches or run a GitHub review flow.
---

# Codekarl review

Read `~/work/codekarl/skills/AGENT-POLICY.md` before
reviewing. Review is Class A at high effort and opposite the actual author
family. Resolve through `model-classes`. Use `ck-chat` as your bound
`ck-reviewer-N`; claim an authorized assignment.

While Copilot delivery is unavailable, use the bounded owner/coordinator
handoff in the visible terminal. Preserve its provenance and exact task,
submission, model, effort and session evidence. Do not impersonate chat
identities or expect automatic wake-up.

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

Read the task spec, author ledger and immutable `.pipeline/queue/ID.json`.
Record the manifest byte SHA-256 and verify that the captured Pijul hashes and
fingerprint match. Compare to the recorded base and read every changed path,
including executable skills and hooks. Check behavior, specs, failures,
recovery and whether the workflow can be followed.

Do not edit the capture or checkpoint. If execution is needed, use a disposable
clone with its `.ck-hub` marker set to the real hub. Recheck manifest digest and
capture fingerprint before recording a verdict.

Use `ck review submission ID --verdict approved|changes-requested --report FILE`
to store the exact immutable receipt and report. Spec and batch reviews use the
matching `ck review spec` and `ck review batch` commands. A batch review is not
requested for a single-submission batch (`ck batch status` shows
`publication_basis: single-submission`); the submission review is its basis. APPROVED requires no
unresolved blocking findings; otherwise state locations, consequences and
required behavior. Post the receipt ID, report digest and task link to the
manager and worker.

An approval applies only to the subject digests and spec revisions in its
receipt. The worker implements fixes. If you implement a fix, declare yourself
an author and obtain independent opposite-family review. Never submit, publish,
accept, or alter queue state as a reviewer.
