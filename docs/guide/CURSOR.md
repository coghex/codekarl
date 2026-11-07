# Guide cursor

Resume after: `1794069522091622000` · patch `OO7MDP6YPDSBGPIVFCKPZFCBE357L7SO7CHPQYFO2XGM5EQ5UDTAC` · pointer `159843c240d30b11ec3d546a6139c046058fdb76bdbce6ccde3901839899515d` · [20261107t164707z-159843c2](20261107t164707z-159843c2.md) · 2026-11-07T16:51:51Z
Covered beyond the boundary: none

## Next

Nothing is pending after the boundary.

## Open findings

- none

## Pending handoff

- none

## Alignment

| Principle | Reading | Since | Note |
|---|---|---|---|
| vision.defer-engine-systems | aligned | 20261107t154110z-064a459b | M6 map and M7 clock/scenario work follow explicit owner-discussed designs and reviewed effective specs. No economy/combat backlog or multiplayer design inferred. |
| vision.early-lua | aligned | 20261107t164707z-159843c2 | Changed from drifting to aligned: CK-203 fixes the omitted environment of textual load while retaining explicit environments and text-only authority. Prior GUIDE-1 resolved. |
| vision.hourly-time | aligned | 20261107t154110z-064a459b | Clock, scheduler and command boundaries implement hourly simulation with coarser cadences and no skipped ticks; controls and save/load are owned separately from rendering. |
| vision.ireland-first | aligned | 20261107t154110z-064a459b | Shipped terrain and the 94 authored places of Ireland-768 implement the pilot-region slice; uncertainty remains explicit in source notes. |
| vision.logging | aligned | 20261030t192312z-8f447d70 | CK-104 retains the writer-thread failure boundary and bounded exit. No new logging architecture assessed. |
| vision.modding | aligned | 20261107t164707z-159843c2 | Changed from drifting to aligned: CK-203 restores ordinary script composition and CK-204 restores prepared crop-edge overlays in the CPU oracle. Prior GUIDE-1 and GUIDE-3 resolved; no new mod trust or loading architecture. |
| vision.reproducible-saves | aligned | 20261107t164707z-159843c2 | Changed from drifting to aligned: CK-203 chooses unsupported-key errors from the key set, independent of native traversal order, without invoking key tostring methods. Prior GUIDE-2 resolved; not a proof of complete gameplay determinism. |
| vision.responsiveness | aligned | 20261107t164707z-159843c2 | Remains aligned with bounded evidence: CK-205 restores generation-scoped palette uploads. The old at-most-1-KB-per-frame contract deviation is resolved, with no speedup measured. Earlier scale/platform qualification limits remain. |
| vision.runtime-first | aligned | 20261107t164707z-159843c2 | Remains aligned: CK-205 admits palette resources before resident-state mutation and releases retired resources under the existing completion proof; allocation refusal preserves old ownership. No new shutdown execution evidence. |
| vision.small-libraries | aligned | 20261107t164707z-159843c2 | Remains aligned: CK-205 keeps palette ownership inside the native Metal owner and CK-204 keeps the CPU reference backend-neutral. Existing hidden test observation is extended; no new dependency or engine telemetry mechanism. |
