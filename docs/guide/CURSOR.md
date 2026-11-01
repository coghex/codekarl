# Guide cursor

Resume after: `D1793539232205556000` · patch `R24GKJ32L53MKJGDWZZLXTUVAZDXLEZAXCFEU3WRQ2JGNPKECAWQC` · pointer `4873df4d981159b1b38d929b190e3cf49416879a2a4d0a93a2cc404e977cc06d` · [20261101t132214z-4873df4d](20261101t132214z-4873df4d.md) · 2026-11-01T13:39:01Z
Covered beyond the boundary: none

## Next

Process the open findings below, then continue from the boundary.

## Open findings

- 20261101t132214z-4873df4d/GUIDE-1 — Wait for the Vulkan first-frame report before asserting it — unprocessed

## Pending handoff

- none

## Alignment

| Principle | Reading | Since | Note |
|---|---|---|---|
| vision.defer-engine-systems | aligned | 20261101t132214z-4873df4d | Owner-discussed M5/M5.5 work follows the scene contract. No ECS/map/game mechanics or Linux qualification inferred; stale Open engineering text is superseded only by explicit later owner-directed milestones. |
| vision.early-lua | aligned | 20261101t132214z-4873df4d | The same resource/scene/frame script now composes on glfw-vulkan; updates and metrics remain separate from GPU ownership. |
| vision.logging | aligned | 20261030t192312z-8f447d70 | CK-104 retains the writer-thread failure boundary and bounded exit. No new logging architecture assessed. |
| vision.responsiveness | drifting | 20261101t132214z-4873df4d | New scale evidence: bindless Vulkan/MoltenVK GPU p95 157.5ms versus Metal 0.625ms, approximately 6 fps at scale. Owner authorized investigation CK-168; acceptance is correctness-only and does not resolve this goal. No duplicate finding. |
| vision.runtime-first | aligned | 20261101t132214z-4873df4d | Proof-before-release and explicit lifecycle remain intact across Vulkan composition; prior M5 repairs are present. GUIDE-1 concerns reliable evidence of the first-frame event, not a changed lifecycle design. |
| vision.small-libraries | aligned | 20261101t132214z-4873df4d | CPU resources and resident policy remain shared; native Vulkan owners stay in gpu-vulkan. CK-162 explicitly accepts temporary metrics/phase duplication and names later consolidation. |
