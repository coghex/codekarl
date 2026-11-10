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
| [vision.md](vision.md) | **Provisional:** the game vision as the owner interview proceeds. It separates firm direction, examples and open decisions; it is not design-ready |
| [game/](game/) | The vision's detailed game design, one document per system (economy, crown/military/church, diplomacy, research, maritime), indexed from vision.md |

## Current: milestone designs

Each milestone's design, with the owner's decisions (**Firm**), the approved proposals and its delivery plan. Its status line says how
far it was delivered. Where an issue spec or a later document refines a design, the accepted code and the later text win.

| Document | Milestones | What it covers |
| --- | --- | --- |
| [runtime-design.md](runtime-design.md) | 1, 2 | The runtime, logging, the exit statuses and the Lua VMs |
| [window-design.md](window-design.md) | 3, 3.5 | The window core: native Metal, and GLFW with Vulkan |
| [render-design.md](render-design.md) | 4, 4.5 | The textured quad and `ck-render-2d` on Metal and Vulkan |
| [scene-design.md](scene-design.md) | 5, 5.5 | Live resources, the ordered scene of quads, input bindings, frame pacing and bindless drawing |
| [map-design.md](map-design.md) | 6, 9 | The map: Ireland, the look, the map data, the GPU-written map and the preview tool; §12 adds the land, climate, look and elevation mode of milestone 9 |
| [places-time-design.md](places-time-design.md) | 7 | Places, catchments, the game clock, reproducible saves, mouse pan and zoom, the readout and hover names |
| [ui-design.md](ui-design.md) | 8 | UI windows and panels: routing between the map and the UI, the widget library, scene groups, windows, lists and the theme |
| [economy-design.md](economy-design.md) | 9 | The first economy slice: pops, buildings, production, eating, demography, prices, land and map modes |

## Records

| Document | Record of |
| --- | --- |
| [evidence/](evidence/) | Dated measurements: the milestone 5.5 scale run, and the CK-168 Vulkan GPU-time investigation |
| [guide/](guide/) | ck-guide's alignment reviews, one per run, and its cursor (`CURSOR.md`) |
| [setup-hardening-plan.md](archive/setup-hardening-plan.md) | The setup-hardening slices, all accepted (completed 2026-10-17) |
| [live-pilot-findings.md](archive/live-pilot-findings.md) | The closed findings ledger of the 2026-10-15 live chat pilot |
| [chat-identity-incident.md](archive/chat-identity-incident.md) | The 2026-10-14 shared chat identity incident |
| [reviews/](archive/reviews/) | Bootstrap and identity-conflict review verdicts (2026-10-14) |

A record is not current guidance where it disagrees with a current document. Fix current documents through `tools/ck docs`, and leave
records as dated history. Each milestone's working files (design revisions, owner directions, issue specs, review verdicts and check logs)
stay in `.pipeline/coordination/`, outside the tracked docs.
