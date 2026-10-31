# The cursor

`docs/guide/CURSOR.md` is the guide's last fix: the one file a fresh session reads to know where the review stands and what to do
next. It is derived from the reports, so the reports stay authoritative. `tools/ck-guide finish` rewrites the whole file at the end of
every full or scoped run (never a `review-only` run), and `tools/ck-guide rebuild-cursor --docs-copy PATH` derives the same file from
the reports alone. Do not hand-edit the lines the helper writes; when the follow-up walk dispositions a finding, update the report
heading and checklist, then rebuild.

```markdown
# Guide cursor

Resume after: `<publication ID>` · patch `<last patch hash>` · pointer `<64-hex SHA-256>` · [<stem>](<stem>.md) · <UTC time>
Covered beyond the boundary: <publication ID (stem), …, or "none">

## Next

<One line: the recommended next piece of work and why.>

## Open findings

- <stem>/GUIDE-N — <one line> — unprocessed | deferred

## Pending handoff

- <a publication after the boundary, or a note such as a diverging chain>

## Alignment

| Principle | Reading | Since | Note |
|---|---|---|---|
| vision.economy-first | aligned | <stem> | |
```

The helper finds the boundary with ``Resume after:\s*`<ID>`( · patch `<hash>`)?( · pointer `<sha>`)?``, so keep that line's shape
exactly. The publication ID is a batch ID or a docs publication ID; the pointer SHA-256 is the accepted pointer that publication
produced, which is what places the boundary on the chain.

## Rules

- **Advance only over what the guide reviewed.** `Resume after` moves to the last publication of the contiguous prefix that is
  `reviewed` or `inherited` from an earlier guide report. A `gap`, `unreviewed` or `outside-range` item stops it, and later covered
  publications are listed under "Covered beyond the boundary". The next `start` sees them as inherited and shows the gap again. A
  publication's gate review, receipt or acceptance never counts as guide coverage.
- **Never move back.** The boundary is the furthest one, on the current chain, among the completed non-`review-only` reports in the
  docs copy **and** in the currently published `docs/guide` files. An older run that finishes after a newer one, even from a docs
  copy that is behind the published guide, therefore does not regress it. A report whose boundary is not behind the current accepted
  pointer is on a diverging chain: it is not used, and "Pending handoff" says so.
- **`review-only` runs never touch the cursor** and their coverage never moves a later one.
- **Readings carry forward.** A principle this run did not exercise keeps its previous reading and `Since`. A reading changes only with
  evidence from the run that changed it.
- **Open findings mirror the report headings.** Every finding not yet `[#N]` or `[no-issue]` is listed. When they disagree, the report
  headings win.
- **Rebuild whenever in doubt.** A cursor that is missing, stale or inconsistent is rebuilt from the reports.
- **Memory points here.** The repository's memory keeps one standing line linking the vision and this file; it never gets a
  paragraph per run.
