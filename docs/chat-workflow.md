# Codekarl chat workflow

Status (2026-10-17): the supervised native Claude/Codex pilot finished on 2026-10-15; every finding is repaired or
closed by an owner disposition (PF-01 to PF-12; see the closed [consolidated pilot findings](archive/live-pilot-findings.md)).
On 2026-10-17 a bounded live check passed: a controlled restart of the shared bridge, delivery afterwards, and the Codex
dialog check. The owner accepted the remaining recovery cases as known and untested, and deferred stopping a busy Codex
turn through chat (stop it manually). **Delivery is ON** as the routine state since 2026-10-16 01:24:52Z;
`tools/ck-chat delivery status` shows the configured value. The `ck-workflow-ready` goal is complete; new work starts from
an owner assignment.
The owner-created two-pane workspaces and conversations are preserved.
Copilot provenance is supported by the gate; Copilot chat discovery/wake
support was not implemented. The bootstrap conversation is `ck-guide`.

Read [agent-workflow.md](agent-workflow.md) and the executable master policy at
[skills/AGENT-POLICY.md](../skills/AGENT-POLICY.md) for model classes, high effort,
model-family review independence.

## Scope

Codekarl uses the existing local Ergo server, WeeChat, The Lounge/Tailscale,
`pchat`, chat bridge, history logs and cmux transport. Its routing policy is
`codekarl-v1`. It introduces no new daemon or plugin. Other projects retain
their existing routing and identities.

The source for the extension, installation adapter, tests and `ck-chat` skill
is tracked here in Pijul. Shared bridge edits are narrowly applied to the
existing chat installation after tests and Codekarl acceptance. Existing local
edits are preserved; a changed shared file forces restaging. The installer
keeps the original files under `~/.local/state/chat/codekarl-install/`.

## People, identities and authority

| Actor | Codekarl role |
| --- | --- |
| Authenticated `vince` or `coghex` | The owner, preserving the actual account and nickname in the record |
| Authenticated `laz` | The owner's assistant, subject to the owner's current instructions and reserved decisions |
| `ck-manager` | Coordinates authorized Codekarl work and assigns workers/reviewers |
| `ck-worker-N` | Implements assigned work and communicates with peers |
| `ck-reviewer-N` | Independently reviews assigned work and communicates findings |

Authority uses the server's account tag, never a visible nickname. If The
Lounge connects as account `vince` with nickname `coghex`, both are retained.
If it connects as account `coghex`, that account is recognized too. The pilot
verified both owner accounts: `vince` in P1/P5/P6i and
`coghex` in P2. The client used is recorded separately from the authenticated
account, based on the owner's report. Legacy global assistant names
`claude` and `codex` do not receive Codekarl authority merely from the old
shared allowlist.

Roles are registry data. Workers and reviewers have independent counters;
numbers are never reused. A role stays with its logical session across moves,
setting changes and explicit resumption. A fresh session receives a new number.
The manager has a stable name; only its current registered session receives
delivery. Future roles need an explicit policy decision before allocation.

The shared macOS account can access agent credentials. Account authentication
provides provenance, not security isolation between local agents.

## One room and explicit routing

Use `#codekarl` for project communication. No request channels are required.
Agents include a task reference with `--re` when one exists. The owner can
write ordinary messages without special syntax.

| Message | Delivery |
| --- | --- |
| Owner/laz names an agent | That agent; the manager reconciles direct owner work from the record |
| Unaddressed owner/laz request | `ck-manager` |
| Manager names a worker or reviewer | The named agent |
| Worker/reviewer names a peer | The peer; the message grants no new assignment or approval authority |
| Unaddressed worker question, blocker, completion or decline | `ck-manager` |
| Routine unaddressed status, acknowledgement | Recorded, no wake-up |
| Reply without an explicit mention | The original agent sender when actionable; routine status replies stay quiet |
| Unknown or out-of-project recipient | Diagnostic in `tools/ck-chat status`; never silently reassigned |
| Recipient is offline | Its durable inbox retains the message; delivery retries can eventually require attention |

An explicit recipient can receive a status update without waking everyone else.
Worker questions/blockers directed to the owner also reach the manager and use
the existing content-free phone notification. Ordinary status/completion posts
do not push the owner. Laz continues to consume its own mentions through its
existing mechanism; this extension does not create a new laz terminal.

Conversation, routing and authority are separate. A peer can ask a question;
it cannot authorize an acceptance or impersonate the owner by quoting them.
The manager checks current authorization before dispatching. Repository success
comes from `ck` receipts, not a chat `[done]` line.

## Durable lifecycle

Every routed message has the server message ID and a separate record for each
recipient. The inbox is `~/.local/state/chat/codekarl/inbox.sqlite3`. IRC history
and the shared JSONL record remain the human-readable development history.

States are `queued`, `delivered`, `acknowledged`, and `completed` or `declined`.
A failed notification is visible alongside the still-outstanding message.
Delivery observations are recorded independently of task progress (CK-4, PF-01, installed and verified live on
the Claude and Codex paths): `waiting`, `delivered`, `accepted`, `attempt-failed` and `failed`, with `notice` derived
from them. For a Claude recipient, `delivered` means the message was handed to the client's queue, not that the
agent has processed it; processing evidence is the transcript turn and the claim.
Delivery does not imply acknowledgement; acknowledgement does not imply work
finished. Completion of chat work does not imply accepted code.

The inbox records requests transactionally before creating terminal notifications.
The bridge can rebuild missing notifications after restarting. A duplicate IRC
message retains its original routing. Replies and acknowledgements belong to
the exact original message and recipient; out-of-order history is reconciled.

Before performing an action, a recipient runs `tools/ck-chat claim MESSAGE_ID`. It posts the IRC reaction for every
message and one visible `[ack] claimed by <name>` line for tasks, questions, blockers and interrupts (CK-5, PF-05);
informational claims stay reaction-only. It prints one JSON envelope, `{"claim": {...}, "effects": {...}}`, for fresh
and duplicate claims alike; freshness is `claim.fresh`, and a duplicate claim exits 4 and posts nothing. Other exits:
3 queued in the outbox (not lost; do not repost); 5 NOT SENT (identity discovery failed first; rerun); 6 an
acknowledgement effect is not confirmed (the claim stands; inspect with `ck-chat effects MESSAGE_ID`). Acknowledged but
unfinished work stays visible, including for unknown bracket kinds (CK-4, PF-04). On a replay or restart, the agent
inspects its history and actual repository state before resuming.
This is at-least-once notification with durable deduplication and recovery, not
a claim of exactly-once external execution across crashes.

Acknowledged informational statuses/results leave the outstanding view without
requiring another completion reply. They remain in history. An `[answer]` to a
`[question]` completes that question; a task still needs done or declined.

`tools/ck-chat done MESSAGE_ID "result and evidence"` and `decline` publish
terminal replies. The bridge records them once it sees the authenticated IRC
message. If IRC is down, the existing outbox retains the post (exit 3). Do not
retry by posting a new copy repeatedly.

## Interruptions and owner control

Normal messages queue attention. `!! @ck-worker-7 hold` is an explicit interrupt,
permitted for the owner, laz and the Codekarl manager. Existing cmux draft,
dialog and transcript-browsing protections remain in place. Chat cannot answer
permission prompts, steal focus, or overwrite a human's draft.

Stopping a foreground turn does not guarantee background tools stopped. The
recipient reports what actually stopped and inspects external outcomes before
continuing. The manager never undoes an owner intervention or reassigns that work
without reconciling it. Keep resumable histories and respect live setting changes.

## Inspection and recovery

```sh
tools/ck-chat status
tools/ck-chat inbox
tools/ck-chat inbox --all --history
tools/ck-chat delivery status
pchat read '#codekarl' --last 30 --json
```

`tools/ck-chat delivery on|off --reason "..."` (owner or manager; CK-10, PF-11) changes Codekarl's delivery setting with
an IRC record and keeps queued work; `delivery status` reports the configured value, not proof that the bridge has
applied it.

`status` is the authoritative Codekarl delivery view. `pchat status` remains
useful for shared service health, but its legacy manager-only unaccepted summary
is not Codekarl's per-recipient lifecycle.

Only a bound Codekarl identity can claim or finish its messages. A manager can
inspect all inboxes and request recovery, but cannot claim another actor's work.
An intended recipient can claim an outstanding message even after its terminal
notification exhausted retries. Do not delete the inbox, reset counters or
rewrite accepted state to clear a warning.

## Installation and workspace checkpoint (historical, 2026-10-14)

(The original installation procedure, kept for reference; steps 4–6 were completed during the 2026-10-14/15 rollout
and pilot. Adapter updates now go through `install.py stage` and `apply` after an accepted batch, as for CK-5; the
latest install was on 2026-10-17, after S7d, giving aggregate `f133dc42…`.)

1. `python3 tools/chat/install.py stage` copies the current shared chat code,
   applies the narrow adapter and runs its isolated regression suite. It cannot
   reach live state, IRC or cmux through that test suite.
2. Record and submit the Codekarl code; run the full acceptance gate once.
3. Synchronize the root, then run its `tools/chat/install.py apply STAGE_PATH`.
   The installer verifies the accepted source and tested outputs, refuses shared
   file drift, preserves backups, and exposes the `ck-chat` skill.
4. Register the Codekarl project with delivery disabled and load the adapter in
   the existing bridge. The room can record messages while terminals remain
   untouched. No agent identities are allocated just for a mock test.
5. **Stop for the owner to create the workspaces.** Use one `codekarl` workspace
   group, with an obvious manager shell, worker shells and a reviewer shell.
   Suggested names: `codekarl manager`, `codekarl work`, `codekarl review`.
   Two worker panes/tabs are enough initially. The owner chooses windows/panes;
   fresh UUIDs will be resolved after creation. Leave shells empty for now.
6. Bind explicitly chosen roles to the actual sessions, implement/load the
   remaining manager/work/review skills, and enable a bounded live pilot. Test
   WeeChat and phone identity, manager dispatch, peer reply, owner takeover,
   acknowledgement, interruption and session recovery before general dispatch.

The hardening plan and its review-eligibility work are separate from this chat
foundation. No game or hardening task starts merely because chat is available.

## Historical chat foundation verification (before the agent-policy rollout)

- Accepted change: `2WF33GF4P4BYVDN4W2XVNIROEQ6MO22OC4PBTYG4LVOZK2C2JIHAC`.
- Passing CI receipt: `.pipeline/checkpoints/1791995909789481000-06fa760e.json`.
  The full gate passed 46 project tests (19 pipeline and 27 chat policy/recovery),
  formatting/lint checks, the Rust build, and all six executable/Metal probes.
- The staged shared-chat suite passed 230 tests, including 18 new bridge/CLI
  integration tests. Its existing live-state/network/terminal isolation guard
  remained enabled. Old project routing, draft/dialog protection and delivery
  recovery tests passed.
- Installed shared-file/config backup:
  `~/.local/state/chat/codekarl-install/1791995944592059000/`.
- Live authenticated setup message: `ymfra3rbhiyvkq9ykju74aa8jw` from `ck-guide`
  in `#codekarl`. It appears once in both the IRC log and the durable inbox;
  its informational status generated no terminal recipients.
- At this checkpoint, `codekarl.delivery_enabled` was false. Phone authentication and actual
  manager/worker/reviewer terminal delivery await the owner-created workspaces
  and bounded pilot. Neither mock tests nor the IRC logging check prove those
  terminal interactions yet.
- The original review handoff is preserved at `.pipeline/review-handoff.md`,
  moved before synchronizing the accepted root.

The original next step was to resolve workspace/surface UUIDs, prepare the
remaining scoped skills, and bind chosen live sessions. Do not
register this project in the GitHub manager's state directory: Codekarl delivery
resolves its manager from the chat identity registry and does not use the old
manager nudge/reconciliation service.

## Agent-policy rollout, 2026-10-14

- Accepted patches: `4JPVKWDSYL2RDVEYPDX2AFO57NV324H73POYBEVMYTJJVNGMFD5AC`
  and `JR237IECJ6VGUI7TPXRMJ23IFJLVI6MJORUPBNTR4VNHK6C3AQWQC`.
- [Independent bootstrap review](archive/reviews/agent-workflow-bootstrap.md) and
  [focused re-review](archive/reviews/agent-workflow-rereview.md): APPROVED by Class A
  Claude-family through Copilot, opposite the Codex author. The re-review
  approves the final composition and resolves the alias/degraded-mode findings.
- Both submissions passed one combined full gate. Receipt:
  `.pipeline/checkpoints/1791997897795199000-0ca66d56.json`. Formatting, Python
  linting, Clippy, Rust tests/build, 46 Python tests and six CLI/Metal probes pass.
- Five accepted skills are linked into Codex, Claude Code and Copilot:
  `ck-manager`, its alias `ck-manage`, `ck-work`, `ck-review` and `ck-chat`.
  Copilot also has the existing model-classes and cmux-supervisor dependencies.
  No plugin was added. Copilot lists all seven as enabled.
  (Later, CK-51 added a sixth accepted skill, `ck-guide`, the advisory guide; this
  entry records the state at the 2026-10-14 rollout.)
- Existing main-left Copilot session:
  `383752b5-300f-4a94-b6e7-f2fce6c58ca7`, Opus 5.5/high (Class S), verified from
  its session events. It completed readiness, wrote its durable handoff and is
  waiting for direct owner input. It is canonical by model family, but not yet
  bound to IRC.
- Native Codex reviewer in review-left: `ck-reviewer-1`, session
  `01a13b66-9dbd-7292-b258-1ed2dd024e63`, Class A GPT-6.1-Sol/high. It posted
  readiness to `#codekarl` and is idle. The independent Copilot bootstrap
  review remains available in review-right with its complete history.
- Worker panes are untouched. No hardening or game tasks were dispatched.

At that rollout checkpoint, Copilot discovery/identity/delivery and the live
pilot were still pending. The owner later selected native clients and completed
the pilot; see [the current consolidated findings](archive/live-pilot-findings.md).
Copilot chat support remains unimplemented.
The earlier generic-discovery collision on the two Codex header sessions was
handled by the accepted [identity repair](archive/chat-identity-incident.md). Preserve
the owner's extra session. Readiness posts alone still do not prove live
delivery; use the consolidated pilot evidence for the tested scope.

The manager's runtime handoff is `.pipeline/coordination/handoff.md`; the setup
ledger is `.pipeline/coordination/tasks/ck-setup-agent-policy.md`. The
mechanical review gate is installed, and the consolidated pilot report is closed.
