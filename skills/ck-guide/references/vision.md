# The vision document

`docs/vision.md` is the owner's heading: a decision map, not a copy of the designs. The large game-design sections it indexes live in
`docs/game/*.md`, and together they are the heading. Today it is a **provisional interview record**:
it marks items **Firm**, **Open** and **Proposal**, has no `V-N` principle IDs, and the owner extends it as the interview proceeds.
The guide reads the versions published at the pin and never edits any of them: the Firm, Open and Proposal rules below apply to the
game documents exactly as to the vision.

## How the guide maps it

- The guide gives each principle it exercises a stable key of its own, `vision.<name>` (lowercase letters, digits and hyphens, for
  example `vision.economy-first`), in the coverage's `principles`.
- Each mapping pins a section heading, a short verbatim quote and the SHA-256 of that quote's UTF-8 bytes. It may name its `document`:
  `docs/vision.md` (the default, only when the key is absent) or a `docs/game/*.md` file the run pinned; a present value that is
  anything else (a number, `null`, an empty string, another path) is refused, never read as the vision. `validate` refuses a key not of the form `vision.<name>`, a
  missing part, a document outside the heading, a quote or section not in that pinned document (a quote that exists only in another
  heading document does not pass), and a wrong SHA-256.
- Only **Firm** items are standing direction. Never treat an **Open** or **Proposal** item as settled, never promote one to Firm, and
  never raise a finding for failing to implement one.
- When a later publication changes a mapped quote, that mapping is pending until it is reviewed again (`recheck` reports any heading
  document that changed since the pin, and any added to or deleted from the heading).

## Authority

- The vision and the game documents change only when the owner settles a choice, with the decision's date and source recorded beside the change. A newer
  explicit owner decision overrides an older summary; an agent proposal, a review receipt or accepted code does not.
- When two authorities conflict, raise an owner decision; do not pick one quietly.
- **Stale Open items:** where the vision still marks Open something the owner has since decided, the report records a handoff note
  naming the decision's source. The audit does not edit the vision, and a note is not a finding.
- Edits waiting in a docs queue are provisional: the guide keeps reviewing against the published version.

## When no usable vision exists

Draft nothing alone. Review for correctness, report no alignment readings, and ask the owner to confirm, amend or reject a short
proposed heading built from their explicit decisions. Silence is not acceptance.
