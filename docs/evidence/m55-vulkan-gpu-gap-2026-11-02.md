# CK-168: Vulkan GPU-time investigation (review revision 2)

**Status:** Matrix complete. A process-local MoltenVK heap-allocation diagnostic reduced the 32,768-texture GPU p50 from about 157.5 ms to 0.75 ms, strongly implicating MoltenVK's Metal resource-allocation path. The independent Metal trace still could not be exported, so timestamp attribution remains unverified. No product code was changed.

**Record status (updated 2026-11-10):** this is revision 2, published unchanged (SHA-256 `437a0fc8…6c3097`) after ck-reviewer-2 approved it in findings review 2 (`.pipeline/coordination/runtime-m55/ck168-findings-review-2-reviewer-2.md`). That review's two non-blocking provenance notes were not applied: N6, the measurement driver was edited at 02:09:37Z, after blocks A and B, and earlier versions were not kept, so the first trace capture did run through a driver; and N7, the export error quoted for the first capture has no retained record. Neither changes a measurement or a conclusion. Later, CK-175 made the Vulkan instance ask MoltenVK to allocate from Metal heaps, unless `MVK_CONFIG_USE_MTLHEAP` is set in the environment.

## Scope and approved measurement changes

The accepted CK-168 r3 specification asked whether the high GPU time follows live/resident textures, sampled textures, quad count, table capacity, or draw mode. It also requires external-tool validation of the GPU timestamps and an explicit Linux assessment.

Replication followed the manager's relayed owner decision of 2026-11-02: “replace r3 point 1 replication with ONE run per cell (14 runs). Repeat a cell … only where its result is close/ambiguous or decides the cause.” A cell with an earlier run already counted. The A/per-revision/256 cell was repeated once because it informs the cause comparison; its p50s were close (4.45 and 4.55 ms). The A/bindless/32,768 cell has three runs: the third was already in progress when the decision arrived. No fourth run was made. Its capture duration is fixed in the accepted harness and has no timing override, so a longer run would have required a source change and was not attempted.

The second owner decision, also relayed on 2026-11-02, replaced CK-130's load wait: “the owner keeps other work running until tomorrow … Drop the CK-130 load wait: start each run immediately and record load1 at start and end.” External GHC/moskophoros builds continued. The GPU timestamps below, rather than wall time, are the intended comparison. The first two A/1-texture runs predated that change and do not have exact per-run load boundary samples; their original capped-start values were 16.73 and 23.06, respectively.

## Machine and command

- macOS 26.7.1 (25G313), Apple M3 Max, Xcode 27.2 (27B52), Rust/Cargo 1.91.1.
- Vulkan 1.2.296 device on MoltenVK 1.2.11; loader Vulkan 1.3.296. Release profile, validation layer not loaded, MoltenVK configuration unset.
- `vulkaninfo --summary` output, complete tool/version metadata, selected environment, and accepted worker-copy status are retained in `.pipeline/coordination/runtime-m55/ck168-measurements-20261102/machine-and-protocol.json`.
- Each per-run `command.txt`, `environment`, load boundary sample, raw harness JSON/stdout/stderr, and file hashes are retained under `.pipeline/coordination/runtime-m55/ck168-measurements-20261102/{A,B,C,D}/<cell>/<replicate>/`. The command template was `CK_SCALE_RUN=1 CK_SCALE_ONLY=diagnostic CK_SCALE_DIAGNOSTIC_CELL=<cell> CK_SCALE_OUT=<per-run evidence directory> ./tools/ck cargo test --release -p ck-render-2d --test vulkan_scale --locked --features vulkan -- --nocapture`.
- The measurement driver is `.pipeline/coordination/runtime-m55/ck168-drivers/ck168_measurement_driver.py`, SHA-256 `c7668798ebeb3f9c0a442797cf72c92fdfa8d5cc7c0e6d273fabe607a3c215ab`. It changes no product code.

## Matrix results

GPU and CPU encode values are p50/p95 in milliseconds. `n` is the harness sample count. A `—` p95 means the harness returned an out-of-range percentile; it is not estimated. Load is load1 at run start → end. Repeated measurements list the observed range/counts; p50 and p95 show their median when available. A 65,536-quad C cell is shared with A/bindless/32,768 and is not a second run.

| Cell | Mode; live / sampled textures; quads; table slots | GPU n | GPU p50 / p95 | CPU encode p50 / p95 | Load1 start → end | Harness |
|---|---|---:|---:|---:|---:|---|
| A bindless 1 | bindless; 1 / 1; 65,536; 131,072 | 1,800 | 0.175 / 0.625 | 2.800 / 2.900 | 16.73 / end unavailable | pass |
| A per-revision 1 | per-revision; 1 / 1; 65,536; n/a | 1,800 | 0.275 / 0.575 | 0.175 / 0.225 | 23.06 / end unavailable | pass |
| A bindless 256 | bindless; 256 / 256; 65,536; 131,072 | 1,733 | 0.625 / 0.650 | 2.925 / 13.750 | 6.61 → 6.35 | pass |
| A per-revision 256 | per-revision; 256 / 256; 65,536; n/a | 1,065; 1,105 | median 4.500 (4.450–4.550) / 6.150 (6.100–6.200) | median 17.875 (17.750–18.000) / 43.750 (42.500–45.000) | 6.35 → 5.56; 4.47 → 4.32 | pass, 2 runs |
| A bindless 4,096 | bindless; 4,096 / 4,096; 65,536; 131,072 | 1,696 | 3.350 / 6.475 | 3.550 / 10.000 | 4.30 → 6.05 | pass |
| A per-revision 4,096 | per-revision; 4,096 / 4,096; 65,536; n/a | 803 | 9.750 / 14.000 | 20.500 / 55.000 | 6.05 → 7.44 | pass |
| A bindless 32,768 | bindless; 32,768 / 32,768; 65,536; 131,072 | 98; 121; 118 | median 157.500 (157.500–160.000) / — | median 8.750 (8.500–8.750) / median 67.500 (60.000–100.000) | 7.44 → 9.97; 10.23 → 8.98; 7.81 → 6.74 | 2 pass; rep1 teardown unproven |
| A per-revision 32,768 | per-revision; 32,768 / 32,768; 65,536; n/a | 109 | 167.500 / — | 26.750 / 117.500 | 12.57 → 11.40 | pass |
| B bindless 1 sampled | bindless; 32,768 / 1; 65,536; 131,072 | 112 | 157.500 / — | 8.500 / 127.500 | 13.41 → 10.33 | teardown unproven |
| B bindless 256 sampled | bindless; 32,768 / 256; 65,536; 131,072 | 128 | 160.000 / — | 8.500 / 38.500 | 10.33 → 10.07 | pass |
| C bindless 32,768 quads | bindless; 32,768 / 32,768; 32,768; 131,072 | 98 | 160.000 / — | 9.000 / 36.750 | 9.10 → 14.70 | teardown unproven |
| C bindless 65,536 quads | bindless; 32,768 / 32,768; 65,536; 131,072 | shared with A | shared with A | shared with A | shared with A | shared with A |
| D bindless minimum table | bindless; 32,768 / 32,768; 65,536; 33,794 | 117 | 160.000 / — | 6.325 / 57.500 | 13.68 → 13.11 | pass |
| D bindless large table | bindless; 32,768 / 32,768; 65,536; 131,072 | 119 | 157.500 / — | 8.750 / 57.500 | 13.11 → 10.43 | pass |

The missing p95 values are caused by GPU histogram overflow, not a sample-count floor. Each unavailable percentile is out of the histogram's representable range. These are low-sample runs: 30-second windows of slow frames, not short captures. The overflows and observed GPU maxima show the heavy tail that the p50 alone hides:

| Run | GPU overflow | GPU maximum |
|---|---:|---:|
| A/bindless/32,768 rep1 | 7/98 | 2.46 s |
| A/bindless/32,768 rep2 | 29/121 | 0.61 s |
| A/bindless/32,768 rep3 | 29/118 | 0.55 s |
| A/per-revision/32,768 | 7/109 | 3.48 s |
| B/1 sampled | 15/112 | 1.27 s |
| B/256 sampled | 31/128 | 0.88 s |
| C/32,768 quads | 6/98 | 3.20 s |
| D/33,794 slots | 25/117 | 0.79 s |
| D/131,072 slots | 24/119 | 0.67 s |
| Diagnostic 01, argument buffers disabled | 10/111 | 0.96 s |
| Diagnostic 02, performance tracking | 40/132 | 0.33 s |

The MTLHEAP diagnostic had 1,405 samples and no GPU histogram overflow; the harness still labels the sample count too few for its nominal `MIN_GPU_SAMPLES` threshold. The values above are the histogram's recorded overflow counts and maximum GPU times, not estimates of the unreported p95.

## Teardown-unproven runs

These runs have a usable window reading and `correctness.passed: true`, but the full harness did not pass: the worker/presenter did not reach the required observed-closed state before teardown. They are included as p50 evidence only, not as harness-pass runs. Each raw `results.json` also records `loop_end: Unproven { reason: "the presenter was not observed finished before the teardown deadline" }` and the result problem `the worker exited with ExitStatus(unix_wait_status(256))`.

1. A/bindless/32,768 rep1: `.pipeline/coordination/runtime-m55/ck168-measurements-20261102/A/a-bindless-32768/rep1/cargo.log`; stdout detail: `.../rep1/harness-evidence/diagnostic.stdout.txt`. The harness printed `diagnostic DID NOT PASS ... (correctness gated; timings reported)` and `the worker exited with ExitStatus(unix_wait_status(256))`.
2. A/bindless/32,768 xctrace diagnostic: `.pipeline/coordination/runtime-m55/ck168-measurements-20261102/A/a-bindless-32768/xctrace-diagnostic/cargo.log`; raw detail: `.../xctrace-diagnostic/harness-evidence/diagnostic.stdout.txt`. Same worker exit and `Unproven` teardown. This extra capture run is not included in the timing-cell summary.
3. B/bindless/1 sampled: `.pipeline/coordination/runtime-m55/ck168-measurements-20261102/B/b-bindless-1/rep1/cargo.log`; raw detail: `.../rep1/harness-evidence/diagnostic.stdout.txt`. Same worker exit and `Unproven` teardown.
4. C/bindless/32,768 quads: `.pipeline/coordination/runtime-m55/ck168-measurements-20261102/C/c-bindless-32768/rep1/cargo.log`; raw detail: `.../rep1/harness-evidence/diagnostic.stdout.txt`. Same worker exit and `Unproven` teardown.

These have been reported to the manager for a separate harness-issue decision. No teardown diagnosis or fix was attempted during the matrix.

## MoltenVK configuration diagnostics

The manager authorized up to four single-run MoltenVK configuration diagnostics at 32,768 live textures, with overrides confined to each measured process. The exact environment, command, load1 boundaries, full harness output, result JSON and hashes are in `.pipeline/coordination/runtime-m55/ck168-measurements-20261102/mvk-config-diagnostics/<run>/`. All three runs used the accepted release `vulkan_scale` harness. No configuration was added to product code.

| Diagnostic | Cell / setting | GPU p50 / p95 | CPU encode p50 / p95 | Samples | Harness and correctness | Load1 start → end |
|---|---|---:|---:|---:|---|---:|
| Argument buffers disabled | A per-revision / 32,768; `MVK_CONFIG_USE_METAL_ARGUMENT_BUFFERS=0` | 170 / unavailable ms | 31 / 155 ms | 111 | Correctness passed; teardown unproven (cargo exit 1) | 11.43 → 12.06 |
| MoltenVK performance tracking | A bindless / 32,768; `MVK_CONFIG_PERFORMANCE_TRACKING=1`, `MVK_CONFIG_ACTIVITY_PERFORMANCE_LOGGING_STYLE=2` | 160 / unavailable ms | 8.5 / 32.75 ms | 132 | Harness and correctness passed | 12.06 → 9.76 |
| Metal heaps enabled | A bindless / 32,768; `MVK_CONFIG_USE_MTLHEAP=1` | **0.75 / 0.75 ms** | 7.925 / 27.75 ms | 1,405 | Harness and correctness passed; correctness saw all 32,768 distinct sampled textures | 7.76 → 5.11 |

The default-config A bindless 32,768 cell's valid runs had median p50 157.5 ms (range 157.5–160 ms); the heap-enabled run's GPU p50 and p95 were both 0.75 ms, with no histogram overflow. That is approximately a 210× reduction at an otherwise identical cell. The heap run had 1,405 GPU samples and a passing teardown, although its count remains below the harness's nominal sample threshold. The external load was present in both the original and diagnostic blocks; the heap-enabled run began at load1 7.76 and ended at 5.11, within the range of the original large-texture runs. Its median presentation interval was 16.75 ms over 1,405 frames, versus 177.5 ms over 121 frames in default-config rep2. Presentation intervals use the harness monotonic clock, independent of GPU timestamps, so this corroborates that the observed change is not limited to GPU timestamp values.

### Point 5: CPU encode scaling

CPU encode does not follow the GPU-time factor. In bindless mode from one to 32,768 live and sampled textures, CPU encode p50 rises from 2.8 ms to 8.75 ms (about 3×), while GPU p50 rises from 0.175 ms to 157.5 ms (about 900×). With `MVK_CONFIG_USE_MTLHEAP=1`, CPU encode remains 7.925 ms while GPU p50 falls about 210×. CPU encode is a separate, smaller scaling effect, and the heap setting does not remove it. Per-revision encode rises from 0.175 ms at one texture to 26.75 ms at 32,768, alongside the increase from one to 65,536 draw calls, so that comparison includes both texture and draw-call changes.

Disabling argument buffers did not produce a large drop: the 170 ms per-revision p50 is close to the existing default-config per-revision 32,768 p50 of 167.5 ms. This argues against Metal argument-buffer use alone explaining the slowdown. The one performance-tracking run remained near the default bindless result (160 ms p50), and MoltenVK emitted no separate activity-statistics summary in its captured output, so that run adds no independent breakdown.

MoltenVK 1.2.11's configuration catalog documents `MVK_CONFIG_USE_MTLHEAP` as an allocation control for textures and buffers, with default `0`; setting it to `1` uses heaps where safe. The same catalog documents `MVK_CONFIG_USE_METAL_ARGUMENT_BUFFERS` (default `1`) for descriptor-set resources and the performance-tracking/logging settings used above. It contains no per-encoder descriptor-resource residency switch, so that suspected behavior could not be toggled directly. Environment configuration is explicitly supported for runtime diagnostics. See the version-pinned [MoltenVK 1.2.11 configuration reference](https://github.com/KhronosGroup/MoltenVK/blob/v1.2.11/Docs/MoltenVK_Configuration_Parameters.md#L192-L208), especially the [performance-tracking settings](https://github.com/KhronosGroup/MoltenVK/blob/v1.2.11/Docs/MoltenVK_Configuration_Parameters.md#L404-L420), [argument-buffer setting](https://github.com/KhronosGroup/MoltenVK/blob/v1.2.11/Docs/MoltenVK_Configuration_Parameters.md#L603-L622), and [MTLHeap setting](https://github.com/KhronosGroup/MoltenVK/blob/v1.2.11/Docs/MoltenVK_Configuration_Parameters.md#L626-L632).

## External trace and attribution

The manager authorized one replacement Metal System Trace capture, with `xctrace --time-limit 10s`, attached after the diagnostic worker had been alive for 35 seconds. The accepted diagnostic uses a 5-second warm-up followed by a 30-second sample, so this placed the capture in the steady frame sample. Exact command, environment, timestamps, raw logs, trace bundle and hashes are retained in `.pipeline/coordination/runtime-m55/ck168-measurements-20261102/point4-replacement/`.

The CLI log confirmed `Time limit: 10.0 s`, but xctrace remained active past its 60-second supervisor wait. The controller interrupted that process; xctrace exited 1 and created only a 52 KiB package containing `RunIssues.storedata`. No export was possible from this replacement capture. No further capture or export attempt was made. The earlier, untimed capture at `A/a-bindless-32768/xctrace-diagnostic/metal-system-trace.trace` is not empty: it occupies 3.1 GiB on disk (about 3.28 GB); an export was attempted and returned “Document Missing Template Error.” No retry was made. The captured Vulkan diagnostic produced a correctness-passing cell reading but its teardown was unproven (cargo exit 1); it is excluded from the configuration comparison. Point 4 therefore remains unresolved: there is still no independent Metal-side duration view of the same Vulkan frames.

The combined evidence now strongly implicates MoltenVK's Metal resource-allocation behavior: the high GPU time tracks 32,768 live textures across sampled-count, quad-count, table-capacity, and draw-mode variations, and enabling MoltenVK's Metal heaps collapses the same bindless cell from about 157.5 ms to 0.75 ms. This is a single diagnostic run, not a repeated result; the external trace failure means the Vulkan timestamp itself has not yet been independently validated. The experiment shows a MoltenVK configuration-sensitive performance mechanism, but does not identify which internal allocation/residency operation accounts for it.

## Linux assessment

No native Linux run was performed. **Prediction:** the same MoltenVK-specific allocation penalty should not carry over unchanged to native Linux Vulkan, because the 210× improvement came from MoltenVK's Metal-specific `MTLHeap` allocation option. That points to macOS translation/resource allocation as the dominant source of this gap, rather than the argument-buffer path or table capacity. However, the matrix still shows that a large live-texture workload can be expensive on this Mac even when few textures are sampled, and a native Linux driver may have its own residency and descriptor costs. The Linux prediction is not a Linux result; measure the same 32,768-live and low-sampled cells on a native Linux Vulkan driver before claiming the workload is unaffected there.

The next attribution step, if needed, is not another xctrace attempt in this bounded task: the one allowed timed capture failed. Point 4 remains unresolved and needs a separately authorized external capture approach. A Linux measurement would directly test the platform prediction. If a repeat confirms the MTLHEAP result, one candidate for the owner to consider is requesting MoltenVK's heap allocation through its configuration interface in the application; that would be a product setting and is not proposed or authorized here. No product fix is proposed in this report.

The authorized GPU/window slot was used for about 68 minutes, from 01:16:24Z through the final diagnostic at 02:24:10Z, within the eight-hour cap.

## Evidence index

- Machine/protocol and full `vulkaninfo --summary`: `.pipeline/coordination/runtime-m55/ck168-measurements-20261102/machine-and-protocol.json`.
- Per-run command, environment, load boundaries, harness outcome and hashes: each cell's `replicate/run.json` plus adjacent `command.txt`, `cargo.log`, and `harness-evidence/` files.
- Failed replacement trace and logs: `.pipeline/coordination/runtime-m55/ck168-measurements-20261102/point4-replacement/` (the earlier failed capture remains under `A/a-bindless-32768/xctrace-diagnostic/`).
- Process-local MoltenVK configuration diagnostics: `.pipeline/coordination/runtime-m55/ck168-measurements-20261102/mvk-config-diagnostics/`; the pinned MoltenVK 1.2.11 configuration catalog is linked above.
- Matrix driver and SHA sidecar: `.pipeline/coordination/runtime-m55/ck168-drivers/ck168_measurement_driver.py` and `ck168_measurement_driver.sha256` (`c7668798…15ab`). That driver did not execute the three MoltenVK diagnostics or either trace capture. The ad hoc controller code for those four runs was not retained, so it has no recoverable source or SHA-256. The diagnostic records contain the driver's SHA due to a reused attribution field; those immutable records were not edited. The recorded commands and environments remain available for reproducing the measurements. The three setting diagnostics were launched with per-process `MVK_CONFIG_*` environment values and `./tools/ck cargo test ...`, while the controller wrote run metadata, load samples and file hashes. For the replacement capture, the controller started the harness, waited 35 seconds after worker identification, launched `xctrace --time-limit 10s` attached to the worker, waited up to 60 seconds, then interrupted xctrace when it did not exit; it saved the command, logs, capture metadata and hashes.
- Earlier 3.1 GiB trace bundle and failed export attempt: `.pipeline/coordination/runtime-m55/ck168-measurements-20261102/A/a-bindless-32768/xctrace-diagnostic/`.
