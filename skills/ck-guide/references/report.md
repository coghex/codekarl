# Guide report and coverage

Each run writes two files to `docs/guide/`, in a docs copy, and publishes them with `tools/ck docs`:

- `<stem>.md`, the report people read.
- `<stem>.coverage.md`, the machine record: one fenced `json` block, written by `tools/ck-guide finish`.

The stem is `yyyymmddthhmmssz-<8 hex of the pinned pointer>`, all lowercase, and `tools/ck-guide start` chooses it. Use
repository-relative links. A temporary copy's path is never a durable link.

## The report

Write a handoff someone can read in five minutes, about 250 lines at most. Inventories, hashes and per-item verdicts belong in the
coverage file, not here.

```markdown
# Guide review: <what this batch was about>

<Two to five sentences. Is the new work aligned? What changed in the alignment readings? Which material findings exist, if any? What
does this cover: the pinned pointer and the range. It is not a whole-project correctness claim or an approval.>

Status legend: `[ ]` unprocessed · `[#N]` filed as issue N · `[no-issue]` reviewed and deliberately never to be filed · `[deferred]` blocked on a concrete precondition

## Status

- [ ] GUIDE-1. <the concrete repair, amendment or decision>
- [x] GUIDE-2. <an already-dispositioned finding> — [#207]

Next step: <the one thing to do next, and why>.

## Alignment

| Principle | Reading | Change | Note |
|---|---|---|---|
| vision.economy-first | drifting | was aligned | GUIDE-1: … |

(List only the principles this batch exercised. The cursor carries the rest.)

## Findings

### GUIDE-1. <the concrete repair, amendment or decision>

**Disposition:** repair / issue amendment / documentation drift / owner decision.
**Verification:** verified / partially verified (say what is missing).
**Principle:** `vision.<key>` and the accepted contract.
**Evidence:** pinned path:line, the effective issue clause, and the probe or check.
**Consequence:** the reachable failure or misleading claim.
**Action:** the smallest useful change, existing coverage, remaining uncertainty.

## What moved

<One to three lines per publication or changed issue: what it integrates, and anything a later reviewer should know. Group small items.>

## Evidence and limits

<Checks that ran, with outcomes; evidence reused from an exact revision; checks left out and why; unavailable environments.>

## Handoff

<Items not covered. What the recheck found moving. Pending concurrent work, including stale Open items in the vision that the owner has
since decided (name the decision's source). The recommended order for continuing.>
```

The `Status legend:` line must appear literally. Finding IDs run `GUIDE-1`, `GUIDE-2` and so on within a report; elsewhere refer to
`<stem>/GUIDE-N`. Each finding is a `### GUIDE-N. title` heading, and `validate` requires the coverage's `findings` list and the
report's headings to match both ways.

When a finding is dispositioned, prefix its heading: `### [#207] GUIDE-2. …`, `### [no-issue] GUIDE-2. …` or
`### [deferred] GUIDE-2. …`. A deferred finding stays unchecked and names a concrete precondition. A proposal waiting for approval
stays unmarked, with its exact text under the finding. Only an applied disposition changes the marker. A report with no findings
keeps the status legend and a `## Status` section that says no actionable finding was established.

## The coverage file

`tools/ck-guide start --resume` writes `coverage.json` in the run directory with every item `unreviewed`; you fill in the judgment
fields, and `finish` copies the finished record into `<stem>.coverage.md`. The mechanical fields are the helper's; do not edit them.

| Field | Owner | Meaning |
|---|---|---|
| `schema`, `stem`, `mode`, `scope`, `started_at`, `completed_at` | helper | identity; `mode` is `full`, `review-only` or `scoped` |
| `pin` `{pointer_sha256, repo, publication}` | helper | the reviewed accepted pointer |
| `baseline` `{pointer_sha256, source}`, `until` | helper | the boundary this run started from, and the optional end of the range |
| `review_copy` `{path, log, log_sha256}` | helper | the copy and log the chain was verified against |
| `publications[]` | both | each batch or docs publication from the baseline to the pin: `kind`, `id`, `basis`, `base_sha256`, `pointer_sha256`, `members` (submission IDs, manifest SHA-256, patch hashes, issues), `hashes` (actually published), `queued_hashes` (provenance, docs only), `files`; you set `verdict`, `depth`, `note` |
| `issues[]` | both | open issues, those the range touches, and closed issues an earlier report covered whose definition has since changed: `title`, `type`, `state`, `depends`, `spec_revision`, `spec_sha256`, `comments`, `reviews`, `definition_sha256`, `state_sha256`, `change` (`new`/`changed`/`unchanged`); you set `verdict`, `depth`, `note` |
| `documents[]` | helper | the heading documents (`docs/vision.md` and every `docs/game/*.md` the pin lists, pinned whether or not the range changed them) and the contracts the range changed, each with `path`, the `sha256` pinned (checked against the pin's fingerprint; the bytes are kept in the run directory) and the `publication` that last changed it, possibly before the baseline, or empty if none is recorded; `absent: true` (no digest, no stored copy) for a file the pin's fingerprint does not list |
| `principles` | you | `{vision.<key>: {section, quote, sha256, reading, note, document?}}`; `document` is `docs/vision.md` (the default) or a pinned `docs/game/*.md` file; a reading is `aligned`, `drifting` or `not exercised` |
| `findings[]` | you | this report's finding IDs |
| `checks[]` | you | `{command, outcome, evidence, reused_from?}` for checks that ran, and the omitted ones with a reason |
| `pending[]` | helper | what `recheck` and `finish` found moving after the pin, including a heading document edited, added or deleted |
| `boundary` | helper | the cursor boundary this run supports: `{id, patch, pointer_sha256}` |

Rules:

- The coverage is bound to the run: `validate` and `finish` compare it with `inventory.json` and `run.json`. Only each item's
  `verdict`, `depth` and `note`, and `principles`, `findings` and `checks`, may differ; an item cannot be added, dropped or reordered, an
  `outside-range` item (after `--until`) cannot be judged, and `inherited` is only what the inventory marked.
- `verdict` is `reviewed` (examined at the recorded revision; needs `depth`), `inherited` (an unchanged item an earlier guide report
  covered; historical coverage, not a fresh check), `gap` (needs a reason in `note`), or `outside-range`
  (after `--until`, or beyond a scoped run's scope). `unreviewed` is not a final verdict. A `scoped` run may mark only its scope
  `reviewed` or `inherited`.
- Issue fingerprints are the SHA-256 of canonical JSON (sorted keys, compact separators, non-ASCII kept): the definition covers title,
  type, sorted `depends`, spec revision and digest, and the comment digests; the state covers state, close reason, assignee,
  priority, and the review receipt IDs with their verdicts. A definition change makes the issue `new`/`changed` again; a state-only
  change keeps the inherited verdict and is reported.
- Later reports supersede an item's conclusion only at the revision they state. Never infer coverage of revisions in between, and
  never rewrite an older coverage record.
- Processing findings later updates the report's markers, not its coverage file. The coverage stays the snapshot of what the audit saw.
