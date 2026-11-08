# Guide cursor

Resume after: `1794160471830792000` · patch `RZ56NB6BSJ2T22JI535BTB7P3BRRNH7IF5PRGSH66EBBY7OV4WAAC` · pointer `7c390c251d1c82853a782770381719a2a51679def663e68fc5385c75c7385f32` · [20261108t190620z-7c390c25](20261108t190620z-7c390c25.md) · 2026-11-08T19:15:40Z
Covered beyond the boundary: none

## Next

Review the 1 publication(s) after the boundary.

## Open findings

- none

## Pending handoff

- publication D1794165347136915000 is after the boundary

## Alignment

| Principle | Reading | Since | Note |
|---|---|---|---|
| vision.defer-engine-systems | aligned | 20261107t154110z-064a459b | M6 map and M7 clock/scenario work follow explicit owner-discussed designs and reviewed effective specs. No economy/combat backlog or multiplayer design inferred. |
| vision.early-lua | aligned | 20261108t190620z-7c390c25 | Aligned: UI composition now lives in rooted Lua modules; deterministic gameplay ignores the rooted option. GUIDE-2 is a local widget ownership defect, not a change of scripting direction. |
| vision.hourly-time | aligned | 20261107t154110z-064a459b | Clock, scheduler and command boundaries implement hourly simulation with coarser cadences and no skipped ticks; controls and save/load are owned separately from rendering. |
| vision.ireland-first | aligned | 20261107t154110z-064a459b | Shipped terrain and the 94 authored places of Ireland-768 implement the pilot-region slice; uncertainty remains explicit in source notes. |
| vision.logging | aligned | 20261030t192312z-8f447d70 | CK-104 retains the writer-thread failure boundary and bounded exit. No new logging architecture assessed. |
| vision.modding | aligned | 20261108t190620z-7c390c25 | Aligned direction: theme data, Lua widgets and one UI scale remain replaceable without an engine framework. GUIDE-2 needs a bounded invalid-input repair; GUIDE-1 needs an explicit checker policy. No new trust/distribution promise. |
| vision.reproducible-saves | aligned | 20261107t164707z-159843c2 | Changed from drifting to aligned: CK-203 chooses unsupported-key errors from the key set, independent of native traversal order, without invoking key tostring methods. Prior GUIDE-2 resolved; not a proof of complete gameplay determinism. |
| vision.responsiveness | aligned | 20261108t190620z-7c390c25 | Aligned: independent input ticks, bounded UI stash, latest-wins selection and viewport-only list rows implement separation. No timing or scalability measurement in this run; existing qualification limits remain. |
| vision.runtime-first | aligned | 20261108t190620z-7c390c25 | Aligned: close-answer sequencing, capture cancellation and group retirement have explicit ownership. No fresh shutdown execution; GUIDE-2 identifies one widget-level ownership hole. |
| vision.single-player-time | aligned | 20261108t190620z-7c390c25 | M8 exposes existing pause/speed controls through the same channel as keys; controls do not become journalled simulation commands. This does not review future order-while-paused gameplay. |
| vision.small-libraries | aligned | 20261108t190620z-7c390c25 | Aligned: scene groups and tint stay backend-neutral, shader records stay backend-owned, and widgets remain Lua modules without a new framework. Local lifecycle defect GUIDE-2 does not establish architectural drift. |
| vision.ui-heavy | aligned | 20261108t190620z-7c390c25 | New mapping: M8 delivers windows, panels, a virtualized places list/dropdown and time controls matching the final owner choices. GUIDE-1/2 are bounded follow-ups, not a reversal of that direction. |
