# Codekarl documentation index

Documents are kept at their paths, with their history. This index says which ones describe the installed system today and which are
completed, dated records. Executable guidance (`AGENTS.md`, `skills/`) is outside `docs/` and passes the code gate.

## Current: guides and contracts

| Document | What it covers |
| --- | --- |
| [tooling.md](tooling.md) | The `tools/ck` workflow: check levels, submission, batches (including single-submission publication), GC, recovery, Metal and Cargo |
| [agent-workflow.md](agent-workflow.md) | Roles, model classes, sessions and the current project state |
| [chat-workflow.md](chat-workflow.md) | The chat contract (`ck-chat`, delivery, identities); its dated rollout sections are historical |
| [review-gate-plan.md](review-gate-plan.md) | The specification of the installed review gate. Despite the "v5" title, it is the operative contract, amended in place (for example by CK-38) |
| [window-design.md](window-design.md) | **Provisional draft:** milestone 3 windowing (native/Metal and GLFW/Vulkan), separating owner direction, proposals and open decisions |
| [vision.md](vision.md) | **Provisional:** the game vision as the owner interview proceeds. It separates firm direction, examples and open decisions; it is not design-ready |
| [game/](game/) | The vision's detailed game design, one document per system (economy, crown/military/church, diplomacy, research, maritime), indexed from vision.md |

## Completed records (history, not instructions)

| Document | Record of |
| --- | --- |
| [setup-hardening-plan.md](archive/setup-hardening-plan.md) | The setup-hardening slices, all accepted (completed 2026-10-17) |
| [live-pilot-findings.md](archive/live-pilot-findings.md) | The closed findings ledger of the 2026-10-15 live chat pilot |
| [chat-identity-incident.md](archive/chat-identity-incident.md) | The 2026-10-14 shared chat identity incident |
| [reviews/](archive/reviews/) | Bootstrap and identity-conflict review verdicts (2026-10-14) |

A record is not current guidance where it disagrees with a current document. Fix current documents through `tools/ck docs`, and leave
records as dated history.
