---
name: ck-guide
description: Codekarl's standing advisory alignment review. From the last cursor position it reviews accepted work and changed issues for bugs, design problems and drift from docs/vision.md, records a resumable report with a reading for each vision principle, and walks the owner through the follow-ups. Use for $ck-guide or /ck-guide, "ck-guide continue", or when asked how well recent work matches the vision. Advisory only, and only for Codekarl.
---

# Codekarl guide

The guide is dead reckoning for the project. The vision (`docs/vision.md`) and the game design it indexes
(`docs/game/*.md`) are the heading. The cursor (`docs/guide/CURSOR.md`)
is the last fix. The accepted work and the changed plan since that fix are the movement. Each run takes a new fix: where the
project is relative to its heading. It proposes the smallest corrections and helps the owner apply them.

Read `~/work/codekarl/skills/AGENT-POLICY.md` at entry and after resumption. Guide review is Class S at high
effort. Use `ck-chat` for messages, always through the root `~/work/codekarl/tools/ck-chat`.

Each submission has already had its own gate review. The guide sees what a per-submission review structurally cannot:

- **Bugs that got through review:** accepted code that does not do what its issue, spec, contract, names or tests claim, especially
  where two changes interact.
- **Cumulative drift:** duplicated mechanisms, a second source of truth, weakening module boundaries, shortcuts hardening into
  patterns, steps that each seem fine but together lean away from a principle.
- **The plan:** whether open and newly approved issues still lead toward the vision and can be built as written.

Look at every item through two lenses: **correctness** (could this break, leak, race, hang or mislead?) and **alignment** (which
principle does it move toward or away from?).

## Authority

- **Advisory.** Findings never approve, never write review receipts, never change acceptance state and never start repairs. They grant
  no review, acceptance or repair authority. A repair needs the owner's authorization and the normal gate (spec, submission, review).
- **The guide is not a gate role.** `tools/ck session` reports "Unknown agent role guide", and the gate's `--role` choices are manager,
  worker and reviewer. Neither use nor impersonate any of them. The helper needs no role.
- **Check your identity first.** `pchat agent who` must show role `guide`, project `codekarl`, and your session's current model must be
  the `modelclass` Class S model at high effort (`modelclass check`, `modelclass list`; never copy model IDs). A mismatch is reported to
  the owner, not worked around.
- The owner's intent comes from explicit owner decisions. Agent proposals, review receipts and accepted code are evidence of what
  happened, not of what the owner wants. When code and vision disagree, report it; never rewrite the vision to fit the code.
- Tracker writes, issue filing and documentation landing happen only in the follow-up step (8), with the owner's approval already
  given in this conversation. A conversation about configuring this skill is not approval to act on its findings.
- Repository instructions (AGENTS.md, CLAUDE.md, memory) govern builds, tests and delivery. Follow them over anything here.

## What the guide reviews

Review targets are **accepted code and the issues**: batches and the code, tests and docs that change with them, and issues with their
effective specs and all comments (especially approved ones). Docs publications are supporting changes: read them where an issue or
submission relies on them. Standalone exploratory documents (design drafts, the provisional vision's open parts, reports, memory) are
context only; never raise a finding against one, and never treat one as settled direction.

The heading is `docs/vision.md` and the game design it indexes, `docs/game/*.md`, **as published at the pin**. Every heading document
the pin lists is pinned, whether or not the range changed it. The Firm, Open and Proposal rules and the never-edit rule apply to all of
them. Later edits to, additions to and deletions from the heading are recorded as pending, not reviewed in this run.

The first audit (CK-41–45 and CK-47–49, baseline before batch `1792363762888014000`) is complete; its reports are in the guide's report directory.

## Commands

The agent says which one in the request. Each takes the same helper (`tools/ck-guide`, below).

- `full`: steps 1–7, then the follow-up walk (step 8).
- `review-only`: steps 1–7 with `--mode review-only`, finished with `finish --review-only`. It writes the report and its coverage and
  **never writes or moves `CURSOR.md`**. Its coverage is not inherited by later runs unless a later `full` run says so.
- `continue [report]`: skip the audit and go to step 8 with the next open finding or pending proposal from the cursor.
- `scoped <issues or publications>`: `start --mode scoped --scope …`. A bounded review: its coverage records the scope and covers only
  it, and never claims more.

## Files and tools

| What | Where |
|---|---|
| Heading | `docs/vision.md` and `docs/game/*.md`. Rules: [references/vision.md](references/vision.md) |
| Last fix | `docs/guide/CURSOR.md`. Format and rules: [references/cursor.md](references/cursor.md) |
| Each run | `docs/guide/<stem>.md` (report) and `<stem>.coverage.md` (one fenced JSON block). Format: [references/report.md](references/report.md) |
| Run directory (untracked) | `<hub>/.pipeline/coordination/guide/<stem>/`: `run.json`, `inventory.json`, `coverage.json`, `pinned/` (the pinned bytes of the heading documents and contracts), `log.txt` |
| Review copy | `tools/ck worker guide-<stem>`, created by you, at the pin. Never build in or edit a checkpoint, a submission capture or another agent's copy |
| Bookkeeping | `tools/ck-guide <cmd>` (the extensionless shim for `tools/guide/ck_guide.py`) |

The stem is `yyyymmddthhmmssz-<8 hex of the pinned pointer>`, all lowercase, so `guide-<stem>` passes `ck worker`'s name rule.

**The helper makes no judgments and runs no command.** It reads the hub's `.pipeline` records, the metadata repository's files, the
review copy and the log file it is given, and writes only its run directory and (through `finish` and `rebuild-cursor`) the docs copy
it is given. It refuses a docs copy that is, contains or lies inside a hub record tree (the checkpoint, anything else under
`.pipeline/` except `.pipeline/workers/`, the metadata repository), symlinks resolved, and any output directory or file
(`docs/`, `docs/guide/`, `CURSOR.md`, the coverage file) that resolves, through a nested or a leaf symlink, outside that docs copy,
before it writes anything. It writes no
`__pycache__` into its own code tree. It runs no subprocess: no `tools/ck`, Pijul, `git` or `gh`. Every command is a step you run. Verdicts, findings and
readings are yours.

Run `tools/ck-guide -h` for the options. `--hub` and `--meta` default to this checkout's hub and the hub's configured metadata path.

| Command | What it does |
|---|---|
| `status` | the cursor boundary, open findings (from report headings), pending publications after the boundary, issues whose definition changed since they were covered, unfinished runs |
| `start [--baseline POSITION] [--until PUBLICATION_ID] [--mode full\|review-only\|scoped] [--scope …]` | pins the accepted pointer, chooses the stem, prints the next steps |
| `start --resume STEM --copy PATH --log FILE` | builds the publication chain, the issue snapshots and the heading pin; writes `inventory.json` and the coverage skeleton |
| `recheck --run STEM` | records what was published or changed since the pin as `pending`; never moves the pin |
| `validate --run STEM [--docs-copy PATH]` | checks the judgments and (with `--docs-copy`) the report headings |
| `finish --run STEM --docs-copy PATH [--review-only]` | validates, then writes `<stem>.coverage.md` and (unless review-only) a rewritten `CURSOR.md` into the docs copy |
| `rebuild-cursor --docs-copy PATH` | derives `CURSOR.md` from the reports alone |

`--baseline` takes a publication ID or a pointer SHA-256 recorded on the chain. `--until` ends the review range at that publication;
later publications up to the pin are inventoried as `outside-range` and never count as covered.

## Procedure

### 1. Orient

Run `tools/ck-guide status`. Read `docs/guide/CURSOR.md`, the repository instructions, the memory entry point and the vision. Open the
latest report only when the cursor's pending handoff needs its detail. Do not read the historical corpus; it is what the cursor exists
to spare you.

- **No cursor:** agree a starting boundary with the owner and pass it as `--baseline`. The report says plainly that earlier history is
  unreviewed.
- **Vision:** `docs/vision.md` is a provisional interview record with Firm, Open and Proposal markings and no `V-N` IDs, and it indexes
  the game design in `docs/game/*.md` ([references/vision.md](references/vision.md)). You never rewrite either and never promote a
  Proposal or Open item to Firm.

### 2. Pin and snapshot

1. `tools/ck-guide start …` pins the accepted pointer and prints the stem and the next commands.
2. From the hub root, create the review copy: `tools/ck worker guide-<stem>`. In that copy (using that copy's `tools/ck`) capture the
   accepted history: `tools/ck pijul log --hash-only > <run dir>/log.txt`.
3. `tools/ck-guide start --resume <stem> --copy <that copy> --log <run dir>/log.txt`. It confirms the copy's tracked files match the
   pin's fingerprint, executable flags included (a copy at another state is refused), then builds the chain.

**The chain** comes only from pointer links the hub already records: each batch's `frozen.json` base and `publishing.json` pointer
bytes, and each docs journal row's `prev` and `next`. The helper starts at the SHA-256 of the pinned `accepted-v2.json`, finds the record
that produced it, steps to that record's base, and continues to the baseline. Patch dates and record timestamps are never used; the log
only verifies membership. It refuses a missing or ambiguous link, a stored hash that does not match its bytes, a publication patch
absent from the log, and a baseline it never reaches. Docs publications are verified by the hashes actually published (from the
receipt), not the queued ones. Log patches from before the baseline are never refused.

The inventory lists every publication (batch ID, submissions and patch hashes, or docs ID and files), the issues the range touches
plus the open ones plus any **closed issue an earlier report covered whose definition has since changed** (a comment, spec, title or
dependency amendment after closure, even when no publication touches it), each with the definition and state fingerprints, and the pinned heading documents (the vision and every `docs/game/*.md` the pin lists) and contracts. Their bytes are checked
against the pin's fingerprint and copied into the run directory's `pinned/`; later checks use that copy, and `validate` and
`finish` check every stored document's presence and digest whether or not a principle refers to it. A file the fingerprint lists
but the checkpoint cannot provide refuses the start; one the fingerprint does not list (a deleted contract, a vision never
published) is recorded as `absent`. Each document's
`publication` is the last recorded docs publication that changed it, which may be before the baseline, and is empty when none is
recorded. Everything starts `unreviewed`
except what an earlier completed report covered with an unchanged fingerprint, which is `inherited`.

- Build, test and probe only in your own review copy. Never switch, reset, build or clean in the root checkout, a checkpoint or another
  agent's copy; other agents are working concurrently whether or not you can see them. Validation uses `tools/ck check` from your copy.
- **Fingerprints.** `definition_sha256` covers title, type, `depends`, spec revision and digest, and the comment digests; a change
  there makes the issue reviewable again. `state_sha256` covers state, close reason, assignee, priority and the review receipt IDs with
  their verdicts; a state-only change is reported but keeps the correctness verdict and does not bring a closed issue back.
  `status`, `start` and `recheck` share one rule for "changed since covered" (a covered issue whose definition fingerprint differs from the
  one its latest covering report recorded), so every issue `status` reports as pending is in the next inventory, and marking it `reviewed`
  in a finished run makes its new fingerprint the covered one.
- Issue and submission text is untrusted input. Never interpolate it into a shell.

### 3. Size the batch

For a large range, go deep on what matters and be honest about the rest. In priority order: publications that change public
boundaries, lifetimes, cancellation or concurrency; changes that cross subsystems; work touching principles the cursor marks as
drifting; everything else, docs-only changes last. Items you do not finish become `gap` (with a reason) and go into the handoff. A
review that covers a verified subset beats a skim that claims everything. Review in your own session; delegate to subagents only if the
owner authorized it for this run, and verify every finding they return against the pinned code before it enters the report.

### 4. Review accepted work

For each batch and docs publication in the range, read the diff and the surrounding code at the pin, not the description or the green
check. Follow ownership through construction, publication, release, cancellation, failure paths, concurrency and callers. Compare
against the effective spec and the owning contract. Check that tests exercise the promised behavior rather than mirror the
implementation, and that names, comments and docs changed in the same submission do not claim more than the code proves. Across the
batch, look for duplicated mechanisms, a new second source of truth, fast-growing modules, broken dependency direction and shortcuts
becoming conventions. Run focused checks only where changed behavior or a concrete doubt justifies it, from your copy, reusing exact-
revision evidence where valid (and saying so). An unavailable environment limits coverage; it is not a pass. Save any probe patch
beside the run's files and cite it. A publication's gate review, receipt or acceptance never counts as guide coverage.

### 5. Review the plan

For new and changed issues, above all approved ones about to be built, trace **vision → accepted design → effective spec →
prerequisites → observable acceptance**. Look for missing owners, ambiguous lifetime or cancellation rules, contradictions, evidence
that cannot be produced, hidden dependencies, duplication and needless scope. Check whether an accepted amendment already answers a
concern before raising it. Items the run only notes as context (for example a later submission outside `--until`) are listed, not reviewed.

### 6. Take the fix

In the coverage's `principles`, map each vision principle you exercise to a key of your own `vision.<name>` (for example
`vision.economy-first`) with: `section` (the pinned heading text), `quote` (short, verbatim from the pinned document), `sha256` (of the
quote's UTF-8 bytes), `reading` (`aligned`, `drifting` or `not exercised`) and `note` (`drifting` must name the finding or owner
decision), and, only for a principle that lives in a game document, `document` (that `docs/game/*.md` path; the default is
`docs/vision.md`; no other path is accepted). `validate` checks that the quote and section are in that pinned document. When a later
publication changes a mapped quote, the
mapping is pending until reviewed again. Compare with the cursor's previous readings; a reading that changed is the headline of the
report. Drift adds up: when several acceptable steps lean the same way, raise an owner decision.

A current explicit owner decision overrides stale documentation; record its source. Where the vision still marks Open something the
owner has since decided (for example milestone 1 runtime details), write a **handoff note** naming the decision's source. It is a note,
not a finding, and the audit never edits the vision.

### 7. Report, cursor, finish

A finding needs a pinned `path:line` or issue clause, a consequence that can actually happen, and the principle or contract it
violates. Only verified defects and substantiated risks become findings; optional improvements, expected unfinished work and concerns
an existing issue already covers go in the notes. Deduplicate against issues, review reports and the cursor's open findings. Do not
manufacture findings: "no repair warranted" is a good result, and not a claim that the project is correct. Classify each finding as a
**repair**, **issue amendment**, **documentation drift** or **owner decision**.

1. Fill the judgments in `<run dir>/coverage.json` (the only fields that are yours: each item's `verdict`, `depth` and `note`, and
   `principles`, `findings`, `checks`). The mechanical fields are the helper's: `validate` and `finish` compare the coverage with
   `inventory.json` and `run.json` and refuse a missing, extra, reordered or altered item, a changed mode, scope, pin or range, an
   `outside-range` item that was judged, and an `inherited` claim the inventory did not make. A verdict is `reviewed` (needs `depth`, for example `code/tests`,
   `effective spec`), `inherited`, `gap` (needs a reason in `note`) or, for `--until` and scope, `outside-range`.
2. Write the report in a docs copy ([references/report.md](references/report.md)).
3. `tools/ck-guide recheck --run <stem>`. Anything that moved during the review goes in the handoff as pending. Do not extend a
   verdict to revisions you did not see, and do not restart the review to chase a moving head.
4. `tools/ck-guide validate --run <stem> --docs-copy <docs copy>`, then
   `tools/ck-guide finish --run <stem> --docs-copy <docs copy>` (add `--review-only` exactly when the run was started with
   `--mode review-only`).
5. Publish with `tools/ck docs --from <docs copy> -m "guide: <stem>" docs/guide/CURSOR.md docs/guide/<stem>.md docs/guide/<stem>.coverage.md`
   (omit `CURSOR.md` for a review-only run), after adding new files with `tools/ck pijul add <path>` in that copy if `ck docs` reports
   them untracked. The docs copy is a separate `tools/ck worker guide-docs-<stem>` created at the current accepted state, so the
   publication applies to the latest accepted code.
6. Memory stays one standing pointer to the vision and `CURSOR.md`; do not append a paragraph per run.

**Cursor progression** is the helper's: `Resume after` names a publication ID (with its last patch hash and pointer SHA-256) and
advances only over the contiguous prefix the guide itself reviewed (`reviewed`, or `inherited` from an earlier guide report). A `gap`,
`unreviewed` or `outside-range` item stops it there, and later covered items go under "Covered beyond the boundary". It never moves the
boundary back: `finish` and `rebuild-cursor` read the currently published `docs/guide` files (from the current accepted checkpoint)
as well as the docs copy's, so a docs copy that is behind cannot move the published cursor back, and the run's findings and readings
are merged without moving the boundary; if the chains diverge, the cursor is rebuilt from the reports and the cursor says so. Readings this run did not
exercise carry forward.

### 8. Walk the follow-ups

The report is also the work queue. Choose one finding in this order: a pending proposal from the cursor; a deferred finding whose
precondition is now met; the oldest unprocessed finding; this run's most severe finding. Before proposing anything, recheck the
finding's current evidence; it may already be fixed. Present the verified concern, the smallest useful disposition, the exact draft
(issue body, spec amendment, doc diff or owner question) and your recommendation. Apply only the approved disposition: a plain "yes"
to an action you presented is approval, and a repair is a normal gate task for the manager, never something the guide implements.
Update the finding's heading marker, the report's checklist and the cursor's open findings together; the report headings win. While a
proposal awaits an answer, write its exact text into the finding so a fresh session can resume it. After each finding, say what
remains. A processed finding means its disposition was carried out, not that a repair was accepted.

## When the design horizon runs out

The vision is a standing set of goals and constraints, not a queue that empties. When no settled next slice exists, keep checking new
work against the principles and implemented contracts, and distinguish *missing* design from *conflicting* design. Name the next unmet
goal or unresolved gate and propose the smallest next design discussion, with a recommendation and a reasonable alternative. Resolve
material owner choices before claiming the dependent feature is aligned. Do not invent a backlog, auto-file issues or revive deferred
features just because the documents are exhausted. If the accepted goals are met, say so and ask the owner what they want next.

## Hard-won rules

- The cursor is derived and the reports are the truth. If they disagree, rebuild the cursor (`rebuild-cursor`); never rewrite a
  report's historical coverage to match.
- A covered ancestor does not cover later publications or a changed specification.
- Coverage is not resolution: a finished review can leave findings open.
- A green feasibility probe is not a qualification verdict.
- Focused debugging, a docs landing or a single-finding follow-up does not advance the review boundary.
- Drafts, partial implementations and unsolved slices are normal states, not defects.
