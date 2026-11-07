# Guide cursor

Resume after: `D1794066013299252000` · patch `SR7QEBJOJY7SJRKUW7EJ5LT3SSX3OF5A3MPXI5SQGFBBN7WCRHCAC` · pointer `064a459b05299902ad0b18b947a11df22aed93356c2d790edb2f4b4b60e8b280` · [20261107t154110z-064a459b](20261107t154110z-064a459b.md) · 2026-11-07T16:00:41Z
Covered beyond the boundary: none

## Next

Review the 1 publication(s) after the boundary.

## Open findings

- none

## Pending handoff

- publication D1794067250011122000 is after the boundary

## Alignment

| Principle | Reading | Since | Note |
|---|---|---|---|
| vision.defer-engine-systems | aligned | 20261107t154110z-064a459b | M6 map and M7 clock/scenario work follow explicit owner-discussed designs and reviewed effective specs. No economy/combat backlog or multiplayer design inferred. |
| vision.early-lua | drifting | 20261107t154110z-064a459b | GUIDE-1: ordinary textual load loses its default environment. Gameplay VM integration otherwise follows the accepted M7 scope. |
| vision.hourly-time | aligned | 20261107t154110z-064a459b | Clock, scheduler and command boundaries implement hourly simulation with coarser cadences and no skipped ticks; controls and save/load are owned separately from rendering. |
| vision.ireland-first | aligned | 20261107t154110z-064a459b | Shipped terrain and the 94 authored places of Ireland-768 implement the pilot-region slice; uncertainty remains explicit in source notes. |
| vision.logging | aligned | 20261030t192312z-8f447d70 | CK-104 retains the writer-thread failure boundary and bounded exit. No new logging architecture assessed. |
| vision.modding | drifting | 20261107t154110z-064a459b | GUIDE-1 and GUIDE-3: normal script composition and valid crop-edge scenario previews have contract defects; authored data remains separate from GPU owners. |
| vision.reproducible-saves | drifting | 20261107t154110z-064a459b | GUIDE-2: unsupported-key error text and metamethod selection remain allocation-order dependent. Save framing, stream isolation and continuation are otherwise aligned at the reviewed boundaries. |
| vision.responsiveness | aligned | 20261107t154110z-064a459b | Changed from drifting: CK-168 identifies a heap-sensitive MoltenVK penalty, CK-175 adopts the owner-selected mitigation and CK-201 reduces default table cost. This is directional alignment with bounded evidence, not a native-Linux or all-scale performance qualification. GUIDE-4 is an unmeasured, at-most-1-KB-per-frame contract deviation. |
| vision.runtime-first | aligned | 20261107t154110z-064a459b | Map resources join completion-proof ownership; simulation shutdown and Lua exit preserve the application lifecycle at reviewed boundaries. Source review is not new shutdown execution evidence. |
| vision.small-libraries | aligned | 20261107t154110z-064a459b | Map and simulation stay windowless/backend-neutral; GPU ownership remains native. GUIDE-4 concerns the palette update cadence inside the Metal owner, not a new dependency boundary. |
