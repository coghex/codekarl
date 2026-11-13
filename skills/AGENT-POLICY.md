# Codekarl agent policy

This is the master role and review policy for Codekarl. Read it at skill entry,
after resumption, and before dispatch or review approval. It applies only to
Codekarl; do not apply the production GitHub manager workflow here.

## Classes, brands and effort

| Work | Class | Default brand | Effort |
| --- | --- | --- | --- |
| Management and design | S | Claude | high |
| Implementation | B | Claude | high |
| Independent review | A | Opposite the implementation brand; normally Codex | high |
| Guide review (advisory) | S | Claude or Codex | high |

Guide findings are advisory and grant no review, acceptance or repair authority.

The owner can choose either brand for a role. Class and review independence are
requirements; the default brand is a convention. Design performed inside a
worker task still belongs to Class S: return substantial unresolved design to
the manager instead of silently designing it as a Class B worker.

Brand means the actual model family, **not the CLI or billing provider**. Opus or
Sonnet in Copilot counts as Claude; an OpenAI model in Copilot counts as Codex.
Record the client separately (`claude`, `codex`, `copilot`, etc.). A compatible
client is canonical when its actual model and effort match the resolved class.
Verify session model/effort evidence, not just a launch default or terminal title.
Automatic routing or a model switch must be reconciled in the author/reviewer
ledger; an unknown model family cannot satisfy opposite-brand review.

`~/.config/model-classes.toml` is the sole authority for current model IDs.
Read the shared `model-classes` skill and run `modelclass check` and
`modelclass list`. Resolve at every launch; never copy model IDs into these
skills, prompts or launch scripts. Use `--effort high` for Codekarl even if a
class default later changes. A missing class/model is a blocker, not permission
to substitute a different class or brand.

Record the resolved class, brand, model and effort with the task/session evidence.
Owner changes to a live session take precedence. Preserve them and reconcile
with the owner when they affect the task or review; do not reset settings or
restart a conversation to enforce a default. A model update affects future
launches; it does not restart existing agents. For an existing Copilot session,
verify its selected model and high effort against the resolved class and retain
the session. The native model spelling may differ; aliases belong in the master
model file, not in individual skills. `modelclass run` currently launches only
the native Claude/Codex clients; it does not launch Copilot.

## Review independence and freshness

Every implementation, including executable skills, needs independent Class A,
high-effort review from the opposite brand before acceptance. Claude-authored
work goes to Codex; Codex-authored work goes to Claude. The manager's brand is
irrelevant to that choice. Never infer authorship from a Pijul signing key: all
local agents share the repository's signing identity. Record actual author
brands and session identities in the task handoff.

The reviewer must be a separate session and must not have implemented the
reviewed change. For mixed or unknown authorship, obtain independent reviews
from both brands; each contribution needs opposite-brand coverage. A reviewer
who writes a fix becomes an author of that fix and cannot approve it alone.

Review an immutable submission, not a moving worker directory. A verdict names:

- task ID and specification, submission ID and exact patch hashes;
- SHA-256 of the queue manifest bytes (binding its spec and full fingerprint);
- author and reviewer identities/brands, reviewer class/model/effort;
- examined base, scope, findings and meaningful validation evidence;
- APPROVED or CHANGES_REQUESTED, with unresolved findings stated plainly.

Changed code, scope or specification needs a new review. Base advancement alone
does not require another review of identical patches; acceptance still checks
their combined state with the planned selection; each milestone is completed
with one full CI run. A conflict resolution is new implementation
and needs review. A chat acknowledgement, a worker's done message, or passing CI
is not review approval. Ordinary documentation uses `ck docs` without review.

## Review-gated acceptance

After activation, `ck issue` stores immutable issue specs and review events in
the sibling Pijul metadata repository. A spec must have fresh independent
Class A/high coverage before its issue is ready. Code authors use `ck record`
and `ck submit --issue CK-N[,CK-M] --spec TEXT`; raw-recorded changes are
unknown authorship and require both review families.

`ck batch prepare` freezes only eligible submissions and runs the planned check
selection for their changes against the frozen base, binding that selection to
the CI receipt. It leaves the CI slot free while an independent integration review is
pending, unless the batch is a single-submission one (`ck batch status` shows
`publication_basis: single-submission`): its submission review is then the basis
and no separate integration review is requested. Rejected members named with `--blame` are blocked; unaffected members
keep their approvals. `ck batch publish` rechecks the base, exact membership,
captures, CI receipt, reviews and issue state under the publication locks before
it atomically advances the v2 pointer. `ck accept` is removed. A changed code
set or spec needs a fresh submission/review; never edit manifests or pointers.

Ordinary docs use `ck docs` and queue during batch review or CI. They publish
after the code pointer; failure and cancellation drain them onto the unchanged
code. Use `ck docs flush` for recovery and `ck docs resolve` for retained
conflicts. Do not manually edit accepted pointers.

The live chat pilot (2026-10-15) and the environment-readiness goal
(2026-10-17) are complete, and their setup hold is lifted. The manager
dispatches owner-authorized work through the normal review and acceptance
workflow. The limit is the owner's assignment: the manager does not choose its
own backlog or take on work beyond what the owner assigned, and gains no new
daemon, session or launch authority. Within an assigned goal it carries the
work through to completion (dispatch, review routing, batch and publication)
without asking the owner again. Every spec and submission still needs its
review, and publication still needs the gate's checks. Passing CI alone does
not authorize publication. The owner's deferrals (busy Codex stop through chat;
the untested live recovery cases) remain known limitations, not tasks.

## Rendering backends

Owner decision, 2026-11-09: until a native Linux machine with a real GPU is in the test loop, Vulkan is compile-only here (on this Mac it
would test MoltenVK, not Linux).

- **Render features land on native-metal.** A spec no longer requires "on both backends". The Vulkan catch-up (CK-239) and the Linux
  enablement are **parked until the owner says so** (owner, 2026-11-09, `m9-design/owner-direction-linux-r1.md`): they are not a
  milestone gate, and no Vulkan port is dispatched at a milestone's end.
- **The Vulkan path keeps compiling.** A change to a backend-neutral interface (`ck-render-2d`, `ck-window`) or to a shared GPU data
  format still updates the Vulkan code enough to keep compiling; Clippy (every workspace target, all features) is the compile check.
- **Its GPU and window groups are optional** (`glfw.window`, `vulkan.gpu`, `vulkan.window`, `render2d.vulkan-window`, `vulkan.app`): no
  planned check, level or batch selects them, so a Vulkan GPU or window failure does not block a submission, and no GPU/window slot run
  is spent on them. Name one with `--group` only when the owner or manager asks.
- No MoltenVK performance work. Vulkan stays compile-only; the owner decides if and when the groups and parity return.

## Visible sessions and communication

Use the existing Codekarl cmux group and owner-provided terminals, preserving the
two-pane layouts. Read `cmux-supervisor` before terminal operations. Resolve live
workspace/surface UUIDs; use explicit targets and preserve focus, drafts,
permission dialogs and histories. Do not create, move, clear or replace sessions
to compensate for an unavailable slot. Clearing at a task boundary under
"Session contexts" is not that. Ask for the destination when ambiguous.

Launch through `modelclass run` with explicit Codekarl identity roles. These
commands illustrate role/class selection, not authority to type into a terminal:

```sh
modelclass run --effort high --chat-project codekarl --chat-role manager S claude -- '/ck-manager'
CHAT_ROLE=worker modelclass run --effort high --chat-project codekarl B claude -- '/ck-work'
modelclass run --effort high --chat-project codekarl --chat-role reviewer A codex -- '$ck-review'
```

For Codex implementation select a Claude reviewer instead. The installed
resolver's CLI does not expose `worker` in `--chat-role`; its chat integration
accepts the explicit `CHAT_ROLE=worker` environment variable. Never inherit
another session's `CHAT_AGENT_ID` or impersonate an identity. Confirm the bound
identity, role, provider session ID and actual launch flags before dispatch.

Client support is separate from model eligibility. The current shared chat
adapter does not discover or safely wake Copilot. Keep terminal delivery paused
for a Copilot manager until an adapter is tested; do not pretend it is Claude
Code to make the bridge type into its prompt. In that degraded state it may read
history with `ck-chat inbox --all --history`, maintain its handoff and work with
the owner directly, without impersonating an IRC identity. Mark this degraded
communication state explicitly; it is not a completed live chat pilot.

All coordination uses `ck-chat` in `#codekarl`. Use the authenticated provenance,
durable inbox and claim protocol in that skill. The owner can work directly with
any agent; the manager reconciles that work rather than sending competing tasks.
Use stable task IDs and durable handoffs. A terminal is a location, not a task
identity. Do not register Codekarl in the old GitHub manager's state directory.

## Session contexts

Every turn re-reads the session's whole context, so sessions are cleared at task
boundaries. A cleared session keeps its chat name, and its old conversation stays
resumable.

| Role | Clear when | Never while |
| --- | --- | --- |
| Worker | its issue is accepted and nothing else is assigned | a submission of its is in review, or a build is in flight |
| Reviewer | what it reviewed is accepted or approved and no rereview is owed | a changes-requested round is open |
| Designer | its spec is approved or its docs note is published, and its start-up brief is current | it is in conversation with the owner on a design |
| Manager | nothing is in flight (no batch, no slot held, nothing in review) and its brief is current | any of those is open |

- **Who:** the manager clears workers, reviewers and designers, following
  `~/.codex/skills/cmux-survey/references/recovery.md` each time. The owner clears
  the manager, after the manager says it is ready. No session clears itself, and a
  worker, reviewer or designer clears no one. The owner may clear any session at
  any time.
- **Before:** the session is idle with an empty prompt and holds nothing
  unrecorded. A designer and the manager keep a start-up brief at
  `.pipeline/coordination/session-clears/briefs/<chat name>.md`; a designer is
  asked to confirm it is ready.
- **After:** the first message to a cleared session names what to read (this
  policy, its role skill, or its brief). The manager types a one-line pointer into
  its empty prompt, because a cleared session is not waiting on chat until it has
  taken a turn; for Codex that first turn is the identity check below.
- **Record:** one private action record per clear in
  `.pipeline/coordination/session-clears/` (the old session id and how to resume
  it), and a line in the manager's handoff.
- **Codex:** Codex sessions are cleared under the same table as Claude Code
  sessions (owner, 2026-11-13). After a Codex clear the chat name returns only once
  the registry has seen the new thread. So the manager posts nothing to the session
  first: it types a one-line pointer asking for `pchat agent who --json`, lets that
  turn end, runs `pchat agent sync`, and checks with `pchat agent list --json` that
  the old name carries the new thread and is the only active name on that tab. Only
  then does it send the first message. If the name has not returned after one more
  sync, it stops and reports. A session launched without a model flag takes the
  client's current default model after a clear; the manager checks the session's
  model and effort against its class before giving it work, and reports a change to
  the owner.
