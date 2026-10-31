# Bootstrap fixture specification

Stable stand-in bytes for the review gate plan. The bootstrap tests copy this file to
`docs/review-gate-plan.md` inside their fixture repositories and hubs, because
`bootstrap_core` looks for that path inside a candidate checkpoint. The tests treat it as opaque
bytes (copied and digested); its content is not a contract and does not track the real gate plan.
