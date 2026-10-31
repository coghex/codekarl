# Codekarl review gate and issue tracker: specification v5

**Status: v5 contract, installed. It was approved by independent Codex Class
A/high specification review round 4 (`spec-review-4.md`) and implemented by
submission `1792012152768312000-52be4dd8`. That submission was approved by
Claude-family submission review round 4 (`submission-review-4.md`) and
activated through the bootstrap helper (§11); see
`.pipeline/coordination/gate-install-result.md`.** This version replaces the v1 draft,
the coordinator's audit of it, and v2–v4. Only this text is operative. Later accepted changes to the gate (for example
owner sign-off receipts, CK-34; the step watchdog, S4; checkpoint leases and `ck gc`, S12) are recorded in
[the setup hardening record](archive/setup-hardening-plan.md) §3.2 and summarized in [tooling.md](tooling.md).

Review history, in `.pipeline/coordination/gate-bootstrap/`:

- v2 (SHA-256 `f7690970ffc1c4bd4a8c0df6a05d995bcc0f5c03584fe1e4d882c4c4f3310e07`):
  CHANGES-REQUESTED in `spec-review-1.md`, findings F1–F9.
- v3 (SHA-256 `a6878d458de522c8b9c061a439c045b24ac4c803329a9e96f867612e18475e51`):
  CHANGES-REQUESTED in `spec-review-2.md`. It confirmed F2 and F5–F9 resolved
  and raised four blockers (R2-1 to R2-4) and two non-blocking findings.

- v4 (SHA-256 `a896e06bea00dfbd45121aad0e4d16e09157f776c87735962813e82d0be7ba3e`):
  CHANGES-REQUESTED in `spec-review-3.md`. It confirmed R2-1 to R2-6, F1 and
  F3 resolved and raised two blockers in bootstrap failure recovery (R3-1,
  R3-2).

v5 resolves R3-1 (crash prefixes before the hold is installed) by ordering
the freeze writes with `frozen.json` first and adding the `never-held` abort
and an exhaustive recovery table (§11.3 step 6, §11.6, test 28a), and R3-2
(captures after a restore intent) by allowing bootstrap captures only while
the hold is installed and no failure or cancel record exists (§11.4, test
28b).

v4 resolved the round-2 findings as follows:

- F1 (event-aware session parser, §3.2) and R2-1 (fallback outcome): an
  unexpected agent model is added to a resolvable history; review then refuses
  on class mismatch (test 2).
- F3 (conflicts) and R2-4 (blocked entries leaking into capture): live and
  blocked queue entries, by path and dependency closure, in §8.1; capture and
  sync use only the live queue (§8.2, §8.4); resolution groups in §8.5
  (test 23).
- F4 (bootstrap docs drain) and R2-2 (drain recovery): the drain builds a docs
  chain under the hold tombstone and writes the pointer once, with
  `restoring.json`/`restored.json` recovery; `prepare` completes incomplete
  drains (§11.3 step 1, §11.6, test 28).
- R2-3 (docs advancing the pointer after publication): a docs journal and the
  verified *docs descendant* relation (§8.3), accepted by batch recovery
  (§7.4) and bootstrap recovery (§11.7) (tests 22 and 29).
- R2-5 (untestable flush/freeze schedule): test 21 names the two legal
  schedules.
- R2-6 (helper environment order): §11.1 sets a baseline PATH, configures from
  the capture, then checks the capture (test 31).

Author: the Codekarl manager. It runs as Class S in the Claude family, in the
Copilot client as `claude-opus-5.5` at high effort, in session
`383752b5-300f-4a94-b6e7-f2fce6c58ca7`. The v1 draft is change
`QMCFYYGHBMQQCWURTTFSYQVYFBOWTKHHU3AQ6I6JICJNUNXN43IAC`. The coordinator
(`ck-guide`, Codex) audited it in change
`KFJ6BWT7VJVAGGZ5DTGMZMVXO4Q6WRCHOZIZ5GHMZ2ITAMCMURWQC`. Its findings are
resolved here as follows:

- the bootstrap ordering, in §11;
- the class ambiguity, in §2;
- supersession and overlap, in §6.3;
- the metadata and publication races, in §9.

The decisions come from `.pipeline/coordination/tasks/ck-review-gate-design.md`
and `.pipeline/coordination/tasks/ck-review-gate-install.md`. Both are owner
decisions relayed by the coordinator.

The words *must*, *refuse* and *never* are normative. A refusal exits nonzero,
writes nothing and names its reason. All hashes are SHA-256 in lowercase hex.

**Canonical JSON** means `json.dumps(value, sort_keys=True, separators=(",",
":"), ensure_ascii=False).encode()`. **`digest(x)`** is the SHA-256 of `x`'s
canonical JSON, and **`file_digest(p)`** is the SHA-256 of `p`'s bytes.

## 1. Binding decisions

| # | Decision | § |
| --- | --- | --- |
| D1 | Local, on-demand Pijul only. No Git or GitHub, services, daemons, new plugins, external hosting or hidden agents. Work happens in visible ck-* sessions. Classes are S (manager/design), B (implementation) and A (review), all at high effort. | all |
| D2 | Review crosses model families. A reviewer is an independent session that authored none of the reviewed work. Copilot qualifies when its actual model and effort match. Native Claude and Codex sessions are authorized for Codekarl roles (the earlier no-native-Claude restriction expired on the owner's instruction, 2026-10-15). There is no cross-project wake-up. | §2, §6 |
| D3 | A sibling Pijul metadata repository holds issues, versioned specs and review receipts, through `ck issue` and `ck review`. IRC is discussion, not the approval database. | §4 |
| D4 | A spec needs approval before its issue is ready. Spec approval is an independent Class A/high review by the family opposite the spec's authors. There is no separate owner-signoff step. | §4.3 |
| D5 | Status is derived from immutable receipts plus explicit supersession, withdrawal and revocation events. There are no editable approval flags. | §4.4, §6 |
| D6 | Contract and acceptance-criteria edits create a new spec revision and invalidate approvals bound to the old one. Comments, priority, title, type and assignment edits do not. | §4.2 |
| D7 | `ck record` writes versioned, hashed provenance from verified session evidence. Neither the class nor the shared signing key is identity. Model switches are preserved. | §3 |
| D8 | Accepted history is not rewritten. Schema-2 submissions record the closure, files, issues and provenance. | §5 |
| D9 | Reviews bind the exact manifest, capture fingerprint and spec revisions. Missing, stale, withdrawn, revoked or insufficient review fails closed. Coverage is per manifest, never a union of patch approvals; approving A+B does not authorize A. A revision explicitly supersedes its predecessor. | §6 |
| D10 | Mixed-family authorship needs reviews from both families, each by a reviewer independent of every author. This holds for submissions, specs and integration batches, with no either-family shortcut. | §6.2, §7.3 |
| D11 | Unknown or human authorship, including laz, uses the both-family fallback. laz's model is never inferred from its name. There is no owner-authorized exemption. Explicit future owner overrides are possible but are not implemented here. | §2.3, §6.2 |
| D12 | Only eligible submissions are frozen into a batch. A batch gets one full CI run and is then integration-reviewed. Only after that is it published. (Exception, CK-38: a one-member batch whose candidate equals its approved capture skips the separate integration review; see §7.4.) The CI slot is released while review is pending. There is no per-issue full CI. | §7 |
| D13 | Changed membership or code needs new CI and new integration approval. Unaffected individual approvals survive. Split subsets are new candidates. Nothing ever publishes silently. | §7 |
| D14 | Docs are real patches, captured immediately and queued while a batch is active. Code is published first, then docs. A failed, rejected or cancelled batch drains docs onto the unchanged code. Docs never depend on code, conflicting docs are never dropped, and docs never invalidate frozen code CI. Executable Markdown remains code. | §8, §11.6 |
| D15 | Prepare, review and publish are separate durable operations. Locks are short, and the publication check and pointer write are atomic with respect to every writer of their inputs. | §7, §9 |
| D16 | Old local controllers cannot bypass the gate after activation. | §10 |
| D17 | Bootstrap: the gate is published by a narrow, tested helper from its reviewed submission. The helper validates a frozen candidate without publishing it, allows integration review, then re-verifies the base, candidate, membership and exact approval evidence under the publication locks before it publishes atomically. It rejects stale evidence, recovers from interruption, and closes the old-controller bypass at activation. The first install uses frozen coordination receipts, which are preserved. Nobody hand-edits a pointer. | §11 |
| D18 | The general and unattended dispatch hold remains until the gate and the live chat pilot exist. This is not a permanent per-batch owner confirmation. (Met; the setup hold was lifted on 2026-10-17, CK-39.) | §11.8 |

## 2. Roles, classes and families

### 2.1 Policy table

A session's **role** is declared, never inferred from its model. Class S and
Class A currently resolve to the same Claude model, so a model alone cannot
identify a class.

| Role | Class | Effort |
| --- | --- | --- |
| `manager` | S | high |
| `worker` | B | high |
| `reviewer` | A | high |
| `owner` | none (human) | n/a |

The table is a constant in the gate code. Changing it is a code change.

### 2.2 Resolution and verification

A session's role comes from its chat identity's registered role when the
session is bound. Otherwise it comes from an explicit `--role` argument. If a
declared role contradicts a bound identity's role, the command refuses.

The master file is `~/.config/model-classes.toml`, or `CK_MODEL_CLASSES` for
tests. It is parsed with `tomllib` and never edited. It is used as follows:

- **Model normalization.** The client spelling is mapped to the master
  spelling through `[clients.<client>.model_aliases]`, in reverse. For
  example, Copilot's `claude-opus-5.5` becomes `claude-opus-5-5`.
- **Family.** The family is the brand key (`claude` or `codex`) under which the
  normalized model appears in any class, after following `route` entries. Any
  other model has family `unknown`.
- **Class match.** The role's class is resolved for that family, following
  `route`. The session matches only if every observed (model, effort) pair
  equals that resolution exactly. Reviews and the bootstrap receipt
  recheck still use this rule. Managers and workers use the current-pair
  readiness in §2.4.
- **The recorded mapping** has these fields:
  - `class`;
  - `family`;
  - `model` and `effort` as resolved;
  - `master_sha256`, the master file's digest at issuance.

  Later verification uses the recorded mapping and the immutable
  session-evidence digest. It does not re-resolve the current master, so
  re-pointing a class later does not revoke old receipts.

A session that has observed models from more than one family has a
**multi-family** family set. It may author work, but it may never review.

### 2.3 Authors, humans and unknown

- **Agent authors.** `ck record` refuses when an agent author's observed models
  do not match its role's class. This catches mistakes.
- **Humans.** A process with no verified agent ancestor is a human author, with
  family `human`, role `owner` and no class. The owner and laz are both
  `human`. laz's model is never inferred from its name.
- **Unknown authorship.** A patch with missing or unparsable provenance, or
  with an unrecognized model, has family `unknown`.
- **Fallback.** `human`, `unknown` and multi-family authorship all require
  both families under the fallback rule (§6.2).

### 2.4 Model switches inside a session (L1; installed)

**Status.** Installed on 2026-10-15 as CK-3:

- submission `1792090900847282000`, batch `1792091100586830000`;
- pointer `c609d762…`;
- CI receipt `.pipeline/ci/1792091100586830000.json`: 182 tests and Metal;
- canonical reviews: spec `103f2f959d6a4795dbdb`, submission
  `0bd6658225db060f09fa`, integration `165845811acd9d7a78bb`.

The spec is `.pipeline/coordination/tasks/ck-model-switch-spec.md`, sha256
`ee0ebdc4…`. The evidence is in
`.pipeline/coordination/model-switch-repair-result.md`.

**Why.** The whole-history rule answers two different questions at once:

- **Current-role readiness:** is the session running its role's class now?
- **Attributable history:** which models produced its work?

An owner switching a live session to the right model, as
`skills/AGENT-POLICY.md` allows, would otherwise disqualify that session
permanently.

**Contract.**

- **The current pair** is kept separate from contributions. Session parsing
  yields the contribution history unchanged, plus an established current
  (model, effort) pair:
  - Claude: the last contribution.
  - Codex: the last `turn_context`, which must have both model and effort.
  - Copilot: the last contribution, which no later selection event may
    contradict or leave incomplete.

  A selection alone never establishes the current pair.
- **Readiness rules** apply in order, and the first that decides wins:
  1. An unresolved current pair is refused, for every role.
  2. A mixed-family or unknown history keeps exactly the mapping shown above.
     That mapping does not establish the current class. It still requires
     both review families, and it still cannot review.
  3. A reviewer keeps the strict whole-history match, and its current pair
     must also match.
  4. A manager or worker in a single family is ready when its current pair
     equals the role's resolved pair. Any differing earlier pairs are listed
     as `off_class`, as `{model, effort, events}`.
- **Recording.** An entry that is ready only because of rule 4, called a
  reconciled entry, keeps its whole evidence. That means `off_class`, the
  complete ordered `models` list, and `evidence` (path, offset, SHA-256 of
  the prefix read). This applies wherever it is stored: issue events,
  submission manifests, and provenance payloads, whose `mapping` gains
  `off_class`. Strict entries are byte-identical to before. No history entry
  is rewritten, dropped or attributed to the current model.
- **What it does not authorize.**
  - It does not change review requirements, which still come from families.
  - A session with any off-class contribution still cannot review; use a
    fresh reviewer session.
  - A declared role still cannot contradict the bound one.
  - The bootstrap receipt recheck stays on the whole-history rule, so
    historical receipts keep their meaning.
- **Known limitation, L3.** A mixed-family or unknown history is not checked
  against the current class. That is today's behaviour, left for a separate
  follow-up.

**Installation.** The accepted gate refuses the switched manager session as
a spec author. Only that one step, `issue new --role manager`, runs through
a one-time maintenance entry. The entry is reviewed before use, pinned to
the reviewed candidate, and expires when `classes.py` changes. (It ran once,
for CK-3. After publication it refused as expired.) Every other
step uses the normal gate.

## 3. Provenance: `ck record`

### 3.1 Command

`tools/ck record -m MESSAGE --issue CK-N[,CK-M] [--role ROLE] [PATHS...]` is the
only supported way to record code once the gate is active. It does the
following:

1. Resolves the calling session (§3.2) and its role and class (§2). It refuses
   if no evidence resolves or the class does not match.
2. Builds the payload (§3.3), then runs `pijul record -a [PATHS] -m MESSAGE
   --author IDENTITY --description DESC`. `DESC` is the line
   `ck-provenance/1 sha256=<digest(payload)>` followed by the canonical
   payload. The record hook still runs.

The change hash covers the description, so provenance is immutable. After
activation, `tools/ck pijul record` refuses and names `ck record`. A raw
`pijul record` outside `ck` cannot be prevented, and such patches count as
`unknown`.

### 3.2 Session evidence

A session is identified as follows:

| Client | Session ID |
| --- | --- |
| Copilot | `COPILOT_AGENT_SESSION_ID`, with `~/.copilot/session-state/<id>/events.jsonl` existing and an `inuse.<pid>.lock` whose PID is an ancestor of this process |
| Codex | The chat registry entry `codex:<id>` whose `pid`/`pid_start` is an ancestor process. The log is `~/.codex/sessions/**/rollout-*-<id>.jsonl`. |
| Claude Code | A chat registry entry, or a verified client environment variable, with an ancestor PID. The log is `~/.claude/projects/<slug>/<id>.jsonl`. |
| None | No agent ancestor: human (§2.3) |

The roots can be overridden for tests through `CK_SESSION_ROOTS` (JSON) and
`CK_CHAT_REGISTRY`.

**Agent model history.** A session's history is the sequence of (model,
effort) pairs that its *agent* model used, read from session start up to the
read offset. Only the events listed below count. Each event updates or
observes the current agent model M and effort E:

- **Copilot `events.jsonl`:**
  - `session.start`: `data.selectedModel` sets M, and `data.reasoningEffort`
    sets E.
  - `session.model_change`: `data.newModel` sets M. `data.reasoningEffort`,
    when present, sets E; when it is absent, E is inherited.
  - `user.message`: `data.responsesReasoning.model` and `.effort` set M and E
    for that turn, when present.
  - `assistant.message` and `tool.execution_start`: `data.model` is
    *observed*. If it differs from M, the pair (that model, E) is added to the
    history. This records fallbacks and switches that took effect without a
    change event.
  - **Excluded:** every `model.*` event, such as `model.turn_started`,
    `model.model_call_*`, `model.message` or `model.response`. These are the
    client's own utility calls (titles, summaries, routing), for example
    `gpt-4o-mini`, `gpt-5.4-nano` or `gpt-5.6-luna-utility`. They produce no
    assistant message or tool action. An assistant or tool event under such a
    model is not excluded.
- **Codex rollout:** each `turn_context` payload sets M from `model` and E from
  `effort`, or from `reasoning_effort` when `effort` is absent.
- **Claude Code:** assistant entries' `message.model` sets M. A turn without
  determinable effort evidence makes the history **unresolvable**.

**Fail-closed rules.**

- The history is **unresolvable**, and the command refuses, in any of these
  cases:
  - the log is missing;
  - no selecting event precedes the first assistant or tool event;
  - an effort is never established;
  - a relevant event cannot be parsed.
- Every pair in the history counts toward the session's families and class
  match (§2.2). An early switch therefore keeps both families. Under §2.4,
  every pair still counts toward families and stays recorded. Only the
  class-readiness decision for managers and workers uses the current pair.
- The recorded evidence is the log path, the read byte offset, and the
  SHA-256 of the log prefix up to that offset.

**Fixtures.** Tests use sanitized fixtures derived from the permitted
Copilot and Codex logs. They keep only the event types above, include
`model.*` utility events, and include a true agent switch.

The trust boundary is a single macOS account. These checks detect mistakes and
stale state. A deliberately malicious local agent can forge them. Nothing may
claim more.

### 3.3 Payload `ck-provenance/1`

```json
{"schema":"ck-provenance/1","issues":["CK-12"],"identity":"ck-worker-3",
 "client":"codex","session":"…","role":"worker","families":["codex"],
 "mapping":{"class":"B","family":"codex","model":"gpt-6-luna","effort":"high",
            "master_sha256":"…"},
 "models":[{"model":"gpt-6-luna","effort":"high"}],
 "evidence":{"path":"…","offset":12345,"sha256":"…"},"recorded":"…ISO8601…"}
```

Readers ignore unknown fields. New schemas get new strings, and old ones stay
readable. Accepted legacy patches carry no payload.

## 4. Metadata repository and `ck issue`

### 4.1 Location and storage

- The repository is a sibling Pijul repo at `<hub>/../codekarl-meta`. Its path
  is recorded in `<hub>/.pipeline/gate.json`.
- It is created at activation (§11.5) and written only by `ck` under the
  locks of §9. It uses the hub's signing key, and every write is one recorded
  patch.
- Event files are append-only and never edited:
  - `issues/CK-<n>/<seq>-<kind>.json`;
  - `issues/CK-<n>/spec/r<k>.md`;
  - `reviews/<rid>.json` and `reviews/<rid>.md`;
  - `events/<eid>.json` for revocations, withdrawals, supersessions,
    quarantine and bootstrap import.
- `counters.json` is the only rewritten file. It allocates `CK-<n>` and is
  never reused.
- Every metadata read first requires the metadata repo to be clean, meaning no
  unrecorded change. A crash between writing and recording leaves it dirty.
  `ck meta recover` then deletes temporary files and records complete event
  files, under the §9 locks. Every other command refuses on a dirty repo.
- Time Machine covers both repositories. The docs must name both.

### 4.2 Issue model

| Field | Kind | Effect of an edit |
| --- | --- | --- |
| Title, type (`bug`/`feature`/`task`/`design`/`debt`/`docs`), priority (`P0`–`P3`), assignment, comments | descriptive | none |
| Dependencies (CK IDs) | readiness | blocks readiness until each dependency closes; approvals unaffected |
| Spec: description, scope, contract and acceptance criteria (one Markdown file) | contract | new revision `r<k+1>`; approvals bound to `r<k>` stop counting |
| Close or reopen, with a reason | lifecycle | closing requires an accepted submission or the reason `wontfix`/`duplicate` |

A spec revision is identified by `CK-<n>@r<k>` plus `file_digest` of its
Markdown. Its **authors** are the provenance of the session that wrote it.
`ck issue spec` refuses to edit an existing revision.

The commands are as follows:

- `ck issue new --title T --type K [--priority P] --spec FILE`
- `ck issue show CK-N`
- `ck issue list [--ready]`
- `ck issue comment CK-N --file F`
- `ck issue edit CK-N [--title|--type|--priority|--assign|--depends]`
- `ck issue spec CK-N --file F`
- `ck issue close CK-N --reason R`
- `ck issue reopen CK-N --reason R`

### 4.3 Readiness

An issue is **ready** when all of these hold:

- it is open;
- every dependency is closed;
- its latest spec revision has §6.2 coverage from unrevoked approved spec
  receipts, using the revision's authors.

`ck issue list --ready` shows only ready issues. Readiness gates dispatch and
submission (§5.2).

### 4.4 Derived state

Every label is computed by folding events and receipts. Revocation,
withdrawal and supersession are events. Each references its target and carries
actor provenance and a reason.

## 5. Submissions (schema 2)

### 5.1 Command

The command is
`tools/ck submit --issue CK-N[,CK-M] --spec TEXT [--supersedes ID[,ID]]`.

It keeps today's clean-copy capture and fingerprint behavior. The manifest at
`.pipeline/queue/<id>.json` adds these fields:

- `schema: 2`;
- `base`: the accepted pointer digest and its fingerprint hash;
- `hashes`: the full unaccepted closure;
- `files`: the paths changed relative to `base`;
- `issues`: `[{id, revision, spec_sha256}]`;
- `provenance`: per patch, the payload digest or `null`. These are recomputed
  from the patches at every evaluation and never trusted.
- `supersedes`: `[ids]`.

`manifest_sha256` is `file_digest` of the manifest. `fingerprint_sha256` is
`digest(manifest.fingerprint)`.

### 5.2 Rules

- **Issues.** Every closure patch names an issue, through provenance or, for
  `unknown` patches, through `--issue`. Every named issue must be ready at
  submit time.
- **Supersession.** Each ID in `--supersedes` must exist, must be live (§6.3),
  and must share an issue with the new submission. The superseded submission
  becomes permanently non-live. Its manifest and receipts are preserved.
- **Quarantine.** Submit refuses a closure that includes patches of a live
  quarantined submission, unless it supersedes that submission. This keeps
  today's quarantine behavior.
- **Docs.** Submit refuses a code patch that depends on a queued docs patch.
  Queued docs hashes are excluded from the closure (§8.4). Submit also refuses
  a closure patch that touches any `doc_path` file: ordinary docs travel only
  through the docs lane.
- **Withdrawal.** `ck withdraw ID --reason R` makes a submission permanently
  non-live.
- **Stale specs.** A later spec revision makes the submission stale. The fix
  is to resubmit with `--supersedes` and get a new review.

## 6. Reviews and eligibility

### 6.1 `ck review`

```text
ck review submission ID   --verdict approved|changes-requested --report FILE [--incremental-over ID]
ck review spec CK-N@rK    --verdict approved|changes-requested --report FILE
ck review batch BATCH-ID  --verdict approved|rejected --report FILE [--blame ID ...]
ck review revoke RID      --reason TEXT
```

Each command:

1. Resolves the reviewer (§3.2) with role `reviewer`, which is class A at
   high effort (§2). It refuses on a mismatch or a multi-family session.
2. Recomputes the subject digests:
   - for a submission, the manifest bytes and the capture fingerprint;
   - for a spec, the revision bytes;
   - for a batch, the validated record.

   It refuses on any mismatch with the files on disk.
3. Refuses if the reviewer's session or chat identity appears among the
   subject's authors:
   - a submission's authors are its closure provenance;
   - a spec's authors are the revision's authors;
   - a batch's authors are the union over all members.
4. Writes the receipt `ck-review/1` as an event, with these fields:
   - `kind` and `subject`, holding the digests above;
   - `issues`, with the spec revisions and digests;
   - `base_examined`;
   - `reviewer`, the provenance payload;
   - `verdict`;
   - `report_sha256`, with the report stored alongside;
   - `incremental_over`.

Receipts are immutable. `revoke` appends a revocation event, and only an
independent reviewer or the original reviewer may revoke.

### 6.2 Coverage

The **author family set** is the union of the authors' families.
**Independent** means a different session and chat identity from every author.

| Author family set | Required unrevoked approvals |
| --- | --- |
| exactly `{claude}` | ≥1 independent `codex` reviewer |
| exactly `{codex}` | ≥1 independent `claude` reviewer |
| any other set: `{claude,codex}`, or any set containing `unknown`, `human` or a multi-family author | ≥1 independent `codex` **and** ≥1 independent `claude` reviewer, from different sessions |

A receipt **qualifies** only if all of these hold:

- its verdict is `approved`;
- it has not been revoked;
- its subject digests equal the current subject exactly;
- for submissions, every bound spec digest is the issue's current revision.

A receipt for X never counts for Y. Coverage is per manifest digest.

An **incremental receipt** on S, `--incremental-over P`, qualifies for S only
if all of these hold:

- P's hashes are a subset of S's hashes;
- P has its own full coverage;
- P is not withdrawn and not revoked-stale.

P may be superseded *by S*. An incremental receipt never lets a superset
approval authorize a subset.

### 6.3 Liveness, blocking and eligibility

A submission is **live** unless it has been accepted, superseded or withdrawn.
A quarantined submission is still live but is not eligible.

**Blocking:** an unrevoked `changes-requested` receipt on a live submission T
blocks T, and every live submission sharing an unaccepted patch with T.
Integration blame (§7.3) issues such receipts. Receipts on superseded,
withdrawn or accepted submissions never block. Revision therefore works this
way: a successor supersedes T, which unblocks the shared patches. The
successor's own exact-scope review then decides eligibility. An unreviewed
live sibling never blocks; it simply is not eligible.

Submission S is **eligible** when all of these hold:

- its schema is 2;
- it is live and not quarantined;
- its capture is clean and matches its fingerprint;
- every bound issue is open and its bound revision is current and ready;
- it has §6.2 coverage;
- it is not blocked.

Eligibility is recomputed from immutable files at every evaluation. The
binding evaluations, at freeze and publish, happen under the §9 locks.
Manifest-declared provenance is never trusted; it is re-parsed from the
capture with `pijul change`.

## 7. Batch lifecycle

There is one active batch at a time. A batch is **active** from `frozen`
until it reaches one of the terminal states `published`, `failed`, `rejected`
or `cancelled`.

Each state is a separate immutable file in `.pipeline/batches/<bid>/`. Every
one of those files is written while holding `state.lock` (§9):

```text
frozen → validating → validated → approved → publishing → published
   └──────────┴→ failed   ├→ rejected
                          └→ cancelled  (frozen/validating only with ci.lock; validated/approved)
```

### 7.1 `ck batch prepare [--no-split]`

`prepare` acquires `ci.lock` **first**, and holds it from before the freeze
until after its result is recorded. A free `ci.lock` therefore proves that no
prepare is alive. Children that inherit the CI descriptor keep the lock held,
as they do today.

1. **Freeze**, under `state.lock` and `meta.lock`:
   1. Refuse if a batch is active, or if any bootstrap is not terminal.
   2. **Drain barrier:** flush the docs queue (§8.3) while still holding the
      locks. The batch is never frozen on a base that lags queued docs.
   3. Evaluate eligibility.
   4. Write `frozen.json`, which holds:
      - the base pointer digest;
      - the eligible manifest digests and fingerprints;
      - the author union;
      - the owner `{pid, pid_start}`.
   5. Release `state.lock` and `meta.lock`, keeping `ci.lock`.
2. **Validate**, still holding `ci.lock`:
   - write `validating.json` under `state.lock`;
   - build the candidate in `.pipeline/checkpoints/<cid>` from the base, as
     `attempt()` does today, without publishing it;
   - run full CI and write the CI receipt.
3. **Split:** on failure without `--no-split`, use the existing group
   bisection. A passing half becomes a cumulative *unpublished* candidate,
   and the next half is tested on top of it.
   - Membership is the final cumulative passing set.
   - Failing groups are quarantined as events in `.pipeline/quarantine/`.
4. **Record**, under `state.lock`:
   1. Confirm that the batch is still non-terminal. This always holds,
      because every terminal transition from `frozen` or `validating` needs
      `ci.lock`. Then write `validated.json` or `failed.json`.
   2. `validated.json` holds the candidate path, `digest(fingerprint)`, the CI
      receipt digest, the membership manifest digests, the base pointer digest
      and the author union.
   3. A `failed` record immediately drains docs (§8.3) under the same hold.
   4. Release `ci.lock` last.

**Interruption.** Suppose `prepare`, `cancel` or `ck batch status` finds a
batch in `frozen` or `validating` and can acquire `ci.lock` with a nonblocking
attempt. Then the owner and every descendant are gone. Under
`ci.lock` → `state.lock`, it writes `failed.json` with reason `interrupted`
and drains docs. Partial candidates are evidence and are never reused.

### 7.2 Waiting

While a batch waits for review, no lock is held. Manual, fast and quick checks hold no project lock, so they can run at any time, including while a batch validates. New submissions wait for the next batch, and docs queue (§8).

### 7.3 Integration review

**Amended by CK-38 (2026-10-17):** a one-member batch whose candidate equals its approved capture publishes without a batch
review receipt, on the conditions in [tooling.md](tooling.md) ("Single-submission batches"); every other batch keeps the
rules below.

`ck review batch` binds `digest(validated.json)`, the candidate fingerprint
digest, the CI receipt digest and the membership manifest digests.

- **Coverage** is §6.2 over the author union. Mixed membership needs both
  families, each from a reviewer independent of every member author.
- **Approved**, with full coverage: write `approved.json` under `state.lock`
  and `meta.lock`, naming the receipts.
- **Rejected:**
  - write `rejected.json`;
  - each `--blame` member gets a `changes-requested` receipt, which blocks it;
  - members that are not blamed keep their approvals and return to the queue;
  - docs drain under the same hold.

### 7.4 `ck batch publish BID`

The command holds `state.lock` and `meta.lock` continuously, from the first
check through the docs flush. Every one of these must hold:

- the batch is `approved`, **or** it qualifies for the **single-submission basis** (CK-38): exactly one member; the member's frozen
  capture fingerprint equals the validated candidate fingerprint; the member's base and patches equal the batch base and the
  candidate's added patches; fresh AI submission coverage (an owner submission receipt alone does not qualify); no `rejected`,
  `cancelled` or `failed` record and no non-approving batch receipt; and the CI log, whose digest `ci_log_sha256` was bound at
  prepare, has exactly one full-level recipe header, one finite timing per listed step and a final full PASS, with the slowest step
  at most a tenth of the header's watchdog. These are evaluated here, under the locks;
- the current pointer's digest equals the batch base digest;
- the candidate is clean and its fingerprint digest is unchanged;
- the CI receipt digest is unchanged and the receipt passed;
- every member is still eligible, re-evaluated now;
- the integration coverage still holds: receipts are unrevoked and bind this
  validated record. On the single-submission basis, the member's submission coverage takes its place.

The steps are:

1. Write `publishing.json`, which holds the exact pointer bytes to write.
2. Atomically replace `accepted-v2.json`. The new pointer records the CI
   receipt and an `integration` field naming the approving receipts. On the single-submission basis it records
   `basis: single-submission`, the submission ID, the submission receipts, an empty batch-receipt list and the timing evidence
   (log digest, slowest step and margin); no batch receipt is invented. `published.json` records the same basis.
3. Write `published.json`.
4. Flush docs (§8.3).
5. Release the locks.

**Recovery**, under the same locks:

| State found | Action |
| --- | --- |
| `publishing.json` exists and the pointer equals the base | re-verify everything above, then continue |
| `publishing.json` exists and the pointer equals the `publishing.json` bytes, or is a docs descendant of them (§8.3) | verify that the candidate fingerprint and CI receipt digests still equal `validated.json`, then write `published.json` if absent and resume the flush |
| anything else | refuse, as an incident |

The basis is fixed when `publishing.json` is written, because it is part of those pointer bytes, and recovery never switches it. In the
first row a single-submission recovery re-verifies the single-submission conditions. In the second row (pointer written) the
member is already accepted, so eligibility is not re-evaluated; the other integrity checks remain, and on the single-submission basis
the member's submission coverage is checked in place of batch coverage.

A rerun after a complete publication finds `published.json` and a docs
descendant of the published pointer, and only resumes the flush.

A changed base or membership requires `cancel` and a new `prepare`.

### 7.5 `ck batch cancel BID --reason R`

- **From `validated` or `approved`:** under `state.lock`, write
  `cancelled.json`, return the members to the queue with their approvals
  intact, and drain docs.
- **From `frozen` or `validating`:** cancel first attempts `ci.lock` without
  blocking.
  - If the lock is held, cancel refuses: "validation in progress (pid …); wait
    or interrupt it".
  - If it acquires the lock, it writes `cancelled.json` under `state.lock`
    and drains docs.

### 7.6 Removed command

`ck accept` refuses, naming `ck batch prepare`, `ck review batch` and
`ck batch publish`.

## 8. Docs lane

### 8.1 Queue states, live and blocked entries

Each queued docs entry is `pending`, `conflict`, `published` or
`resolved-by <group>`. A `pending` entry is **blocked** if any of these holds:

- it touches a path of an entry in `conflict`;
- its dependency closure (from `pijul change` on its hashes, transitively)
  contains a hash of an entry in `conflict`, or of another blocked entry.

Every other `pending` entry is **live**. Blocking is recomputed from the queue
whenever it is read; it is never stored. Blocked and `conflict` entries keep
their state and their patches as evidence. The **live queue** is the live
entries in FIFO order. Its hashes never depend on a conflicted or blocked
hash, so pulling it with `--complete` never brings one back.

### 8.2 Capture

`ck docs -m MSG PATHS` keeps today's path rules and its local reconciliation
behavior. Its freshness rule is extended: the caller's copy must contain the
accepted hashes, plus every live queued hash that touches any of `PATHS`.
`ck sync` provides this (§8.4).

Under `state.lock`, capture does the following:

1. It refuses if any of `PATHS` is a path of a `conflict` or blocked entry.
   The message names the entry and `ck docs resolve`.
2. It records the docs patch in a capture repo, `.pipeline/docs/<did>/`. The
   capture is cloned from the accepted checkpoint plus the live queue, in FIFO
   order; blocked and `conflict` entries are never pulled. Its manifest
   records the hashes, paths, author and fingerprint.
3. It enqueues the entry as `pending`.
4. If no batch or bootstrap is active, it flushes (§8.3) under the same hold.
   Otherwise it prints `queued DOCS-ID`.

### 8.3 Flush and the docs journal

Flush always runs while `state.lock` is held. It runs:

- at the freeze drain barrier (§7.1);
- after every terminal batch transition (§7.1, §7.3, §7.4, §7.5);
- from `ck docs` capture;
- on demand, through `ck docs flush`.

Before **every** docs pointer write, flush rechecks under the lock that no
batch or bootstrap is active. If one is active, it stops and leaves the rest
queued. `ck docs flush` during CI or review is therefore a no-op that reports
`deferred behind batch B`.

It takes the live queue in FIFO order. For each entry it:

1. clones the current checkpoint and pulls the entry's hashes;
2. requires all of these:
   - no conflicts;
   - every dependency present in the checkpoint or in this entry;
   - non-doc file digests unchanged;
3. builds one `kind: docs` checkpoint, whose receipt inherits the current
   receipt;
4. writes the journal record `.pipeline/docs/journal/<seq>.json`:
   `{seq, entries: [DID…], prev_sha256: file_digest(current pointer),
   next: <exact new pointer bytes>}`;
5. atomically replaces the pointer with `next`;
6. marks the entry `published`.

An entry whose hashes are already in the current checkpoint is marked
`published` without a new checkpoint. A journal record whose pointer write
never happened (the pointer still equals its `prev`) is left as evidence and
superseded by the next record. Every step is therefore idempotent, and a crash
resumes at the next flush. An entry that conflicts is marked `conflict`, with
the conflicting state kept as evidence; entries it blocks wait (§8.1).

**Docs descendants.** A pointer X is a *docs descendant* of a pointer P when
X equals P, or when walking back from X reaches P: find the journal record
whose `next` equals X, then continue from the pointer whose digest is that
record's `prev`, and so on. Each step is verified: every checkpoint named
along the chain is clean and matches its fingerprint, and it differs from its
predecessor only in `doc_path` files. Recovery rules that accept "P or a docs
descendant of P" (§7.4, §11.7) rely on this walk, so legitimate docs
publications after a code publication are never treated as incidents.

### 8.4 Working copies and code

- `ck sync` brings a clean copy to the accepted state plus the live queue. It
  accepts copies whose hashes are a subset of accepted plus every queued docs
  hash, including blocked ones, and leaves blocked hashes a copy already has
  in place.
- `submit` excludes queued docs hashes from the closure. It refuses any code
  patch that depends on a queued docs patch.
- `submit` refuses any closure patch that changes a `doc_path` file. Ordinary
  docs travel only through the docs lane, so code never carries docs and code
  CI never depends on docs.
- Queued docs never enter a frozen candidate.
- `doc_path` is unchanged: `README.md` or `docs/**/*.md`, excluding
  `AGENTS.md`, `CLAUDE.md` and `SKILL.md`.

### 8.5 Conflicts and resolution

A conflict should be rare: capture serializes entries on the live queue head,
and code cannot touch docs. Conflicts are still handled fully, and nothing is
ever dropped or rewritten.

- **Blocking** follows §8.1. Entries on disjoint paths that do not depend on a
  conflicted hash continue to capture, sync and flush.
- **`ck docs resolve DID`** creates `.pipeline/docs/<rid>-resolve/`. That copy
  is cloned from the current checkpoint and pulls the conflicted entry's
  hashes, so it contains Pijul's conflict markers. The author edits it, then
  runs `ck docs resolve DID --record -m MSG`. That records a resolution patch
  R and replaces the conflicted entry's queue position with a **group entry**
  `{resolves: DID, hashes: DID.hashes ∪ {R}}`. The conflicted entry stays on
  disk, marked `resolved-by <group>`.
- **Group flush.** A group is pulled and published as one docs checkpoint and
  one journal record. Its dependencies may lie within the group itself.
- **Formerly blocked entries** become live again and flush in their original
  order. Each is checked again; if one now conflicts, it becomes the next
  conflict.

## 9. Locks, snapshots and recovery

| Lock | File | Acquisition | Held by |
| --- | --- | --- | --- |
| CI | `.pipeline/ci.lock` | nonblocking; "Already running" (unchanged); `CK_CI_LOCK_FD` inheritance kept | `batch prepare`, from before freeze to after its record; cancel or recovery of a `frozen`/`validating` batch; bootstrap `prepare`, `publish`, `cancel` and docs drain. Batch validation is the only `check` run that holds it, through the inherited descriptor; manual, fast and quick `check` runs hold no project lock |
| State | `.pipeline/state.lock` | blocking, with a 60 s timeout and a message naming the holder | every write of batch, bootstrap, pointer, docs-queue or eligibility state; freeze; publish; flush |
| Meta | `<meta>/.lock` | blocking, 60 s | every metadata write; freeze; publish; integration verdicts |

- **Total order:** `ci.lock` → `state.lock` → `meta.lock`. No operation
  acquires an earlier lock while holding a later one. `state.lock` is never
  held while waiting for CI, because CI runs only after the freeze has
  released `state.lock`.
- **Inputs to eligibility** are written while holding both `state.lock` and
  `meta.lock`, and are recorded before release. That covers spec revisions,
  receipts, revocations, withdrawals, supersessions, manifests, issue
  close/reopen and quarantine.
- **Freeze and publish** evaluate and write while holding both locks
  continuously. No input can change between their check and their write.
- **Writes that need `state.lock`** must not be made under `ci.lock` alone.
  That covers every batch and bootstrap state file and every docs-queue
  transition.
- **`status` and `issue list`** read without locks and may be momentarily
  stale. They are display only.
- **Recovery.** Each lifecycle step writes its outcome file before the next
  step begins. A rerun finds the last file written and resumes or refuses,
  naming the step.

## 10. Controller trampoline and old-controller cutover

### 10.1 Pointer files

| File | Meaning |
| --- | --- |
| `.pipeline/accepted-v2.json` | the real pointer after activation (`schema: 2`) |
| `.pipeline/accepted.json` before the bootstrap | the legacy pointer |
| `.pipeline/accepted.json` during a bootstrap window | the **hold tombstone** (§11.3) |
| `.pipeline/accepted.json` after activation | the **final tombstone** |

Both tombstones have the following content:

- They keep the legacy `submissions`, `receipt` and `previous` fields for
  audit.
- `schema` is `"bootstrap-hold"` or `"tombstone"`. The hold tombstone also
  names `bootstrap: BID`. The final tombstone has
  `upgrade: "accepted-v2.json"`.
- `repo` is the absolute nonexistent path
  `<hub>/.pipeline/UPGRADE-REQUIRED-see-docs-review-gate-plan`, and
  `fingerprint` is `{"hashes": [], "files": {}}`.

Every legacy command that reads state calls `verified_state()`, which calls
`clean(repo)`. That fails before any write, because `pijul` cannot start in a
missing directory. This covers `submit`, `accept`, `docs`, `worker`, `sync`
and `status`.

Two legacy commands behave differently:

- Legacy `init` refuses because a pointer exists.
- Legacy `retry` deletes only legacy `.pipeline/blocked/` markers. The gate
  never reads those for eligibility; its quarantine is in
  `.pipeline/quarantine/`.

One shared module, `tools/gate/pointer.py`, reads and writes pointers. All
readers use it, including `tools/chat/install.py`.

### 10.2 Trampoline

The new `tools/ck` imports no local project module. It uses a small inline
standard-library reader to resolve the hub (the `.ck-hub` marker) and then the
pointer:

1. If `accepted-v2.json` exists, use it.
2. Otherwise, use a legacy `accepted.json`.
3. A hold tombstone refuses: "bootstrap BID in progress; state commands are
   paused". A final tombstone without v2 refuses: "activation incomplete;
   rerun the bootstrap publish recovery (§11.7)".

It then verifies the checkpoint, using the same rules as `verified_state()`:
the checkpoint is clean and matches its fingerprint. Then:

- **Locally**, from the copy, it runs `check`, `cargo` and `pijul`, as today.
  After activation, `pijul record` refuses and names `ck record`.
- **Everything else** goes through
  `os.execv(sys.executable, [sys.executable, "-B", "-c", LAUNCHER,
  <checkpoint>/tools, <caller root>, *args])`. That is a fresh interpreter, so
  no local module is ever loaded.
  - The fixed `LAUNCHER` puts the checkpoint's `tools` directory first on
    `sys.path`. It imports `environment` and `pipeline`, asserts that each
    module's `__file__` lies under that directory, and runs the same sequence
    as the legacy entry point: `configure(root)`, `signing(hub)`, then
    `main(root, hub)`, with the legacy error handling.
  - The legacy `main(root, hub)` takes the caller root, so this also works
    against the pre-gate controller.

Consequences:

- A worker's modified `pipeline.py` or `environment.py` never runs for state
  commands.
- Controller changes take effect at publication.
- CI still tests the candidate's own code.

### 10.3 Old copies after activation

Old copies fail closed for state commands. They may still run
`tools/ck pijul pull <checkpoint named by accepted-v2.json>`, which reads no
state, and afterwards they have the trampoline. Copies holding unaccepted work
use the existing manual integration path. The preferred route is a fresh
`ck worker`.

## 11. Bootstrap of this gate

### 11.1 Shape and environment

The gate is implemented in one legacy (schema-1) submission, made with the
legacy `tools/ck submit`. That submission contains a helper,
`tools/gate/bootstrap.py`, which is reviewed with it.

The manager runs the helper from that submission's immutable capture:

```text
python3 -B <hub>/.pipeline/submissions/<ID>/tools/gate/bootstrap.py --hub <hub> <command>
```

`-B` means no bytecode is written into the capture. The helper's code, its
tests (run against the vendored legacy controller) and its execution all come
from the reviewed bytes.

The helper may be started from an ordinary shell. It sets up its own
environment from reviewed code, in this order:

1. Using only the standard library, it refuses unless its own file lies at
   `<hub>/.pipeline/submissions/<ID>/tools/gate/bootstrap.py`, where the
   capture's `.ck-hub` names `--hub` and `ID` is the submission given to
   `prepare` or recorded in the bid.
2. If `PATH` is unset or empty, it sets the documented baseline
   `/usr/bin:/bin:/usr/sbin:/sbin`. If `HOME` is unset, it refuses.
3. It imports the capture's own `environment` module, asserting that the
   module's `__file__` lies inside the capture, and calls
   `configure(<capture>)` (pinned PATH, Xcode selection, Pijul config).
4. Only now, with the configured tools, it refuses unless the capture is clean
   and matches its manifest fingerprint.
5. It runs every command inside `signing(<hub>)`.

It refuses every command once `accepted-v2.json` exists, except the
post-publication recovery and tail of §11.7.

### 11.2 Records and receipts

Bootstrap state lives in `.pipeline/bootstrap/<bid>/`. These files are each
written atomically under `state.lock` and never rewritten:

- `frozen.json`
- `legacy-pointer.json`: the exact saved legacy pointer bytes
- `hold.json`: the exact hold-tombstone bytes
- `validated.json`
- `failed.json`
- `cancelled.json`
- `restoring.json` and `restored.json` (§11.6)
- `publishing.json`
- `published.json`
- `evidence/`

If a file already exists with different bytes, the helper refuses. A
bootstrap is **active** from `frozen.json` until `published.json` or
`restored.json` exists. While one is active, §8.3 flush defers.

Frozen coordination receipts live in `.pipeline/coordination/gate-bootstrap/`.
The metadata system does not exist yet, so they are the review evidence for
this install. They are preserved, copied into `evidence/`, and imported into
the metadata repo as a bootstrap event (§11.5).

**Review receipt `ck-bootstrap-review/1`.** Each reviewer writes one, as
canonical JSON:

```json
{"schema":"ck-bootstrap-review/1","kind":"spec|submission|integration",
 "verdict":"approved","subject":{…},
 "reviewer":{"identity":"…","client":"…","session":"…","role":"reviewer",
             "class":"A","family":"…","model":"…","effort":"high"},
 "report":{"path":"…","sha256":"…"},"recorded":"…"}
```

The `subject` depends on `kind`:

| Kind | Subject |
| --- | --- |
| `spec` | `{"path":"docs/review-gate-plan.md","sha256":…}` |
| `submission` | `{"submission":ID,"manifest_sha256":…,"fingerprint_sha256":…}` |
| `integration` | `{"bootstrap":BID,"validated_sha256":…,"candidate_fingerprint_sha256":…,"ci_receipt_sha256":…,"membership_manifest_sha256":[…]}` |

**Authors ledger `ck-bootstrap-authors/1`.** The manager writes it from the
task ledger. It holds:

- `spec_authors` and `submission_authors`, each a list of
  `{identity, client, session, role, family, model, effort}`;
- `patches: {hash: [author index…]}`, which must map every submission patch.

**Evidence index `ck-bootstrap-evidence/1`.** It holds:

```text
{bootstrap, validated_sha256, spec: {path, sha256}, authors: {path, sha256},
 receipts: [{path, sha256}]}
```

All paths are relative to `.pipeline/coordination/gate-bootstrap/`.

### 11.3 `prepare --submission ID --manifest-sha256 HEX`

`prepare` acquires `ci.lock`, and holds it until its result is recorded. That
excludes legacy `accept`, `docs` and `retry`, and any second prepare. It then
acquires `state.lock` for each record write.

1. **Earlier bids.** For every earlier bid:
   - A bid in `frozen` with no result has lost its owner, because this
     `prepare` holds `ci.lock`. The helper marks it `failed` with reason
     `interrupted`.
   - A bid with `failed` or `cancelled` but no `restored.json` has an
     incomplete restore and drain. That includes the bid just marked.
   - The helper completes §11.6 for each such bid before continuing.
   - If a bid is `validated` and awaiting review, `prepare` refuses; one
     bootstrap runs at a time.
2. **Preconditions.** `prepare` refuses if any of these holds:
   - `accepted-v2.json` exists;
   - `accepted.json` is a tombstone;
   - the legacy `verified_state()` checks fail.
3. **Quiescence.** The process scan finds a process other than the helper and
   its ancestors whose command line invokes `tools/ck` with a state subcommand
   (`submit`, `accept`, `docs`, `worker`, `sync`, `status`, `retry` or
   `init`), or runs `tools/pipeline.py` for this hub. If it finds one, the
   helper refuses and names it. This covers legacy submitters that hold no
   lock. In addition, the procedure requires all submitting sessions to be
   held (§11.8).
4. **Submission.** All of these must hold:
   - `file_digest(queue/ID.json)` equals `--manifest-sha256`;
   - the submission is not accepted and not legacy-blocked;
   - the capture is clean and matches its fingerprint;
   - its hashes equal the capture hashes minus the base hashes.
5. **Other pending legacy submissions.** Each must have hashes that are a
   subset of this submission's hashes. They are fix-cycle predecessors and
   are recorded as `superseded_legacy`. Anything else refuses.
6. **Freeze and hold**, under `state.lock`. Each write is separately atomic,
   so a crash can leave any prefix of this sequence; §11.6 recovers every
   prefix. The bid directory is created by the first write, so every bid has
   `frozen.json`.
   1. Write `frozen.json`, which holds:
      - `base_pointer_sha256`, the `file_digest` of the current `accepted.json`;
      - the queue snapshot: every manifest file name and its digest;
      - the manifest and fingerprint digests;
      - `superseded_legacy`;
      - the owner `{pid, pid_start}`.
   2. Write `legacy-pointer.json`, the exact current `accepted.json` bytes.
   3. Write `hold.json`.
   4. Atomically replace `accepted.json` with the hold tombstone (§10.1). From
      this point until publication or restoration, every legacy state command
      and the new trampoline fail closed. That includes a submit that read
      state earlier: its later writes are detected in step 3 of §11.5.
7. **Validate.** Build the candidate in `.pipeline/checkpoints/<cid>` from the
   saved base, as `attempt()` does, and require all of these:
   - no conflicts;
   - hashes equal the base hashes plus the submission hashes;
   - the candidate is clean.

   Then run the candidate's own full `tools/check.py` with `CK_CI_LOCK_FD`,
   and require an unchanged fingerprint afterwards. Write the CI receipt
   `<cid>.json`, with `kind: ci`, `passed`, `fingerprint`, `submissions: [ID]`,
   `bootstrap: BID` and `log`.
8. **Record**, under `state.lock`:
   - On success, write `validated.json`. It holds the base digest, the
     manifest and fingerprint digests, `superseded_legacy`, the candidate path
     and fingerprint digest, the CI receipt digest and `helper_sha256`.
   - On failure, write `failed.json`, then restore and drain (§11.6) before
     releasing `ci.lock`.

Nothing is published. `status BID` prints the subject digests that reviewers
must bind.

### 11.4 Docs during the bootstrap window

While a hold tombstone is present, legacy `tools/ck docs` fails closed.

`docs --from COPY -m MSG PATHS` is allowed only while, under `state.lock`,
`accepted.json` equals the active bid's `hold.json` and that bid has no
`failed` or `cancelled` record. Otherwise it refuses and names the next step:
`cancel BID` to complete a pending restore, or the legacy `tools/ck docs` once
none is pending. It records a real docs-only patch, using the
§8.2 capture rules, against the saved legacy base plus the live docs queue
(§8.1). It takes only `state.lock`, enqueues the patch in `.pipeline/docs/`,
the same queue that the new controller flushes after publication, and never
publishes. Outside
a window, it refuses and names the legacy `tools/ck docs`.

Because `failed.json` and `cancelled.json` are written under `state.lock`
before any restore work starts, every capture either precedes that record
and is drained by the restore, or follows it and is refused. No capture can
be stranded behind a restore intent.

### 11.5 `publish BID --evidence FILE --evidence-sha256 HEX`

`publish` holds `ci.lock`, then `state.lock`, continuously from the first
check to the `published.json` write. It verifies, in this order:

1. `validated.json` exists, and no `failed` or `cancelled` record exists.
2. `accepted.json` equals `hold.json` exactly. The digest of
   `legacy-pointer.json` equals the base digest, and the base checkpoint passes
   the legacy `verified_state()` rules.
3. The process scan (§11.3, step 3) is clean. The queue equals the frozen
   snapshot exactly, so there is no late or changed manifest. The manifest
   bytes and the capture fingerprint equal their frozen values.
4. The candidate is clean. Its fingerprint digest equals the validated value,
   and its hashes equal the base hashes plus the submission hashes. The CI
   receipt bytes are unchanged and the receipt passed.
5. The helper's file digest equals `helper_sha256` and equals the candidate's
   `tools/gate/bootstrap.py`.
6. The evidence index digest equals `--evidence-sha256`, and every listed file
   exists with a matching digest.
7. The spec receipt's `sha256` equals the candidate's
   `docs/review-gate-plan.md`.
8. Every receipt's subject equals the current digests exactly.
9. Every verdict is `approved`.
10. Every reviewer satisfies all of these:
    - its role is `reviewer`;
    - its class mapping is A at high effort for its family, under the current
      master;
    - its session log exists, and its agent-model history (§3.2) is resolvable
      and equals that mapping;
    - it is single-family;
    - it is independent of every author named in the ledger for its subject.
11. Coverage (§6.2) is met for the spec authors, the submission authors and the
    integration batch, which uses the submission authors.

Then it performs these steps:

1. Copy the evidence into `evidence/`.
2. Write `publishing.json`, which holds the exact final-tombstone bytes and v2
   pointer bytes.
3. Replace `accepted.json` with the final tombstone.
4. Write `accepted-v2.json`, which holds:
   - `schema: 2`;
   - the candidate's `repo` and `fingerprint`;
   - `submissions`: the legacy accepted list plus ID;
   - `receipt`: the CI receipt;
   - `previous`: the base receipt;
   - `integration: {bootstrap, evidence_sha256}`;
   - `superseded_legacy`.
5. Write `published.json`.

There is no window in which any controller can publish: from step 6 of §11.3
onward, legacy controllers are tombstoned, and the new controller has no
pointer until step 4.

A legacy write that completes after publication is inert. A schema-1 manifest
is never eligible, and `ck status` shows it as `legacy-late`. It is preserved
as evidence.

**Activation tail.** This runs after `ci.lock` is released. Every step is
idempotent:

1. Under `meta.lock`, create the metadata repo and `gate.json` if absent.
2. Record a `bootstrap` event that names the bootstrap ID and every evidence
   digest.
3. Sync the hub root to the new checkpoint, using the new code. The root must
   be clean and a subset of accepted plus queued docs. Otherwise the remaining
   steps are named and the tail stops.
4. Under `state.lock`, flush the docs queue onto the v2 pointer (§8.3). Each
   publication is journaled, so the pointer becomes a docs descendant of the
   published v2 bytes.

### 11.6 Failure, cancel, restore and drain

**`cancel BID --reason R`** is refused once `publishing.json` exists. It first
attempts `ci.lock` without blocking. If the lock is held, a prepare is still
validating, and cancel refuses. Otherwise, under `state.lock`, it writes
`cancelled.json` and then restores and drains.

**Restore and drain** runs for failure, interruption and cancellation, always
after the bid's `failed.json` or `cancelled.json` exists. It holds `ci.lock`
and then `state.lock` throughout.

If the hold was never installed, it **aborts without a hold**: it writes
`restored.json` with `{mode: "never-held", pointer_sha256: <observed>}` and
does not touch `accepted.json`, whose content may legitimately have advanced
through legacy commands since the freeze. No docs can be queued for such a
bid, because §11.4 capture requires the installed hold.

Otherwise the hold tombstone stays in place until a single final pointer
write, so no legacy controller can act on an intermediate state and the saved
base is never overwritten by a stale restore:

1. **Build.** The helper verifies that `file_digest(legacy-pointer.json)`
   equals `frozen.json`'s `base_pointer_sha256` and that the base checkpoint
   passes the legacy `verified_state()` rules. Starting from that base, it
   builds a chain of docs checkpoints from the live docs queue, in FIFO order,
   using the §8.3 per-entry checks. Each link is a legacy-format `kind: docs`
   checkpoint and receipt, the same shape that legacy `tools/ck docs` writes,
   inheriting the previous link's receipt. An entry that conflicts is marked
   `conflict` and skipped; the chain continues from the previous link.
   Nothing is published yet.
2. **Intent.** Write `restoring.json`, which holds the exact final legacy
   pointer bytes (the last link, or the saved base bytes when no docs were
   queued), the entries the chain contains, and any entries left in
   `conflict` or blocked.
3. **Write.** Atomically replace `accepted.json`, which still equals
   `hold.json`, with the `restoring.json` bytes.
4. **Acknowledge.** Mark each chained entry `published`, then write
   `restored.json` with `mode: "restored"`. The bootstrap is no longer
   active.

Entries left in `conflict` or blocked stay queued as evidence, and
`restored.json` lists them. They cannot conflict in practice, because each was
captured on the same base plus the live queue in order. If one does, the
manager reports it and its author redoes the edit with the legacy
`tools/ck docs`.

Rerunning `cancel`, or the next `prepare` (§11.3 step 1), resumes. It first
marks a `frozen` bid without a result `failed` (`interrupted`), and then
applies the first matching row:

| State found | Action |
| --- | --- |
| `restored.json` exists | nothing to do |
| `restoring.json` exists and `accepted.json` equals its bytes | step 4 |
| `restoring.json` exists and `accepted.json` equals `hold.json` | verify that every checkpoint in the recorded chain is clean and matches, and that the live queue holds no entry missing from `restoring.json`; then step 3 |
| no `restoring.json`, `hold.json` exists and `accepted.json` equals it | step 1; a partial chain from an interrupted run is left as evidence and never reused |
| no `restoring.json`, and `accepted.json` is not a tombstone (crash after `frozen.json`, `legacy-pointer.json` or `hold.json`, before the hold pointer write) | abort without a hold |
| any other combination | refuse, and report an incident naming every file digest |

The hold pointer can only be replaced by step 3 or by publication, and both
write their intent record first, so these rows are exhaustive.

A `failed` bootstrap restores and drains automatically and never needs a
cancel.

### 11.7 Publication recovery

Rerunning `publish` handles these states:

| State found | Action |
| --- | --- |
| `publishing.json` exists; `accepted.json` equals `hold.json`; v2 is absent | re-verify every §11.5 check and continue from step 3 |
| `accepted.json` equals the intended final tombstone; v2 is absent or equals the intended bytes | see below |
| `published.json` exists | require that `accepted.json` equals the final tombstone and that v2 equals the intended bytes or is a docs descendant of them (§8.3), then resume the tail only |
| any other combination | refuse, and report an incident naming every file digest |

In the second case, the helper holds `ci.lock` and `state.lock` and verifies
all of these before writing anything:

- the intended bytes recomputed from `validated.json`, `legacy-pointer.json`
  and the evidence index equal the bytes in `publishing.json`;
- the candidate is clean, and its fingerprint digest and the CI receipt digest
  equal `validated.json`;
- every copied evidence file matches the index, and the index digest matches;
- the base checkpoint still passes.

Only then does it write v2 if absent, write `published.json`, and run the
tail. A docs descendant of the intended v2 bytes can exist only after
`published.json`, because the tail is the only docs writer after the
tombstone. Changed candidate or evidence bytes are an incident: nothing is written,
and every controller stays fail-closed.

### 11.8 Procedure for this install

| Step | What happens |
| --- | --- |
| 1 | **Spec review.** An independent Codex Class A/high review of the exact digest; the spec's author family is Claude. Findings are resolved, the docs are republished, and the review repeats on each new digest until approved. |
| 2 | **Implementation.** The Codex Class B/high worker implements the contract in a fresh worker copy, as focused patches in one legacy submission (§12), with the §13 tests. It records with `tools/ck pijul record` and submits with the legacy `tools/ck submit`. The authors ledger comes from the task ledger. |
| 3 | **Submission review.** The Copilot Claude-family Class A/high reviewer reviews the exact manifest. Each fix cycle produces a new legacy submission that contains its predecessor (§11.3, step 5) and is reviewed in full or incrementally over its reviewed predecessor. |
| 4 | **Hold.** The manager confirms that every submitting session is idle with no pending instruction. Today that is only the worker. The manager records this in the progress file. Then `prepare`, which hold-tombstones legacy state and validates the candidate. Docs written during the window use §11.4. |
| 5 | **Integration review.** The Copilot reviewer reviews the validated record. The only author family is Codex, so a Claude review gives full coverage. A rejection means `cancel` (restore and drain), a fix cycle, and a new `prepare`. |
| 6 | **Publish.** The manager writes the authors ledger and evidence index, then runs `publish`. |
| 7 | **Verify.** Check the receipts, `accepted-v2.json`, the tombstone, and `tools/ck status` through the trampoline. Check that a legacy worker copy's `status`, `submit`, `worker`, `sync` and `docs` all fail closed. Check that the docs queue drained. Install the ck-* skills from the synced accepted root. |

After activation, the existing workers are inert legacy copies. Coordination
files remain the historical ledger. General and unattended dispatch remain
held until the live chat pilot exists (D18). (Both conditions are now met: the pilot ran on 2026-10-15, and the owner's
setup goal completed on 2026-10-17. The setup hold was lifted that day (CK-39): the manager dispatches owner-authorized work.)

## 12. Implementation layout

- **Standard library only** (Python ≥3.11 `tomllib`). `ruff` and every
  existing check pass. Modules stay small:

  | Module | Contents |
  | --- | --- |
  | `tools/ck` | trampoline (§10.2) |
  | `tools/pipeline.py` | CLI `main` and the core `Pipeline` operations, kept compatible with today's structure |
  | `tools/gate/pointer.py` | legacy/v2/tombstone read and write, shared by every reader |
  | `tools/gate/locks.py` | `state.lock` and `meta.lock` acquisition and order |
  | `tools/gate/classes.py` | the master parser and the role/class/family resolution (§2) |
  | `tools/gate/sessions.py` | session evidence (§3.2) |
  | `tools/gate/provenance.py` | `ck record` and payload parsing |
  | `tools/gate/meta.py` | metadata repo, events, issues and recovery |
  | `tools/gate/reviews.py` | receipts, coverage, liveness, blocking, eligibility |
  | `tools/gate/batch.py` | batch lifecycle (§7) |
  | `tools/gate/docsqueue.py` | docs lane (§8) |
  | `tools/gate/bootstrap.py` | helper (§11) |
  | `tools/gate/procscan.py` | the process scan (§11.3, step 3), with an injectable process lister for tests |

- **Executable Markdown updated in the same submission:**
  - `skills/AGENT-POLICY.md`, `skills/ck-work/SKILL.md`,
    `skills/ck-review/SKILL.md`, `skills/ck-manager/SKILL.md` and `AGENTS.md`,
    to use `ck record`, `ck submit --issue`, `ck review` and `ck batch`;
  - `tools/chat/install.py`, to read through the pointer module.
- **Ordinary docs** go through the docs queue (§11.4). That covers
  `docs/tooling.md` (the acceptance transaction and failed batches) and this
  plan's status line.

## 13. Acceptance criteria

All criteria use real Pijul repositories, as `tests/test_pipeline.py` does,
with injected validators, session roots, process listers and model-class
masters. Races are made deterministic with subclass hooks that pause an
operation at a named point, never with sleeps.

The 19 existing specs keep their meaning. Specs that assert `accept`
publishes move onto `prepare`/`review`/`publish` without weaker assertions.

The legacy controller is vendored as `tests/fixtures/legacy/`, byte-identical
to the accepted pre-gate `tools/ck`, `tools/pipeline.py` and
`tools/environment.py`. A test pins their digests.

**Roles and session evidence**

1. Copilot's alias normalizes. Claude Opus at high effort is accepted as role
   `reviewer` (A) and as role `manager` (S). It is refused as a `reviewer`
   when its effort is not high. A declared role that contradicts the bound
   identity is refused.
2. Sanitized Copilot fixtures: a session whose `model.*` utility events name
   `gpt-4o-mini`, `gpt-5.4-nano` and `gpt-5.6-luna-utility` resolves to the
   agent model alone. A `session.model_change` without an effort inherits the
   previous effort. A model change between turns yields a history with both
   models. With no change event at all, an assistant or tool event whose
   `data.model` differs from the selected model adds that model to the
   history: the history is resolvable, `ck record` lists both models and
   both families, and `ck review` then refuses because the history is not
   exactly the reviewer's class mapping.
3. A Codex fixture resolves from `turn_context`. A Claude Code fixture without
   effort evidence is unresolvable. A missing log, or an assistant event before
   any selecting event, is refused.
4. `ck record` embeds a payload whose digest matches. It refuses without
   evidence. A model switch lists both models and both families. A
   raw-recorded patch is `unknown` and needs both families.

**Tracker**

5. Concurrent `issue new` calls allocate unique IDs.
6. A spec edit creates `r<k+1>` and stops old approvals from counting.
   Descriptive edits do not.
7. A dirty metadata repo is refused until `ck meta recover` runs. Editing an
   event file is detected.

**Eligibility**

8. `{claude}` needs codex, and `{codex}` needs claude. `{claude,codex}`,
   `unknown`, `human` and multi-family authorship each need both, from
   distinct sessions.
9. Reviewer-is-author, wrong class, wrong effort and multi-family reviewers are
   refused.
10. Approving S1 = A+B does not make S2 = A eligible. An incremental review
    over a covered P does cover S.
11. Each of these makes a submission ineligible: revocation, withdrawal,
    supersession, a stale spec, an edited capture, or blocking.
12. A superseded predecessor with `changes-requested` does not block its
    successor. A live sibling with `changes-requested` that shares a patch
    does block. An unreviewed live sibling does not block.
13. Submit refuses a closure patch that touches a `doc_path` file.

**Batch, locks and races**

14. Two eligible submissions get one CI run, and stay unpublished until
    integration approval. `ci.lock` is free while review is pending.
15. Ineligible submissions are never frozen. A split yields a new
    passing-subset candidate that needs its own review.
16. Mixed-family membership needs integration reviews from both families.
17. Rejection with `--blame` blocks only the blamed submissions.
18. Publish refuses if any of these happened after approval: the base
    advanced, the fingerprint changed, a member was revoked or a spec edited,
    or the receipt binds a different validated record. A spec edit attempted
    while publish holds the locks waits and then makes the next evaluation
    fail.
19. A crash before `publishing.json` publishes nothing. A crash after the
    pointer write is completed by a rerun.
20. Ownership: a prepare paused between its freeze and its CI start keeps
    `ci.lock`, so a concurrent `cancel` refuses and a second `prepare` cannot
    mark it interrupted. Once the owner exits without a result, the next
    `prepare` marks the batch interrupted. `cancel` of a validating batch
    refuses.

**Docs**

21. With no batch active, docs publish immediately. During CI and review
    they queue, and `ck docs flush` reports deferred. Both legal lock
    schedules are tested: a flush holding `state.lock` publishes before a
    waiting prepare can freeze; and when prepare freezes first, a later
    flush acquires the lock and defers without writing.
22. After publish, queued docs flush in FIFO order onto the new code, under the
    same lock hold. On rejection, cancel or failure they flush onto the
    unchanged code. A terminal transition followed immediately by the next
    `prepare` publishes the earlier docs first. With two queued entries, a
    crash after the journal record and before the pointer write, after the
    first publication, and after the final publication before the command
    returns are each resumed by a rerun of `ck batch publish`, which accepts
    the docs descendant and neither replays nor overwrites the code pointer.
23. A test hook captures a docs entry D1 from a stale base, so that it
    conflicts at flush, and a later entry D2 on another path depends on D1.
    D1 is marked `conflict` and keeps its evidence; D2 is blocked and waits.
    Capture on D1's or D2's paths refuses. A new capture and a `ck sync` on a
    disjoint path then succeed without pulling D1 or D2, and the new entry
    publishes. `ck docs resolve DID --record`
    publishes one group entry, the waiting entries then publish in order, and
    a later independent docs edit on the same path publishes on top.
    Executable Markdown is refused. `submit` refuses a code patch that depends
    on queued docs.

**Bootstrap and cutover**, with the vendored legacy controller managing the
fixture hub:

24. `prepare` saves the exact legacy pointer bytes, installs the hold
    tombstone, and validates. Its candidate is the exact base plus submission.
    During the hold, the vendored legacy `status`, `submit`, `accept`, `docs`,
    `worker` and `sync`, and the new trampoline, all fail closed with no state
    change.
25. The process scan, with an injected lister, refuses `prepare` and `publish`
    when another `tools/ck` state command or `pipeline.py` of this hub is
    running. It ignores the helper itself and its ancestors.
26. A vendored legacy `submit`, paused by a hook after reading state and
    released after the hold, leaves a new manifest. `publish` then refuses
    because the queue differs from the frozen snapshot. A legacy manifest
    written after publication is shown as `legacy-late` and is never
    eligible.
27. Publish refuses in each of these cases:
    - the hold tombstone or saved pointer changed;
    - a manifest or capture was edited;
    - an unrelated pending legacy submission exists;
    - the candidate or CI receipt changed;
    - the evidence index digest is wrong, or a listed file differs;
    - any receipt subject is stale;
    - a reviewer is the same family as the author, is an author session, is
      not class A or high, or has a missing or mismatched session log;
    - the integration receipt is missing.
28. Failure and cancel: a failing CI, an interrupted prepare (found by the next
    `prepare`), and `cancel` each replace the hold tombstone, in one write,
    with the saved legacy pointer or a docs chain built on it from the queued
    bootstrap docs, without any manual step. With two queued entries, a crash
    after the first chain link, after `restoring.json`, and after the pointer
    write but before `restored.json` are each completed by a rerun of
    `cancel` and by the next `prepare`; neither ever writes the saved base
    over published docs.
28a. Freeze prefixes: a `prepare` crashing after `frozen.json`, after
    `legacy-pointer.json`, and after `hold.json` (each before the hold pointer
    write) is recovered by the next `prepare` and by `cancel` as
    `never-held`, without touching `accepted.json`, including when a legacy
    `docs` advanced it in between. A crash after the hold pointer write is
    recovered by a full restore and drain.
28b. Captures and restore: a bootstrap `docs` capture made after the hold and
    before the failure record is included in the drain. After `failed.json`
    or `cancelled.json`, and after a crash following `restoring.json`, the
    bootstrap `docs` command refuses and names `cancel`. Recovery then
    restores with every earlier entry published, and legacy `tools/ck docs`
    works again.
29. Interruption of publish:
    - before `publishing.json`, nothing changes;
    - after the tombstone and before v2, the new `tools/ck` refuses, legacy
      commands fail closed, and a rerun re-verifies and completes;
    - with the tombstone written, a candidate, CI receipt or evidence file
      mutated before the rerun makes it refuse as an incident, writing
      nothing;
    - after v2, a rerun runs only the tail;
    - with two queued docs, a crash after the first tail docs publication,
      and a rerun after a complete tail, each find a docs descendant of the
      intended v2 bytes and resume the tail without an incident.
30. After publish, the vendored legacy commands fail closed with no state
    change. The trampoline runs the accepted controller even when a copy's
    `pipeline.py` and `environment.py` are modified to write a marker: no
    marker appears, and the launcher's origin assertion holds. The trampoline
    works pre-activation against the legacy controller. `install.py` reads v2.
    Legacy manifests and receipts still display.
31. The helper refuses a `--hub` that differs from the capture's `.ck-hub`,
    and refuses when run from outside its capture. Started exactly as §11.1
    documents, under `env -i` with only `HOME` set, it passes its initial
    capture check, configures its own environment and signs correctly.

## 14. Superseded text elsewhere, and out of scope

Text in other documents that this specification supersedes:

- **`docs/archive/setup-hardening-plan.md`**, which is not edited:
  - §1, publication immediately after full CI;
  - §2, docs waiting behind acceptance;
  - §4, the `accept` row;
  - §5, the lock model and rollout;
  - the §10 GC reader table. Retention must add batch records, candidates,
    the docs queue, receipt-referenced captures, bootstrap records and the
    metadata repo.
  - §12, stage 2 lock introduction;
  - §13, docs waiting.
- **`docs/tooling.md`**: the acceptance transaction and failed-batch
  auto-publication. These are updated through the docs queue (§12).

Out of scope:

- the Rust/tooling hardening plan;
- Copilot chat transport;
- the live IRC pilot;
- GC;
- `ck export`;
- explicit owner-override commands;
- any change to model classes or shared chat code.
