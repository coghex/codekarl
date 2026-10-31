# Bootstrap review verdict: 1791997256798611000-01db6481

> **Historical review record (2026-10-14).** The reviewed bootstrap was accepted. The deployment preconditions below were
> completed at rollout, and the non-blocking suggestions are kept for reference. Several were overtaken by the later
> review gate, for example N4, author provenance, which `ck record` now records. None is a scheduled task. Current state:
> [agent-workflow.md](../../agent-workflow.md).

**Verdict: APPROVED** (no unresolved blocking findings; non-blocking findings and
deployment preconditions below)

## Exact scope

- Submission: `1791997256798611000-01db6481`
- Capture: `~/work/codekarl/.pipeline/submissions/1791997256798611000-01db6481`
- Patches (manifest `hashes`): `4JPVKWDSYL2RDVEYPDX2AFO57NV324H73POYBEVMYTJJVNGMFD5AC`
  (Pijul author field "Codex", 2026-10-14 10:00:56 PDT: "Define Codekarl agent
  classes, client-independent review policy, and scoped workflow skills")
- Queue manifest SHA-256 (bytes):
  `ac5b46ba8782ea3e43f162d65a53b8ff26ce739a3d58cf76f1d29857eaf9f179`
- Task base: `~/work/codekarl/.pipeline/checkpoints/1791996147883630000-8805a10d`
- Accepted state at review time: docs-only checkpoint `1791997330751448000-4d5a763d`
  (adds patch `FB4EEG…6AC`, `docs/agent-workflow.md`; `accepted.json` SHA-256
  `893c35bc4ab23028e58a3a9a0e026836b4fcd2a2834847011195fbcc59a044ca`). Base
  advancement is docs-only; it does not change the reviewed patch.
- Spec (from manifest): "Codekarl manager/design uses S, implementation B, and
  opposite-model-family review A at high effort. Skills resolve the shared model
  master, preserve visible cmux sessions, and hold acceptance until review
  eligibility is enforced. Copilot Opus is canonical Claude-family work;
  unsupported terminal delivery remains explicitly paused. Skill frontmatter
  validation passes."

Changed paths versus base (all read in full): `AGENTS.md` (+6 lines),
`skills/ck-chat/SKILL.md` (+4 lines), new `skills/AGENT-POLICY.md`,
`skills/ck-manage/SKILL.md`, `skills/ck-manager/SKILL.md`,
`skills/ck-review/SKILL.md`, `skills/ck-work/SKILL.md`. No code, hooks,
`pijul.toml`, tests or tools changed.

## Reviewer identity

- Brand: Claude family (opposite the declared Codex author). I did not author any
  part of this change.
- Class A, resolved via `modelclass list`: `claude-opus-5-5 high`; the Copilot
  alias in `[clients.copilot.model_aliases]` is `claude-opus-5.5`.
- Actual: model `claude-opus-5.5`, effort high, client GitHub Copilot CLI 1.0.97.
- Session ID: `9bcdbe98-41c4-4fdf-9a6f-5eb4cc70a523`. Its `events.jsonl` records
  session effort `high`. All 21 assistant messages and 28 tool executions before
  this file was written ran on `claude-opus-5.5`. The only other model calls were
  Copilot startup utility calls (`gpt-5.6-luna-utility` at medium and
  `gpt-4o-mini`) that ran before any review content; they produced no review
  output.
- No chat identity was used or claimed. This verdict is out-of-band because
  Copilot has no tested chat adapter.
- Authorship is taken from the Pijul author field and the coordinator's request
  ("Codex-authored"). The manifest has no author ledger, and the shared signing
  key does not identify a brand.

## Checks performed

1. Manifest digest: computed before review and again before signing. Both match
   the value above.
2. Capture integrity: `tools/pipeline.py` `fingerprint(capture)` equals the
   manifest fingerprint, both before review and before signing. `pijul status`
   reports no changes or untracked files.
3. Patch set: the capture equals the base plus exactly `4JPV…AC`. The only
   accepted hash missing from the capture is the later docs patch `FB4EEG…6AC`.
4. Combined state: in a disposable clone of accepted checkpoint `4d5a763d`
   (with `.ck-hub` set), `pijul pull --complete` of `4JPV…AC` reported
   `{"conflicts":[]}`. The resulting tree contains both `docs/agent-workflow.md`
   and `skills/AGENT-POLICY.md`, which it links to. The clone was then deleted.
   Full CI was not run; it belongs to `ck accept`.
5. Frontmatter: `~/.codex/skills/.system/skill-creator/scripts/quick_validate.py`
   reports "Skill is valid!" for ck-chat, ck-manage, ck-manager, ck-review and
   ck-work.
6. Central models: the skills, policy and AGENTS.md contain no model IDs; only
   family names ("Opus or Sonnet") are mentioned. `modelclass check` returns `ok`.
   `modelclass flags` resolves S/claude to `--model claude-opus-5-5 --effort high`,
   A/codex to `-m gpt-6.1-sol -c model_reasoning_effort=high`, and B/claude to
   `--model claude-sonnet-5-5 --effort high`.
7. Factual claims checked against the installed tools:
   - The `modelclass run --chat-role` choices are `{manager,guide,solver,reviewer}`;
     `worker` is absent, as the policy says.
   - `identities.prepare_launch` honors `CHAT_ROLE` and allows `worker` only for
     Codekarl.
   - `modelclass run` brands are `{claude,codex}` only, as the policy says.
   - `tools/pipeline.py` `accept` batches every unaccepted, unblocked manifest and
     checks no review receipt, as the policy says.
   - `ck-chat inbox --all --history` works without a bound identity.
   - An unbound `ck-chat post` reaches `pchat who()`, which refuses
     ("say who you are"). It does not fall back to the owner's or another identity.
8. Queue at review time: the four earlier manifests are all listed in
   `accepted.json`. This submission is the only pending one.

## Requirement assessment

| Owner requirement | Result |
| --- | --- |
| Codekarl-only `ck-*` skills, no plugin, Pijul batch CI | Met. Every description is scoped to Codekarl, no plugin is used, and the skills use `tools/ck` submit/accept/docs. |
| Design/manager S, workers B, reviewers A, all high | Met: policy table and `--effort high` rule. Design inside a worker task is returned to S. |
| Claude manager/solvers and Codex reviewers by default; every implementation reviewed by the opposite family | Met. Review is chosen by the author's brand, not the manager's. Mixed or unknown authorship needs both families, and a reviewer who writes a fix becomes its author. |
| Brand means model family, not CLI | Met. Copilot Opus counts as Claude, the client is recorded separately, and an unknown family cannot satisfy review. |
| Models change centrally in `model-classes.toml` | Met. Models are resolved at each launch and aliases stay in the master file. |
| Visible cmux sessions, histories and two-pane layouts stay owner-controlled | Met. The skills defer to `cmux-supervisor`, require live UUIDs, and forbid creating, moving, clearing or replacing sessions. |
| Copilot delivery is not falsely claimed | Met. The degraded mode is explicit, with no impersonation and no claim of automatic wake-up. |
| Acceptance held until review eligibility is enforced | Met. The policy says plainly that no gate exists yet, and the bootstrap-only acceptance procedure is narrow. |
| `ck-manage` usable in the existing Copilot session | The content is ready, but it is not installed (precondition P1). |

## Blocking findings

None.

## Deployment preconditions (not defects in the reviewed bytes)

- **P1. The new skills are not installed anywhere.**
  - `~/.copilot/skills` is empty. `~/.claude/skills` and `~/.codex/skills`
    contain only `ck-chat` from this project.
  - The root working copy (`~/work/codekarl`) does not have
    `skills/AGENT-POLICY.md` or the four new skills until acceptance and
    `ck sync`, yet every skill names those absolute paths.
  - Install only after acceptance and sync. Symlink to the root, as was done for
    ck-chat:
    - `ck-manage` and `ck-manager` into `~/.copilot/skills`;
    - `ck-manager`, `ck-work` and `ck-review` into `~/.claude/skills` and
      `~/.codex/skills`.
  - Otherwise the policy's `/ck-manager` and `$ck-review` launch examples will
    not resolve.
  - This Copilot session does not list `~/.claude/skills` entries (`ck-chat`,
    `model-classes`, `cmux-supervisor`) as skills. The Copilot manager therefore
    also needs `ck-chat`, `model-classes` and `cmux-supervisor` in
    `~/.copilot/skills`, or it must be told their absolute paths, because the
    policy and ck-manager refer to them by name.
  - `tools/chat/install.py apply` verifies and installs only ck-chat. No
    installer covers the new skills.
- **P2. Manager bootstrap ordering.**
  - `.pipeline/coordination/manager-bootstrap.md` tells the Copilot session to
    "read the accepted skills" at `…/skills/ck-manager/SKILL.md`.
  - Those files are not accepted and do not exist in the root copy, and
    `handoff.md` has not been written yet.
  - Start the manager only after this submission is accepted, synced and
    installed. Do not point it at the capture as if it were accepted.

## Non-blocking findings (recommend a follow-up implementation task)

- **N1. The alias breaks role auto-detection** (`skills/ck-manage/SKILL.md`).
  - The installed `identities.prepare_launch` detects the manager role only from
    `[/$]ck-manager\b`.
  - A Codekarl launch using `/ck-manage` without `--chat-role manager` would
    therefore bind as a **worker**.
  - The policy's examples always pass `--chat-role manager` with `/ck-manager`,
    so this occurs only if someone launches through the alias.
  - Fix either way: have ck-manage say it is for client-side invocation, never as
    a modelclass launch argument, or extend the adapter's regex.
- **N2. ck-manager conflicts with its own degraded mode** (`skills/ck-manager/SKILL.md`).
  - Lines 26-27 ("Publish concise decisions and results to IRC") and 57-59
    ("rely on the chat bridge to wake this persistent session", "Report … through
    `ck-chat`") are unconditional.
  - They contradict the Copilot exception at lines 13-16. Attempting them fails
    safely (`pchat` refuses an unbound post), but the text should state the
    degraded-mode substitute: report in the visible terminal or handoff, and do
    not expect a wake-up.
- **N3. ck-review has no path for a Copilot reviewer** (`skills/ck-review/SKILL.md`).
  - It requires a bound `ck-reviewer-N` identity and a claimed assignment, so a
    canonical Claude-family reviewer running in Copilot cannot follow it.
  - This review relied on an out-of-band coordinator exception. Either document
    that Codex-authored work goes to a native Claude Code reviewer launched with
    `modelclass run A claude`, or extend the documented Copilot exception to
    reviewers.
- **N4. Authorship is not recorded in the submission.**
  - The policy requires an author ledger, but `ck submit` records no author brand,
    and this bootstrap task has no `tasks/<id>.md` ledger.
  - The future mechanical review gate should bind author brand and session into
    task records, or into the manifest via a spec field.
- **N5. Earlier submissions predate the policy.**
  - Earlier accepted submissions (for example `1791995908970340000-cf06e0e4`, the
    Codex-authored chat routing) were accepted before this policy existed.
  - The policy says nothing about retroactive review. The owner may want to
    decide whether earlier Codex-authored executable work needs Claude-family
    review.

## Acceptance note

This approval covers exactly the bytes above (manifest SHA-256 `ac5b46ba…f179`,
patch `4JPV…AC`) and that specification. Under the policy's bootstrap rule, the
coordinator may accept this submission only while all submitting sessions are
held and this is the sole pending manifest. Re-run `tools/ck status` immediately
before `ck accept`, and report the actual CI receipt and the accepted IDs.
