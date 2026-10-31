# Bootstrap re-review verdict: 1791997773834930000-3cf0d8fe

> **Historical review record (2026-10-14).** The reviewed composition was accepted; acceptance then used the
> pre-gate `ck accept`, which the review gate has since replaced. Current state: [agent-workflow.md](../../agent-workflow.md).

**Verdict: APPROVED** (no unresolved blocking findings)

This verdict covers the final two-patch composition. The earlier verdict
(`bootstrap-verdict.md`, SHA-256
`0688c4b2c15800314bbc43f5cb24f82b089485c3296fdbee071beff3c0994d6e`) is unchanged
and still covers submission `1791997256798611000-01db6481`.

## Exact scope

- New submission: `1791997773834930000-3cf0d8fe`
- Capture: `~/work/codekarl/.pipeline/submissions/1791997773834930000-3cf0d8fe`
- Patches (manifest `hashes`):
  - `4JPVKWDSYL2RDVEYPDX2AFO57NV324H73POYBEVMYTJJVNGMFD5AC`: the prerequisite,
    reviewed previously and identical in both captures
  - `JR237IECJ6VGUI7TPXRMJ23IFJLVI6MJORUPBNTR4VNHK6C3AQWQC`: new; Pijul author
    "Codex", 2026-10-14 10:09:33 PDT, "Clarify alias launches and Copilot
    bootstrap communication"
- New queue manifest SHA-256:
  `65492cacb5cf650419bf3a48abe209c2e4dc1472b92eb6fe7459df4c23b688d6`
- Companion manifest `1791997256798611000-01db6481` SHA-256:
  `ac5b46ba8782ea3e43f162d65a53b8ff26ce739a3d58cf76f1d29857eaf9f179` (unchanged)
- Comparison base: the previous capture `1791997256798611000-01db6481` (task base
  `1791996147883630000-8805a10d`, as in the first verdict).
- Accepted state at review time: docs-only checkpoint
  `1791997810260262000-ad42866c`, which adds only `docs/archive/reviews/agent-workflow-bootstrap.md`
  (`accepted.json` SHA-256 `158c201747fc0db81138dc71dbf60181bf3f16d1bb65473e3dc5e159b29e0588`).
- Spec (from manifest): "Clarify ck-manage explicit manager launch role,
  condition manager IRC and wake instructions on tested delivery, and allow
  bounded Copilot reviews through verified coordinator handoffs while preserving
  all model-family and freshness requirements. Includes prerequisite 4JPV…AC.
  Addresses bootstrap review N1-N3."
- Declared author: Codex, session `01a13ab7-838c-76d0-99ff-a04491db37e0`. This
  comes from the coordinator; the Pijul author field is consistent with it.

Paths changed relative to the previous capture (fingerprint diff):
`skills/ck-manage/SKILL.md`, `skills/ck-manager/SKILL.md` and
`skills/ck-review/SKILL.md`. I read the complete diff of each. No code, tests,
hooks, `pijul.toml` or other files changed.

## Reviewer identity

- Claude family, opposite the Codex author. I did not author either patch.
- Class A, resolving to `claude-opus-5-5 high`, which Copilot spells
  `claude-opus-5.5`.
- Actual: model `claude-opus-5.5`, high effort, GitHub Copilot CLI 1.0.97.
- Same session as the first review: `9bcdbe98-41c4-4fdf-9a6f-5eb4cc70a523`.
- No IRC identity was used or claimed. The assignment came through the
  coordinator's file in the visible terminal, which matches the new ck-review
  exception.

## Checks

1. Manifest digest: computed at the start and recomputed before signing; both
   equal the value above.
2. Capture integrity: `tools/pipeline.py` `fingerprint(capture)` equals the
   manifest at start and before signing, and `pijul status` is clean.
3. Composition: compared with the previous capture, the hash set adds only
   `JR23…QWQC` and removes nothing.
4. Combined state: in a disposable clone of the current accepted checkpoint
   `ad42866c`, pulling `4JPV…AC` and `JR23…QWQC` with `--complete` returned
   `{"conflicts":[]}`. All five `skills/*/SKILL.md` in the result are
   byte-identical to the capture, and `docs/archive/reviews/agent-workflow-bootstrap.md`
   is preserved. The clone was deleted. Full CI was not run; the coordinator's
   batch gate covers it.
5. Frontmatter: `quick_validate.py` reports "Skill is valid!" for ck-chat,
   ck-manage, ck-manager, ck-review and ck-work in the combined tree.
6. Grouping: `Pipeline.groups` puts both manifests into one acceptance group,
   because they share `4JPV…AC`. They will be validated and promoted, or
   quarantined, together.
7. Queue: these two are the only manifests not yet listed in `accepted.json`.

## Resolution of earlier findings

- **N1 (alias role): resolved.**
  - `ck-manage` now requires `--chat-role manager` for modelclass launches and
    states that the adapter does not infer the role from the alias.
  - It also says that invoking the alias inside an existing session does not
    change that session's role.
  - This matches the installed `identities.prepare_launch` behavior, where
    explicit `--chat-role` takes precedence over regex detection.
- **N2 (degraded-mode contradiction): resolved.**
  - IRC publishing, bridge wake-up and `ck-chat` reporting now apply only to
    bound sessions with tested delivery.
  - The Copilot bootstrap exception substitutes the visible terminal and the
    handoff, and expects no automatic wake-up.
  - The skill no longer instructs an unbound post.
- **N3 (Copilot reviewer path): resolved.**
  - The exception is bounded. It applies only while Copilot delivery is
    unavailable and only to assignments from the owner or an authorized
    coordinator.
  - It requires owner provenance, exact task and submission scope, and the
    reviewer's model, effort and session evidence.
  - The verdict goes to a durable file that the coordinator publishes.
  - Impersonation, claiming another agent's inbox and expecting wake-up are
    forbidden, and all independence and freshness rules still apply.
  - It does not weaken model-family requirements, because brand eligibility still
    comes from AGENT-POLICY.md.
- **N4 and N5** (author ledger in submissions; retroactive review of earlier
  accepted work) are unchanged. They were non-blocking and remain open for the
  owner or a future review-gate task.
- **P1 and P2** (installing the skills into the Copilot, Claude and Codex skill
  directories only after accept and sync; starting the manager only from
  accepted and installed skills) remain deployment preconditions.

## Blocking findings

None.

## Non-blocking notes

- ck-review's general rules (for example "Publish a durable verdict through the
  ordinary docs lane" and "Post its path and digest in `#codekarl`") sit beside
  the new exception, which hands publishing to the coordinator. Read together
  they are consistent: the exception explicitly replaces only the unsupported
  communication.

## Acceptance note

This approval covers exactly the two-patch composition above: manifests
`65492cac…88d6` and `ac5b46ba…f179`, patches `4JPV…AC` and `JR23…QWQC`, and their
stated specs. Accept both together as one group, with submitting sessions held.
Re-run `tools/ck status` immediately before `ck accept` and report the actual
receipt and the accepted IDs.
