# Codekarl live IRC pilot — consolidated findings and worklist

Date: 2026-10-15, updated 2026-10-17. Status: **closed under the owner's `ck-workflow-ready` goal.** Every PF item has an
installed result or an owner disposition. On 2026-10-17 the owner accepted the remaining PF-09 recovery gaps as known and
untested, deferred the busy Codex stop, and authorized one bounded live check, which passed (bridge restart, delivery and
Codex dialog E). This is the working document for the pilot and its immediate workflow
follow-ups. It consolidates the manager's result, preflight, progress log,
coordinator findings, runbook corrections and model-readiness follow-ups.
Original evidence is preserved; this document supersedes their scattered
current-status summaries and finding numbers.

The owner later authorized implementation of this worklist and the S-slices as
the `ck-workflow-ready` goal (`.pipeline/coordination/tasks/ck-workflow-ready-goal.md`).
Delivery is **ON** as the routine state since 2026-10-16 01:24:52Z, after the
bounded live checks (it was off between them). That goal is complete (2026-10-17); new work starts from an owner
assignment. The separate [setup hardening record](setup-hardening-plan.md) covers the build, lint, test and storage
slices, all accepted. This document does not create issues, approve specs,
enable delivery, or authorize changes to shared services.

Jump to the [outcome matrix](#2-pilot-outcome-matrix),
[worklist](#3-worklist-and-status), [recommended sequence](#5-suggested-way-to-work-through-this-document-historical),
[old-ID mapping](#6-old-finding-ids-mapped-to-this-worklist), or
[evidence index](#7-source-and-evidence-index).

## 1. Assessment

The basic architecture worked in this supervised run: authenticated owner
messages reached the intended agent, the worker performed the file task once,
the Codex reviewer answered a peer question, an owner interrupt stopped the
observed foreground process, and the worker resumed with its original identity
and conversation. No duplicate file effect or wrong-surface delivery was
observed. One incidental mention caused an unwanted informational notification
under the documented routing rules.

The manager's blanket statement that every exercised criterion passed is too
strong. P2 explicitly required a recorded delivered stage, and that evidence
is missing. Successful command handling and complete lifecycle accounting are
different results. P5 needed a retry after a real pre-send failure. P6 cleanup
needed a repeated coordination message. P6h was skipped, so preservation of an
existing owner draft was not demonstrated by this live run.

Recommended next step: work through the ledger below, beginning with delivery
accounting and missed-instruction recovery. Retain the system rather than
restart it. It has useful durable identity, routing and claim foundations;
the pilot exposed specific gaps that can be repaired and retested separately.
Do not equate finishing the pilot with approval for unattended operation.

### Independently checked evidence

The consolidation audit opened the live SQLite database read-only and matched
messages from **19:16:52Z to 20:14:48Z** against recipient rows, events and
shared transport logs:

| Measure | Observed result |
| --- | --- |
| Room messages in the interval | 22 |
| Recipient rows | 17: manager 7, worker 9, reviewer 1 |
| Stored routing versus recipient rows | Exact match; no orphan inbox rows |
| Delivered events | 2: Codex P4 and the P6i prompt-typed interrupt |
| Notification status | All 17 rows still say `notice=pending` |
| P3 file | Exactly `bvpbg44gehuyr26dptkp6px7te ck-worker-2` plus newline |
| P5 completion replies | One stored reply to the owner request |
| Dead letters linked to pilot message IDs | None |
| Current delivery switch / config mode | `false` / `0600` |
| Current outstanding inbox | Empty |

Transport logs support the expected Codekarl targets. Historical process,
viewport, counter and model observations are attributed to the contemporaneous
manager/coordinator records; they cannot be recreated by inspecting current
process state. A single successful run does not prove crash-safe exactly-once
external actions, every interrupt path, or every client.

## 2. Pilot outcome matrix

| Check | Consolidated assessment | Evidence or limitation |
| --- | --- | --- |
| R0 | Bindings/readiness passed | Native Claude manager and worker, Codex reviewer, separate Claude reviewer readiness; L1 repaired first |
| R1 | Delivery enable passed | Locked single-key edit; historical rows reconciled first |
| P1 | Passed | Verified `vince` status, no recipients |
| P2 | Functional pass; audit incomplete | Verified `coghex`, manager only, completed; delivered event absent (PF-01) |
| P3 | Functional pass; audit incomplete | One append, one completion; delivered event absent (PF-01) |
| P3d | Passed in this run | Duplicate claim returned `fresh:false`, exit 4; no second append/ack/event |
| P4 | Functional and delivery-event checks passed | Codex prompt delivery, claim, linked answer; notice still pending (PF-01) |
| P5 | Functional pass after recovery | Worker-only owner request, no manager request row; first reply failed (PF-02), one successful retry; incidental status mention (PF-06) |
| P6/P6i | Interrupt worked; cleanup recovered | Original sleep refused; replacement foreground wait used (PF-10). Process gone, visible worker ack after about 38 s. Original task declined; interrupt completed only after follow-up recovery (PF-03) |
| P6h | Skipped, not passed | No live proof of holding an interrupt over an existing owner draft (PF-09) |
| P7 | Resume worked; audit incomplete | Offline delivery backed off, same session/name resumed, no counter bump; completed, delivered event absent |
| P8 | Routing/row reconciliation passed; audit incomplete | 22 messages / 17 matching recipient rows; incomplete delivery accounting remains PF-01 |
| P9 | Passed | Historical informational messages claimed once, no new work or completion reply |
| P10 | Passed | Delivery off at 20:14:48Z; no outstanding inbox work |

P7 retries occurred at 20:01:22, 20:01:27, 20:01:58 and 20:03:59 while the
worker was unavailable; the next queued wake was at 20:14:03 and completion at
20:14:13. Recovery was eventual, not an immediate wake on resume. This fits
the current backoff policy; the interval alone does not establish a defect.

## 3. Worklist and status

An unchecked item is unresolved. Check it only when its decision, implementation
if any, and acceptance evidence are recorded. A deferred item needs an explicit
disposition rather than a fictional pass. These are report IDs, not CK issues.

| Done | ID | Concern | Evidence status | Recommended order / scope |
| --- | --- | --- | --- | --- |
| [x] | PF-01 | Lost delivery observations and permanently pending notices | **Installed (CK-4)** and verified live on both the Claude and Codex paths | Done; see the delivered-vs-processed note |
| [x] | PF-02 | Identity discovery can abort a reply before sending | **Installed (CK-5)**: one protected resolution, exact-error retry, NOT SENT exit 5; exact-error regressions in the accepted suite. **Limit:** the transient trigger is unit-tested only (cannot be induced live without manipulating cmux) | Done, with the stated limit |
| [x] | PF-03 | Coordination message marked read without visible worker delivery/action | Stock nightly with the upstream fix installed; **live busy-to-idle and bundled-pair checks passed** | Done; original incidents' caller still not traced |
| [x] | PF-04 | Unknown bracket kind hides unfinished work after claim | **Installed (CK-4)** and verified live | Done |
| [x] | PF-05 | Claim acknowledgement absent from ordinary room text | **Installed (CK-5)** and verified live: actionable claims post one visible `[ack]` (Claude and Codex), informational claims stay reaction-only | Done |
| [x] | PF-06 | Incidental handles trigger status delivery | **Accepted (CK-7)**, batch `1792116963374632000` (2026-10-16 02:20Z) | Done |
| [x] | PF-07 | Bridge-generated status appears as an untrusted diagnostic | **Installed (CK-4)** and observed live 2026-10-16: a verified `[status] chatbridge:` post routed as role `bridge`, no recipients, no diagnostic | Done |
| [x] | PF-08 | Old queued coordination notices can look like new work | **Accepted (CK-7)**, batch `1792116963374632000` (2026-10-16 02:20Z) | Done |
| [x] | PF-09 | Draft/dialog and failure-recovery coverage remains incomplete | **Tested live:** draft (Claude and Codex), dialog (Claude and, on 2026-10-17, Codex check E), one controlled bridge restart with delivery. **Owner-accepted, not tested** (2026-10-17): busy Codex stop via chat (D2; reproducer kept; Plateia escalation and fix deferred; manual interruption accepted), IRC outage and outbox replay, restart recovery beyond the smoke check, reminder, dead-letter and phone-push timing | Closed by owner disposition; future starting points listed below |
| [x] | PF-10 | Runbook, instructions and status summaries drifted | **Accepted (CK-7)**, batch `1792116963374632000` (2026-10-16 02:20Z) | Done |
| [x] | PF-11 | Delivery pause control requires editing shared configuration | **Accepted (CK-10)**, batch `1792118143758785000` (2026-10-16 02:39Z) | Done |
| [x] | PF-12 | Mixed/unknown history bypasses current-class comparison | **Accepted (CK-9)**, batch `1792115313013817000` (2026-10-16 01:53Z) | Done |

### PF-01 — Preserve delivery observations independently of task progress

**Sources:** manager O1; coordinator delivery-history finding; P2/P3/P5/P7;
`tools/chat/codekarl_chat.py`, installed shared `chat-bridge`.

P2 and P3 have acknowledged/completed events without delivered. P5 and P7
show the same pattern. Codex P4 and P6i do record delivered, but their notices
also remain pending. Thus there are two related defects, not merely a
Claude-specific display issue: loss of later delivery observations, and no
successful-notification status update.

The bridge's Claude message send can initially return queued. Its poll interval
is 60 seconds. The recipient may claim before a transport confirmation is
processed. `accepted()` acknowledges before `Deliveries.accept()` invokes the
successful outcome; `Inbox._advance()` discards lower/equal-ranked changes,
including their events. `outcome()` does not clear `notice=pending` on success.
The coordinator reproduced the late-event loss in an in-memory SQLite probe.

The shared log's `accepted by ... (ack:ack)` is evidence of acknowledgement,
not an independently observed delivery timestamp. Do not reconstruct a
fabricated delivered timestamp from it: a recipient can claim by manually
reading an inbox without any terminal wake.

**Recommended contract:** keep task progress monotonic while recording actual
transport observations independently, keyed to message, recipient and transport
receipt. Expose useful notification status. Distinguish confirmed terminal
delivery, acceptance without independently observed delivery, failure and an
unresolved notification. A late observation must neither vanish nor reopen
finished work. Faster polling alone does not fix correctness.

**Acceptance:** cover early claim, late confirmation, completion before
confirmation, manual claim without wake, duplicate receipts, multiple
recipients, restart and failed delivery. Both native Claude and Codex paths
must have truthful status. Rerun affected live checks; preserve historical
unknowns rather than backfilling invented events.

**Disposition (2026-10-16): resolved; installed (CK-4) and verified live on both paths.**

- **Acceptance:** CK-4@r2 (spec `1a01864d…`; reviews: spec `a601d691`, submission `faa6763c`, integration `62ed3555`); submission `1792097229476638000`; batch `1792097583249775000`, CI `b0160144…` (182 tests and Metal; the chat suite ran separately, see RC-1); published to pointer `7ef910c9…`; installed with `tools/chat/install.py apply` (stage 252/252, backup `codekarl-install/1792097887173643000`, Plateia delta `codekarl-handoff/20261015-ck4-inbox-correctness`); bridge restarted; CK-4 closed.
- **Design:** observations are recorded independently of task progress, in a new
  `observations` table (`waiting`, `delivered`, `accepted`, `attempt-failed`,
  `failed`). `notice` is recomputed from them, in the order failed, delivered,
  accepted, latest. Dead letters are never re-rung, and history is not
  backfilled.
- **Live check** (`wrc77z6m…`, 21:00Z): a Claude agent-message wake recorded
  `waiting` (queued at the worker) and then `accepted`, giving notice
  `accepted`. No delivery was invented.
- **Codex path live** (V3b release, 2026-10-16 00:32Z, `.pipeline/coordination/workflow-ready/pf03-live/v1-v2-result.md`): the
  bridge typed one wake into ck-reviewer-1's empty Codex prompt; observations
  `delivered` then `accepted`, notice `delivered`; done once. While an owner
  draft was present the same row stayed `waiting` ("Codex prompt not empty").
- **Delivered is not processed (Claude):** in V4 cmux marked a wake
  `delivered` while Claude Code only queued it behind an open picker; the turn
  ran after the picker closed. For Claude, `delivered` means handed to the
  client queue. Processing evidence is the transcript turn and the claim.
- Rows from before the install keep `notice=pending`.

### PF-02 — Make pre-send discovery failures recoverable and retryable precisely

**Sources:** coordinator O4 (not the manager's O4); owner-reported P5 worker
output; coordinator finding with transcript event and source verification.

P5 request `53mjhiku49gzgt4ask6cm9g79w` reached the worker. At
19:30:10.935Z its first `ck-chat done` returned exit 2 with
`pchat: empty cmux process inventory; existing identities preserved`.
One retry succeeded; reply `75gfikrjsgqj4q5gwieg4j34e2` at 19:30:23.522Z
is the only stored reply to that request.

Installed `pchat` resolves `who()` before `chatlib.post()` and before its
post-error outbox handler. `identities.sync()` raises this exact error when
discovery yields no rows while active surface-bound identities remain in the
registry. It preserves those identities. This specific failed attempt neither
sent nor queued its reply. Why the inventory was empty is not established.

The worker's broader inference was wrong: exit 2 rather than 3, and an inbox
still acknowledged, do not by themselves prove a resend is safe. Asynchronous
state and errors at other stages require evidence of the actual send outcome.

**Recommended contract:** retain the identity-preservation guard, diagnose
empty/incomplete inventory, and expose the failure stage and outcome. Permit
bounded recovery when the operation is definitely not sent; reconcile queued
or uncertain outcomes instead of blindly posting again. Do not force another
identity or trust stale session bindings to make the command succeed.

**Acceptance:** simulate empty inventory before send, persistent discovery
failure and recovery; verify no registry retirement, no initial send/outbox
write, and exactly one reply after recovery. Exercise queued and uncertain
post outcomes separately. Preserve other projects' behavior.

**Disposition (2026-10-16): resolved; installed (CK-5); transient trigger unit-tested only (stated limit).**

- **Accepted and installed:** CK-5@r3 submission `1792112420595716000` (fix 1, hermetic tests; reviews `a18c0c73`,
  `4c3b456b`), batch `1792113249834006000` (CI: 217 workflow, chat 274, Metal; integration `f7d576a3`), published
  2026-10-16 01:23:25Z; adapter installed via `install.py apply` (backup `codekarl-install/1792113822413750000`).
- **Limit:** an empty cmux inventory cannot be induced live without manipulating cmux, so the exact-error retry
  and NOT SENT exit 5 rest on the CK-5 unit tests.

- **Investigation:** `.pipeline/coordination/workflow-ready/pf02-pf03-investigation.md`.
  Every bound post runs a full cmux inventory sync, and an agentless snapshot
  aborts the post before it is sent. Twelve probes were clean, so the
  trigger is unknown.
- **Design (CK-5, Codekarl `ck-chat` only, no shared policy change):** one
  protected resolution, with retry only on that exact pre-send error. The
  resolved identity is passed to the in-process `pchat` call, so it does
  not run a second sync. Exhausted retries give a distinct NOT SENT exit
  5.
- **Status:** r1 (`80db80d0`) and r2 (`e04aa083`) were changes-requested on
  PF-05's partial-failure contract. **r3 approved** (`00611aff`): no reopening
  of uncertain effects; exit 2/4 is `not-sent` only when in-process probes show
  no network login or outbox write; atomic acquisition. Implemented by
  ck-worker-2: submission `1792107422842173000` (manifest `585cc8d4…`), full
  check with Metal and staged suite 273 OK; **submission review approved**
  (`a18c0c73`). Awaiting batch CI, integration review, publication and install.

### PF-03 — A transport “read” flag is not evidence that an instruction arrived

**Sources:** manager O5; manager and worker provider transcripts; P6 cleanup.

Manager cmux message `5961665c-0d7b-48c4-a6f8-5fd9339e09ad`, queued at
19:37:44Z, asked for two actions: claim an informational notice and complete
the already-carried-out interrupt. A manager tool result at 19:38:22Z shows
that message as `read`, with `delivered_via` absent. The worker transcript
contains the chat wake `d27be767-4bb4-4bab-ba92-55a8e2feaab1` and its claim,
but not the first manager instruction. The interrupt remained acknowledged.

A second message `e24213f2-b4a5-45a9-bb7e-a646c3e9413f` appears in the
worker transcript at 20:00:00Z. The worker completed the interrupt at
20:00:09Z, reply `y98f5gkkxa9stiuz8axe558niw`.

The missed instruction is real. “Bundled wakes drop instructions” is a
hypothesis, not a proven mechanism. The recorded chat wake preceded the first
manager send; the evidence does not establish that both bodies were delivered
in one hook payload. cmux inbox targeting/read semantics and hook delivery
need investigation. Do not blame worker reasoning without delivery evidence.

**Recommended contract:** durable task/inbox state remains the work authority.
Prefer IRC for coordination; cmux fallback should point to the same request,
with unchanged identity and scope. Correlate queued, actually injected,
acknowledged and completed evidence. Do not infer execution from a transport
read flag, or replace an unhandled request with a second independent task.

**Acceptance:** reproduce two near-simultaneous notifications around a turn
boundary, retain both IDs, compare hook output with provider transcript, and
test retry/restart recovery without duplicate effects. Verify any inbox query
returns the intended recipient. Reconcile with the cmux/shared-hook owner
before choosing a code fix. No live stress test is authorized by this report.

**Disposition (2026-10-16): resolved; live re-verification passed on the stock nightly.**

- **Second incident:** the manager surface's CK-4 r2 approval `7b44ac57…` was
  read without injection. Its `read_at` equals that of an injected message.
- **Worker incident timeline:** the Stop hooks returned `{}` while
  `5961665c…` was queued, and it was marked read about 130 ms later.
- **Source-level match:** ck-guide found that the former cmux build `496195b`
  marks queued rows read on hook poll and claim, and that this is fixed
  upstream in `7f15e40` / PR 19105, which the current nightly contains
  (`.pipeline/coordination/workflow-ready/pf03-upstream-fix.md`).
- **What is and is not shown:** this is a **matching installed-source
  defect**. Neither incident has a runtime trace of the exact caller.
- **Meanwhile:** a cmux fallback carries only a pointer to an IRC request,
  and receipt counts only when the IRC row is claimed.
- **Installed update:** the owner authorized the stock public nightly and
  closed cmux. The old updater had retained an October 3 installation session;
  quitting completed that intermediate update. With all cmux updater helpers
  gone, ck-guide installed the official ARM64 DMG for
  `0.65.1-nightly.3850749966902` (`f9d8ad6`), which contains `7f15e40`.
  The published SHA-256 matched, strict/deep signature verification passed,
  and the notarized Manaflow signer matched the prior app.
- **Recovery:** the prior app and session metadata were preserved privately;
  only obsolete cmux NIGHTLY Sparkle staging, launcher and download files were
  removed. The running app and CLI report the intended version. Its launch
  update check reports `onLatestVersion` and returns to `idle`; no updater
  helper remains running. All 41 workspaces, 8 groups, 76 panes and 116
  surfaces returned, with split proportions, surface IDs and tab order
  preserved. Internal pane IDs were regenerated. Agent terminals reopened
  as shells; project work has not been restarted by this maintenance.
- **Live re-verification** (2026-10-15 23:55–23:59Z, `.pipeline/coordination/workflow-ready/pf03-live/v1-v2-result.md`):
  - **Busy to idle:** a task sent while ck-worker-2 ran a full check stayed
    `queued` with no `read_at` for the whole check, was injected at the end of
    the busy turn, read afterwards and done once.
  - **Two notifications at a boundary:** two tasks 1 s apart arrived as one
    bundled injection containing both bodies (the shape of the original
    incident); both were read after injection and each done once.
- **Limits:** the exact caller of the two original incidents remains untraced;
  the evidence is that the installed build no longer shows read-without-injection
  in these cases. Busy *Codex* interruption was not exercised.
- **Evidence:** `.pipeline/coordination/workflow-ready/cmux-update-authority.md`
  and the private checkpoint
  `~/.local/state/cmux-update/20261015T221644Z/` contain the exact release,
  signature, replacement, cleanup and restoration records.

### PF-04 — Unknown message kinds hide unfinished acknowledged work

**Sources:** preflight F7; `message_kind()`, `plan()` and `Inbox.rows()`;
new isolated consolidation probe.

The live pilot deliberately avoided `[task ...]`. During this analysis, a
temporary SQLite inbox using the current module received
`[task T1] @ck-worker-7 leave unfinished` from a synthetic authenticated owner.
It routed to the worker with `actionable:false`. Outstanding rows changed
from 1 to 0 after a fresh claim, although history still said acknowledged.
No live IRC, registry or terminal was used. The source hash matches the
installed adapter: `d4fb3266c494f5ec4915d70ec1ae9fab7e95910d10ebe2805260ba00e1a7dc64`.

This is now reproduced behavior, not merely an unverified suspicion. It
conflicts with the promised visibility of unfinished tasks. Existing history
retains the record, but the normal outstanding view loses it.

**Recommended design choice:** treat unrecognized kinds conservatively as
requests, or reject them visibly before accepting/routing them. Favor the
first for ordinary owner prose; decide this explicitly in the spec. Neither
choice should reinterpret unknown senders as trusted or alter quiet handling
of known informational kinds.

**Acceptance:** unknown bracket kinds, plain requests, known statuses/results,
questions, explicit mentions and unaddressed owner messages; an acknowledged
unfinished request stays visible. Cover replay and already-stored unknown-kind
rows so the fix does not apply only to newly received messages. Live behavior
remains untested until a bounded rerun.

**Disposition (2026-10-15): resolved.**

- **Change:** CK-4 maps unrecognised bracket kinds to `request`. Stored
  acknowledged rows stay outstanding when their text kind is actionable.
  Trust and quiet kinds are unchanged.
- **Acceptance:** CK-4@r2 (spec `1a01864d…`; reviews: spec `a601d691`, submission `faa6763c`, integration `62ed3555`); submission `1792097229476638000`; batch `1792097583249775000`, CI `b0160144…` (182 tests and Metal; the chat suite ran separately, see RC-1); published to pointer `7ef910c9…`; installed with `tools/chat/install.py apply` (stage 252/252, backup `codekarl-install/1792097887173643000`, Plateia delta `codekarl-handoff/20261015-ck4-inbox-correctness`); bridge restarted; CK-4 closed.
- **Live check** (`wrc77z6m…`): `[task ck4-live]` was routed with
  `actionable: true`. After the claim, the worker reported it still listed
  (`[done] ck4-live listed=yes`).

### PF-05 — Make claim acknowledgement visible in the room record

**Sources:** coordinator acknowledgement finding; manager O1; `acks.jsonl`.

`ck-chat claim` calls `pchat ack`, which sends IRC TAGMSG reaction metadata.
The acknowledgement is retained in tagged history and `acks.jsonl`; it is not
an ordinary `[ack]` text line. The runbook implied that such a line would be
visible. P6's explicit `[status] [ack]` was a separate worker post.

**Recommendation:** one concise, linked visible claim acknowledgement would
fit the owner's room-as-development-record workflow. This is a presentation
decision, not evidence that claims were lost. Keep the machine receipt
semantics and avoid creating another actionable request.

**Acceptance:** one visible acknowledgement per fresh claim, none on duplicate
claims, no reply/wake loop, useful original-message linkage, and recovery when
the local claim succeeds but the network acknowledgement is queued. Decide
whether informational notices also warrant a visible line before implementing.

**Disposition (2026-10-16): resolved; installed (CK-5) and verified live.**

- Live (2026-10-16 01:25Z, `.pipeline/coordination/workflow-ready/post-install-live/c-result.md`): a Claude worker's
  claim of `[task ck5-live]` posted one `[ack] claimed by ck-worker-2` (no recipients; effects reaction and visible
  `sent`); the Codex reviewer's claim of a routine review request did the same; informational claims stayed
  reaction-only.

- **Owner choice** (IRC `iwu3rb78cx2ijym7pnw4fc86hw`): "Visible
  acknowledgements for tasks and questions; informational notices stay
  quiet". Reaction metadata is retained for every claim.
- **CK-5 Part B** specifies durable per-effect records, per-effect
  reporting and bounded recovery.
- **Status:** resolved; accepted with CK-5 (submission `1792112420595716000`, batch `1792113249834006000`, published
  01:23:25Z; the earlier submission `1792107422842173000` was superseded) and installed; the visible `[ack]` was
  verified live after installation (Claude and Codex).

### PF-06 — Incidental mentions in status text still route

**Sources:** manager O3; status message `i8vrgc8xr3fehcid2piweb7tvs`.

The manager described P5 in a `[status]` containing literal `@ck-worker-2`.
The router correctly created an informational worker row. The worker claimed
it once; no duplicate task occurred. The notification was unintended by the
writer, rather than a recipient-selection defect.

**Recommendation:** use plain identity names when describing past work; use
handles when intentionally addressing someone. Do not globally silence
addressed statuses, which are supported by the existing contract. An escaped
or quoted mention syntax is optional future design, not required by this run.

**Acceptance:** guidance distinguishes descriptive names from explicit
recipients; quiet summaries create no rows, intentionally addressed status
still reaches its recipient, and informational claims do not trigger done loops.

**Disposition (2026-10-16): resolved; accepted (CK-7).** Pilot wording corrected; the executable mention guidance
shipped in CK-7@r3 (spec `8af3c370`); submission `1792116557723521000` (review `415016ba`, with the three-case inbox walkthrough); batch `1792116963374632000` (CI 249 workflow, chat 274, Metal; integration `d3e4fa51`), published 02:20:54Z.

### PF-07 — Classify authenticated bridge observations without granting authority

**Sources:** manager O4; bridge status `hdgvrma6h5ktwp5gnynmteh6xn`.

The authenticated `chatbridge` interrupt status has no active Codekarl agent
role, so it produces an `untrusted` diagnostic and no recipients. This is
visible in current `ck-chat status`. Refusing to give it agent authority is
correct; repeatedly surfacing routine bridge output as a routing problem is
diagnostic noise. The diagnostic view examines recent messages, so this is
not proof that the warning remains visible forever.

**Recommendation:** represent the narrowly authenticated service observation
as informational, or filter its known diagnostic category. Keep the message
in history. Do not add chatbridge to owners/assistants or give it interrupt,
assignment or completion authority to suppress the warning.

**Acceptance:** authentic known service status is inspectable without a false
actionable warning; spoofed nicknames, unverified posts and unexpected service
commands retain diagnostics and cannot route as authorized work.

**Disposition (2026-10-16): resolved; installed (CK-4) and observed live.** On 2026-10-16 01:36:10Z the verified
bridge post `ps4jzq6p` (`[status] chatbridge: interrupt → ck-reviewer-1: …`) was classified role `bridge`, recipients
`[]`, not actionable, with no routing diagnostic, and `ck-chat status` lists no routing diagnostics
(`.pipeline/coordination/workflow-ready/post-install-live/c-result.md`). Rows stored before the install keep their old
untrusted diagnostic, as specified. (The earlier 2026-10-15 disposition read "installed; live observation pending".)

- **Change:** only a verified `chatbridge` post that starts
  `[status] chatbridge: ` gets the role `bridge`, with no recipients, no
  authority and no diagnostic. Every other `chatbridge` post keeps today's
  untrusted diagnostic.
- **Acceptance:** CK-4 (see PF-01).
- **Evidence:** unit tests at install, then the live observation in the disposition above (2026-10-16). Rows
  stored before the install keep their old diagnostic.

### PF-08 — Reconcile stale notices against durable task state

**Sources:** L2 and preflight F2; native startup observations; P9.

Five old cmux notices about G1/provenance reached the new manager even though
the work was already accepted. Separately, two old IRC informational rows were
claimed once before delivery was enabled. P9 successfully handled that known
backlog; it does not fix general stale-notice handling.

**Recommendation:** every startup/resume reconciles handoff, issue/submission
state, accepted receipts and outstanding inbox before acting. A notice is a
pointer to work, not proof that it is current. Do not discard all acknowledged
items: unfinished acknowledged tasks still need recovery. Preserve histories.

**Acceptance:** old completed notices cause no new work; genuine unfinished
work resumes once; direct owner changes are reconciled before reassignment;
fresh and resumed sessions do not inherit another session's authority. Connect
any cmux change to PF-03; S13 owns the related executable guidance.

**Disposition (2026-10-16): resolved; accepted (CK-7).** Known backlog resolved; the startup/resume
reconciliation guidance shipped in CK-7@r3 (spec `8af3c370`); submission `1792116557723521000` (review `415016ba`, with the three-case inbox walkthrough); batch `1792116963374632000` (CI 249 workflow, chat 274, Metal; integration `d3e4fa51`), published 02:20:54Z. The PF-03
live run also showed that old notices for a retired identity keep retrying
(see PF-09).

### PF-09 — Separate skipped live checks from successful observations

**Sources:** P6h, runbook exclusions, original pilot acceptance contract.

No draft/dialog was lost during P6, but there was no deliberately present
owner draft to protect. P6h was skipped by agreement. Permission/question
dialog protection, a busy Codex interruption, reminder/dead-letter/phone-push
timing, bridge restart and IRC outage/outbox recovery were not demonstrated
live. `laz` authority and Copilot terminal delivery were also outside this
run. Existing isolated tests are useful evidence with a different scope.

**Recommendation:** before claiming unattended readiness, run the missing
draft-preservation check under owner supervision, and verify dialog protection
without answering a permission prompt. Add targeted recovery checks for the
repairs above. Test disruptive shared-service failures in isolation/staging
unless separately authorized. No need to repeat already-good owner identity
checks merely to complete a checklist.

**Acceptance:** record tested versus simulated versus skipped cases, clients,
exact message IDs, draft integrity and resulting process state. Report actual
child survival after an interrupt; do not generalize one terminated wait into
“all background work always stops.” Explicitly document any deferred coverage.

**Disposition (2026-10-16): draft and dialog protection verified live; recovery coverage open.**

Owner-supervised run, 2026-10-16 00:09–00:37Z (`.pipeline/coordination/workflow-ready/pf03-live/v1-v2-result.md`). Tested, not simulated:

| Case | Client | Result |
| --- | --- | --- |
| Owner draft present, task delivered | Claude (ck-worker-2) | Delivered through the hook; draft byte-identical for 75 s; task done once |
| Owner draft present | Codex (ck-reviewer-1) | Bridge held ("Codex prompt not empty"), typed nothing; draft and viewport byte-identical |
| After the owner cleared it | Codex | Bridge typed one wake; delivered, accepted, done once |
| `/model` picker open | Claude | Client queued the wake, no turn; picker untouched; processed once after Esc |

No permission prompt was used or answered. Delivery was ON only in short,
IRC-recorded windows and is **OFF** at the end.

**Still not demonstrated live:** a bridge-driven stop of a busy Codex turn (D2, 2026-10-16: the bridge typed the
instruction mid-turn without Esc because the surface's cmux `lifecycle` was `unknown`; steering worked, cancellation did
not, and the turn's child process survived until the agent stopped it; see
`.pipeline/coordination/workflow-ready/post-install-live/c-result.md` and the shared-bridge finding awaiting the owner's
Plateia decision), a dialog in Codex (check E), bridge restart, IRC outage and outbox recovery, and reminder,
dead-letter and phone-push timing.

**Owner disposition and bounded live check (2026-10-17).** The owner (in the ck-guide conversation, 04:04:12Z; record
`.pipeline/coordination/workflow-ready/owner-closeout-decisions-20261017.md`) accepted the remaining recovery cases as known
and untested and deferred the busy-stop fix ("note this as a future starting point"), and allowed one live check of under
20 minutes. Run 04:09:26–04:14:46Z (`.pipeline/coordination/workflow-ready/live-check-20261017-result.md`):

| Case | Client | Result |
| --- | --- | --- |
| Controlled restart of the shared bridge | all projects | pid 35940 to 30385; 32 channels rejoined from their checkpoints |
| Delivery after the restart | Claude (ck-worker-2) | Claimed after 12 s; done once |
| `/model` picker open (check E) | Codex (ck-reviewer-1) | Request held for 71 s, nothing typed or selected; after Esc, delivered, claimed and done once |

**Known untested scenarios** (future starting points; isolated tests exist where noted in the evidence package above):
the bridge stopping a busy Codex turn and its child processes (D2: the bridge sees `lifecycle unknown`); IRC server outage
and outbox replay; bridge restart with pending items for an absent recipient; reminder and dead-letter timing; phone-push
delivery and timing. Unit and fixture results are not live coverage.

**Observation (2026-10-16 ~00:40Z, superseded):** at that time ck-guide ran outside cmux, so its registry identity
was retired and messages to it retried with "no live session". **Resolved ~00:53Z:** the owner assigned a fresh Codex
session in the guide's cmux pane as ck-guide (`.pipeline/coordination/guide-handoff-20261016.md`); it is bound under the
same name, reconciled and claimed its retained rows (none dead-lettered), and now receives deliveries normally. The
shared registry still has no state for a verified session outside cmux; that design gap is recorded for its owner
(`.pipeline/coordination/workflow-ready/ck-guide-offcmux-identity.md`).

### PF-10 — Reconcile runbook and instruction drift

**Sources:** D1, O2, F8, runbook coordinator corrections and contradictory
current-status summaries.

The `sleep 300` command was refused by the worker's tool. It reported blocked;
the manager chose a condition-wait form referenced by the guard's diagnostic.
The observed foreground wait was then interrupted successfully. Record this
deviation truthfully. Future runbooks should select a harmless foreground
fixture supported by the current tool and record its child process; a refusal
is not permission to evade the guard.

The phone authenticated as `coghex`, which was already an authorized owner.
The earlier assertion that the account did not exist came from confusing the
bridge credential map with the IRC server account inventory. Both `vince` and
`coghex` are now verified. Client identity is owner-reported evidence, not
proved by message text. Omit “from weechat” and similar client labels from
future required message bodies, as requested by the owner.

Other runbook corrections already identified: modelclass options precede
class/brand; worker role uses the supported mechanism; use the observed worker
name; inspect actual binding with `pchat agent who` and `ck session`; claim
informational history without done replies; questions use a linked answer;
tasks use done/declined. Some archived wording still predicts old sessions,
accounts and visible acknowledgement behavior.

**Consolidation changes:** unique PF IDs, corrected current-status summaries,
source-qualified results, and this single working ledger. Original runbooks,
progress and verdicts remain historical evidence. New execution should use a
revised runbook, not blindly replay the archived one.

**Acceptance:** revised executable runbook/skill text agrees with current
commands and the chosen contracts, identifies skips and deviations, preserves
owner controls, and passes the applicable review gate. Ordinary explanatory
docs use `ck docs` without CI/review. Only the ordinary-doc portion is done here.

**Disposition (2026-10-16): resolved; accepted (CK-7).** Ordinary consolidation done; the executable skill
corrections shipped in CK-7@r3 (spec `8af3c370`); submission `1792116557723521000` (review `415016ba`, with the three-case inbox walkthrough); batch `1792116963374632000` (CI 249 workflow, chat 274, Metal; integration `d3e4fa51`), published 02:20:54Z. The PF-03/PF-09 run used a
new bounded fixture, `.pipeline/coordination/workflow-ready/pf03-pf09-live-verification.md`.

### PF-11 — Consider an explicit delivery pause/resume command

**Source:** preflight F3; successful R1/P10 manual toggles.

There is no dedicated Codekarl command for the delivery switch. The pilot
changed one shared-config key under the identity lock, using atomic replacement
and preserving mode 0600. That worked; this is a usability improvement, not a
proven toggle failure.

**Optional design:** provide status and explicit on/off operations with the
same locking and preservation rules. Clarify when the running bridge observes
the change; do not report effective delivery solely from a config write. Avoid
service restarts, hidden inbox cleanup or side effects on other projects.

**Acceptance if selected:** concurrent unrelated config edits are retained,
permissions stay correct, only Codekarl's flag changes, and pause retains queued
work without rings/dead-letter escalation. Test restart/readback behavior.

**Disposition (2026-10-16): resolved; accepted (CK-10).** The owner chose to add the command.
Approved scope: `ck-chat delivery status|on|off` for the owner and the
manager, with a required reason, queued work and other projects preserved, and
an IRC record. `status` must not claim the bridge's effective state from a
config write alone. CK-10@r4 (spec `70599366`); submission `1792117758439601000` (review `0c80e6a9`); batch
`1792118143758785000` (CI 249 workflow, chat 290, Metal; integration `8eeb4738`), published 02:39:14Z. Observed after
publication at the root: `tools/ck-chat delivery status` prints `{"configured": true, "config_mtime": …,
"bridge_effective": "unknown"}` with a note that this is the configured value, not proof of effect (the shared config
file's mtime moves with unrelated writers). `on`/`off` were not exercised live; delivery stayed ON.

### PF-12 — Retain the mixed/unknown model-history follow-up

**Source:** L3 from the accepted model-switch repair; `classes.readiness()`.

L1/CK-3 is complete. For a known single-family manager/worker, it verifies the
current class and retains earlier off-class contributions; reviewer checks stay
strict. However, mixed-family or unknown histories return through the older
`verify()` path before the current-pair comparison. That path classifies the
history without enforcing the current class. This is unchanged pre-existing
behavior, explicitly deferred during CK-3, not a regression shown by the pilot.

**Recommendation:** a separate gate design should enforce current-role
eligibility while retaining all contributor families and required independent
review coverage. Do not reduce provenance to the latest model or erase earlier
work. Unknown current model evidence should fail closed.

**Acceptance:** current off-class/low-effort and unresolved selections are
refused under mixed/unknown histories; valid current readiness does not launder
authorship or confer review independence. Test every supported client parser
and receipt recheck affected by the design. The existing conservative Copilot
alias-selection behavior and old receipt rechecks are separate limitations,
not silently included in this fix.

**Disposition (2026-10-16): resolved; accepted (CK-9).** Not a live-pilot failure. CK-9@r4 (spec `c80979c1`);
submission `1792113813847702000` (review `6657e67f`); batch `1792115313013817000` (CI 241 workflow, chat 274, Metal;
integration `f3cc46d6`), published 01:53:33Z. Mixed or unknown
histories must match the current class for the current model's family; an
unknown current model is refused; the history classification is kept, so
review requirements are not relaxed.

### RC-1 — Chat regressions are outside the frozen batch recipe (readiness item)

**Sources:** `.pipeline/coordination/workflow-ready/ci-chat-coverage-gap.md`;
`tools/check.py`, which discovers only `tests/`.

**The gap.** `tools/ck check` and batch CI do not run `tools/chat/`. Those
tests run only inside `tools/chat/install.py stage`, which also reads the
installed shared chat code. CK-4's chat evidence was therefore separate
worker and reviewer stage runs (252/252), not batch coverage.

**Required before S3 or readiness:** a bounded, reviewed design that brings
the relevant chat suite into the full batch recipe, or an explicit
owner-agreed alternative. It must:

- bind the exact shared-chat inputs to the batch receipt and the install
  drift check;
- avoid introducing an untracked mutable dependency;
- include a negative proof that a failing chat regression fails the batch.

**Disposition (2026-10-16): resolved; CK-6 accepted (batch `1792113249834006000`).**

- The full recipe runs the staged chat suite (`install.py ci`), invoker-independent (identity variables removed);
  shared-input digests are recorded per run and, from the next batch on, bound in the CI receipt and rechecked
  before publication. This first batch was prepared by the previous gate code, so its receipt is unbound (stated in
  its integration review).
- The first attempt, batch `1792111779412964000`, failed and was cancelled: CK-5's tests inherited the invoker's
  `CHAT_AS`. Fixed in CK-5 and CK-6@r3; the replacement batch passed while prepared with `CHAT_AS` set
  (`.pipeline/coordination/workflow-ready/batch-1792111779412964000-failure.md`).
History: CK-6 spec r2 (`553ce644`) and r3 (`66ea6adb`, invoker-independent environment); accepted submission
`1792112716225818000` (review `5e908010`), whose live `ci` step passed (50 files) and whose deliberately broken
chat test failed the full check. Earlier submissions `1792108261547871000` (changes requested) and
`1792111517041091000` (quarantined in the cancelled batch) are superseded.

### RC-2 — Eligibility trusts submitter authorship instead of patch provenance (readiness item)

**Source:** ck-guide finding and real-Pijul probe,
`.pipeline/coordination/workflow-ready/provenance-eligibility-finding-20261015.md`;
verified independently by the manager.

`ck submit` records the submitting session as the author and leaves every
provenance slot `null`; eligibility trusts those manifest authors. The review
gate plan requires authors to be recomputed from each immutable patch, with
unknown patches needing both families' reviews. A raw patch submitted by a
Codex session therefore became eligible with one Claude review. CK-4's and
CK-5's actual patch provenance was checked by hand and is correct.

**Disposition (2026-10-16): resolved; accepted (CK-8).** CK-8@r2 (spec `976dc70d`); submission
`1792114719342642000` (review `717cfc0d`); batch `1792115313013817000` (integration `f3cc46d6`), published 01:53:33Z.
That batch was prepared by the previous controller, so its integration review hand-checked provenance; from the next
submission on, manifests carry per-patch provenance digests and eligibility derives authors from them (observed in
CK-13's manifest).
Until it is accepted, every submission review checks patch provenance from the capture by hand.

### RC-3 — Bootstrap quiescence scan matches unrelated commands (readiness item)

**Source:** CK-9 submission review replay (2026-10-16): a full check failed bootstrap test `c27` because
`procscan.scan` treats the last word of any `ck` command as its subcommand, so a concurrent read-only
`tools/ck session --role worker` counted as a state writer. Some bootstrap tests also read the real process table.
Evidence: `.pipeline/coordination/workflow-ready/finding-bootstrap-quiescence-flake.md`.

**Disposition (2026-10-16): resolved; accepted (CK-13).** CK-13@r2 (spec `dc34be67`); submission
`1792115841130127000` (fix 1, batch-test guard; review `829b9035`, with a final full check under a concurrent
authenticated `tools/ck session --role worker` loop); batch `1792116357358030000` (CI 249 workflow, chat 274, Metal;
integration `ccd591bd`), published 02:09:11Z. The scanner now matches the real `ck` subcommand and tests never read
the machine's process table by accident. S3 (CK-11) is unblocked.

## 4. Resolved prerequisites and preserved boundaries

| Earlier concern | Current disposition |
| --- | --- |
| G1 docs sync | Accepted before the pilot; not reopened here |
| CK-1 Copilot provenance | Accepted; model evidence support does not imply Copilot chat/wake support |
| L1 model-switch readiness | CK-3 accepted and closed; submission `1792090900847282000`, batch `1792091100586830000`; 182 Python tests plus Rust/Metal validation in the accepted run |
| F1 Copilot chat unsupported | Pilot unblocked by choosing native clients; no Copilot adapter implemented |
| F2 historical IRC rows | Reconciled by P9; general stale notices were PF-08 (accepted) |
| F4 protected header surfaces | Left alone; no pilot delivery to them observed |
| F5 guide effort observation | Owner's guide settings preserved; not a dispatched role-setting repair |
| F6 old Codex worker/reviewer pairing | Old worker left held; new native worker used opposite-family Codex reviewer; separate Claude reviewer readiness checked |
| F8 owner identities | Resolved by actual authenticated `vince` and `coghex` messages |

No pilot patch was recorded and no session was replaced during the final run;
P7 explicitly exited and resumed the existing worker conversation. The earlier
owner-directed client transitions and CK-3 repair have their own evidence.
Pilot artifacts, sessions, old queue items and other projects' state remain.

## 5. Suggested way to work through this document (historical)

All five steps are done: every PF item is closed in §3 by an installed result or an owner disposition (2026-10-17). The
recommendation is kept as written.

1. Resolve PF-01's state contract and PF-04's unknown-kind policy, then create
   narrowly scoped CK specs for any Codekarl changes chosen for implementation.
   Decide PF-05's room presentation alongside the delivery design.
2. Investigate PF-02 and PF-03 with bounded reproductions. Combine PF-03's
   recovery requirements with PF-08. Establish shared-source ownership with
   Plateia before modifying installed chat or cmux code.
3. Review and implement chosen repairs through the existing gates. Each issue
   spec and immutable submission needs opposite-family Class A/high review;
   independent approved submissions can share one frozen full CI batch,
   followed by integration review. Update skills/runbooks through their code
   lane. Ordinary docs remain ungated.
4. Resolve PF-06/PF-07/PF-10 wording and diagnostics without granting new trust.
   Decide whether PF-11 is useful now. Keep PF-12 a separate gate concern.
5. Run targeted live verification for changed behavior and PF-09 under the
   owner's control. Record exact evidence and unresolved limitations, then
   decide readiness and the next S0–S13 batch with the owner.

This ordering is a recommendation, not dispatched work. Keep one owner and one
status for each PF item. Add its CK issue (or shared-project tracking location),
spec digest, reviewed submission, installed receipt and verification result
under its disposition as it progresses. Closing a finding requires evidence;
planning its fix is not closure.

## 6. Old finding IDs mapped to this worklist

| Original label/source | Consolidated home |
| --- | --- |
| O1 (manager), delivery-history gap (coordinator) | PF-01; acknowledgement presentation separated into PF-05 |
| O2 / F8 phone expectation | PF-10; authentication resolved in §4 |
| O3 incidental status mention | PF-06 |
| **O4 coordinator: P5 empty inventory** | **PF-02** |
| **O4 manager: bridge untrusted diagnostic** | **PF-07** |
| O5 bundled/missed cmux instruction | PF-03, with mechanism explicitly unresolved |
| D1 sleep guard | PF-10; P6 outcome retains the deviation |
| F1 unsupported client | Resolved choice / retained limitation in §4 |
| F2 historical inbox and L2 stale notices | PF-08 |
| F3 manual switch | PF-11 |
| F4/F5/F6 | Preserved boundaries in §4 |
| F7 unknown kinds | PF-04; now reproduced offline, still not live-tested |
| L1 | Accepted CK-3 in §4 |
| L3 | PF-12 |
| P6h and deliberate live-test exclusions | PF-09 |
| Early runbook coordinator corrections | PF-10 |

## 7. Source and evidence index

Paths under `.pipeline/coordination/` are retained local operational evidence,
not independently versioned publications. The findings and essential evidence
are now in this Pijul-tracked document so it can be edited and worked through
without relying on scattered status summaries. Preserve original files.

| Source | Role in consolidation |
| --- | --- |
| [Pilot result](../../.pipeline/coordination/live-pilot-result.md) | Final manager assessment, D1 and manager O1–O5; old R0 blocker archived below it |
| [Pilot progress](../../.pipeline/coordination/live-pilot-progress.md) | Chronological timestamps, message IDs, process observations and cleanup |
| [Preflight](../../.pipeline/coordination/live-pilot-preflight.md) | F1–F8 and installed behavior as observed before native bring-up; stale claims corrected here |
| [Coordinator findings](../../.pipeline/coordination/pilot/p2-audit-findings.md) | Verified delivery cause, visible-ack proposal, phone correction and coordinator O4 |
| [Archived runbook](../../.pipeline/coordination/tasks/ck-live-pilot-runbook.md) | Original exact criteria, stop rules and explicit exclusions |
| [Runbook coordinator check](../../.pipeline/coordination/live-pilot-runbook-coordinator-check.md) | Launch parsing, role/binding and reply/claim corrections |
| [Original pilot task](../../.pipeline/coordination/tasks/ck-live-pilot-and-hardening-plan.md) | Scope and acceptance expectations; planning does not authorize implementation |
| [Native verification](../../.pipeline/coordination/native-session-verification.md) | Original L1 failure and historical startup/model evidence |
| [Model-switch result](../../.pipeline/coordination/model-switch-repair-result.md) | Accepted CK-3 receipts, retained provenance and L3 limitations |
| [Consolidation audit](../../.pipeline/coordination/pilot/consolidation-audit-2026-10-15.json) | Read-only snapshot: pilot messages, recipient rows/events, relevant delivery/ack logs, source hashes |
| [Unknown-kind probe](../../.pipeline/coordination/pilot/unknown-kind-analysis-2026-10-15.json) | Isolated PF-04 reproduction and installed/source equivalence |
| [Chat contract](../chat-workflow.md), [agent policy](../../skills/AGENT-POLICY.md) | Intended behavior and existing role/review constraints |

Audit snapshot SHA-256:
`88b8a63e5947e9e3e27caf0961a7c5e2bfbb99585cf715e2ad477e3badf68420`.
The current owner-action file is a replaceable handoff, not historical evidence.

Underlying stores: `~/.local/state/chat/codekarl/inbox.sqlite3`,
`~/.local/state/chat/{deliveries,acks,dead-letters}.jsonl` and the bridge log.
The scoped snapshot includes only this pilot's relevant records and no
credentials. Provider transcript evidence uses these logical sessions:

- Manager: `44a0d62f-8841-406e-af97-3140ed5de9f6`, under
  `~/.claude/projects/-Users-<user>-work-codekarl/`.
- Worker: `4864d6a5-60e5-48e4-a687-81cec632faba`, under
  `~/.claude/projects/-Users-<user>-work-codekarl--pipeline-workers-copilot-provenance/`.
- PF-02 failure event: `e1f405b4-7212-4bc7-8329-d2595526ea3f`, tool use
  `toolu_01AK9xBAYNwP4uEMDfD2iPJE` at 19:30:10.935Z.
- PF-03 send tool use: `toolu_01NFp3hxZApcgYirxUZNcJVn`; read-state tool
  result: `toolu_01KVdmYgxKksk82jekgfyqmE`; worker recovery event:
  `1d82d94c-eecb-499d-94e1-20239bee8898` at 20:00:00.714Z.

### Primary message IDs

| Purpose | Message ID |
| --- | --- |
| P1 owner status | `qr5piyxin2234d6igd2g43wxha` |
| P2 phone request / done | `jkn5yvp4453zibynpvp7e9hsis` / `2qs5av9qsqf6fb88xbn6drvs66` |
| P3 append / done | `bvpbg44gehuyr26dptkp6px7te` / `rv3fmuicpsgu7bniaxeskk9pa2` |
| P4 question / answer | `3mxqekdd4kfhawgrf3kfx6ztun` / `qaiu75bg975awe8q36fgy4z2gn` |
| P5 owner request / done | `53mjhiku49gzgt4ask6cm9g79w` / `75gfikrjsgqj4q5gwieg4j34e2` |
| P5 incidental mention | `i8vrgc8xr3fehcid2piweb7tvs` |
| P6 task / blocked / alternative answer | `sz9kke2yt9gdq2qb8pyb3pbbha` / `6cjiin8mtpigkf2943bai5j4bw` / `yg2e9bc9hakv6yj47sadrw8vhs` |
| P6i interrupt / bridge status / visible ack | `2d4g6f8mk97jhaekw2khd4izu6` / `hdgvrma6h5ktwp5gnynmteh6xn` / `d9m8a26fx3cxhnih89yg9fuibe` |
| P6 decline / manager done notice | `iiikax4gaat29uu7pza8zb9hws` / `hczegktcgj37bh478vpgmqpk8n` |
| P6i eventual completion | `y98f5gkkxa9stiuz8axe558niw` |
| P7 resume request / done | `g37y5257i4dbcvjivvbskbdkje` / `ugwpfj4x3pfby2krxxyd6zck8i` |
| P9 old informational rows | `5adw2kiyvw9h6s453e2jbvbfxn` / `wypw7wdkdjxuk4d7pdqetncpcn` |
