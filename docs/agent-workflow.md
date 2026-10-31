# Codekarl agent workflow

The master role policy is [skills/AGENT-POLICY.md](../skills/AGENT-POLICY.md).
It is executable workflow guidance and travels through the code/review lane.
Every ck skill reads it. Current model IDs live only in
`~/.config/model-classes.toml`; inspect them with `modelclass list`.

| Job | Class | Default model family | Effort |
| --- | --- | --- | --- |
| Management and design | S | Claude | high |
| Implementation | B | Claude | high |
| Review | A | Opposite the implementation family; normally Codex | high |

The model family determines review independence, not the application or billing
provider. Opus/Sonnet through Copilot counts as Claude. An OpenAI model through
Copilot counts as Codex. A session is canonical when its actual selected model
and effort match the requested class. Preserve owner overrides and histories;
record model changes in task evidence. Unknown or mixed implementation provenance
requires both independent review families.

After an owner switches a live manager or worker session to its class's model,
the gate accepts it again: its current model and effort must match the class.
`tools/ck session --role ROLE` shows the current pair, and lists earlier
off-class turns as `off_class`. Those turns stay in the session's recorded
history. A reviewer with any off-class turn still needs a fresh session.
See review gate plan §2.4.

`modelclass run` resolves the native Claude Code and Codex clients. Copilot is
currently selected separately against that same master. Its model spelling can
differ; the master's `clients.copilot.model_aliases` table records verified
aliases. An alias is not permission to substitute a model. No plugin is used.

## Skills and terminals

- `ck-manager` coordinates design, tasks, reviews and eventual batch acceptance.
  (Its former alias `ck-manage` was retired by CK-75 on 2026-10-20.)
- `ck-work` implements a bounded assignment in its own Pijul worker directory.
- `ck-review` reviews an exact immutable submission with recorded provenance.
- `ck-chat` owns inbox recovery, acknowledgements and project conversation.

The owner supplies the cmux terminals and prefers two vertical panes per
workspace. Keep the existing layout and full session histories. A session's
role is distinct from its model family and its terminal location. Resolve UUIDs
from live cmux state before operating on a terminal; do not rely on stale refs.

Manager handoff and task records live under `.pipeline/coordination/`. Each task
ties its authorization and spec to the working copy, implementing sessions and
brands, submission, exact review evidence and eventual CI receipt. IRC remains
the shared development record. Ordinary documentation publishes through `ck docs`.

## Current state (2026-10-17)

The role/skill policy and the mechanical review gate are installed, and the owner's `ck-workflow-ready` goal is
complete: every pilot repair and setup-hardening slice is accepted (see
[setup-hardening-plan.md](archive/setup-hardening-plan.md) §3.2 and the closed
[live-pilot-findings.md](archive/live-pilot-findings.md)). Chat delivery is on as the routine state. There is no open issue.
The setup hold is lifted (CK-39, 2026-10-17): the manager dispatches owner-authorized work through the normal review and
acceptance workflow, and does not choose its own backlog or take on work beyond what the owner assigns. The owner's deliberate deferrals (busy Codex stop through
chat; remaining live recovery coverage) are known limitations, not pending tasks.

Copilot provenance support is installed, but its chat discovery/delivery
adapter is not. The pilot used native clients. An Opus model does not make a
Copilot terminal behave like Claude Code; a Copilot session must not
impersonate another IRC identity or claim automatic wake-up works.

The gate requires reviewed specs and immutable submissions, combined full CI,
and independent integration review before publication. A batch with one submission whose candidate is byte-identical to
the approved capture skips the separate integration review (CK-38): its submission review is the reviewed basis, and the
gate checks the identity, the fresh coverage and the CI timing margin itself. Chat completion and
passing CI alone confer no acceptance authority. The former model-history
limitation (PF-12) was resolved by CK-9.

See [chat-workflow.md](chat-workflow.md) for the bridge and inbox contract,
including observed implementation gaps and historical deployment evidence.
