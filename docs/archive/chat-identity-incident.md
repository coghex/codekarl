# Shared chat identity incident — 2026-10-14

**Historical record.** The repair described here is accepted and installed. Statements below about pending work describe
2026-10-14. Current state (2026-10-17): Codekarl delivery is on as the routine state, the pilot and the review gate are
complete, and the Copilot chat adapter was not built because the pilot used native clients. The Plateia capture and
public-port status belong to Plateia's own records and are not re-verified here.

The Codekarl rollout exposed a shared discovery failure: two live header
sessions were inferred as the singleton `ck-guide`. Registration correctly
refused to replace its live owner, but the uncaught exception stopped the
entire global inventory. Synarchy therefore could not resolve its own identity
when posting. The singleton guard predated Codekarl; cross-project discovery
during this rollout was insufficiently tested.

The accepted repair catches only the new typed ownership conflict during
discovery, leaves the conflicting candidate unbound, preserves the live
incumbent, and continues registering healthy sessions. Successful rows retain
their own duplicate-retirement attribution. Private current diagnostics live
in `~/.local/state/chat/identity-conflicts.json` (mode 0600). Ordinary resolution
still refuses a conflicting caller, including an explicit-account request, and
refreshes discovery before deciding so stale conflicts can recover. Direct
registration still refuses live ownership conflicts; unrelated errors propagate.

## Delivery and evidence

- Author: `ck-worker-1`, Class B, Codex / gpt-6-luna / high, visible session
  `01a13b6e-7c6d-7b41-90e5-8d0c2b967b55`.
- Reviewer: independent Class A, Claude Opus 5.5 / high through Copilot,
  session `9bcdbe98-41c4-4fdf-9a6f-5eb4cc70a523`.
- [First review](reviews/chat-identity-first-review.md): changes requested;
  found stale-diagnostic recovery failure, private fixture names and missing
  foreign-project coverage.
- [Final review](reviews/chat-identity-rereview.md): approved the combined pair,
  with regression evidence that fails against the first capture.
- Submissions: `1791998764113053000-9e90f31d` and
  `1791999056969754000-87526df1`, accepted together.
- Patches: `KUZOBK52QKFLOO3LPNLPXJ2PD5VR4O6TAFIIHITXRPSOPUS5DBOQC` and
  `HY4IGZB5DPP3PXQ3VYZQNPE67UGAPODC7JUJCNXJDQ3PVPFVYWBAC`.
- Passing combined CI receipt:
  `.pipeline/checkpoints/1791999340483684000-9141b718.json`.
  It includes 46 Python tests, Rust formatting/Clippy/tests/build and six
  executable probes, including Metal.
- The final accepted source was staged again: all 239 shared-chat tests passed.
  The checked installer verified source and live baseline hashes before applying.
  Deployment backup: `~/.local/state/chat/codekarl-install/1791999385340664000`.
- The existing bridge was restarted. Normal discovery and posting succeeded
  with the original collision still quarantined. The original guide and
  manager/worker/reviewer bindings remain active; no session was renamed,
  re-roled or closed. The second header session remains unbound.
- Synarchy independently confirmed a normal bound post from its own manager
  session, with no workaround, in #synarchy message
  `8ubpygmzwnvzm2hrpjwd93fa8s`.

## Plateia capture handoff

The original rollout changed shared source without first reconciling Plateia's
source-capture work. This blocked its approved OWNER-2 issue #31, whose owner
subsequently placed it on hold pending a settled tree and a rescope. The
coordinator preserved that boundary. Codekarl routing capture belongs to the
owner's separate later issue; this repair does not activate Plateia or silently
expand #31.

Plateia's manager verified the pre-install bridge backup against its original
drift hash. A private frozen source bundle is available at:

`~/.local/state/chat/codekarl-handoff/20261014-identity-repair/`

Its `manifest.json` SHA-256 is
`b0b3dc7a0440e973b80320cbfdaa0cfd6252677badff64af4725a153c245c6bf`.
It contains exact original-byte snapshots, separate owner-led/Codekarl/repair
deltas, seven final source/test files and hashes, review/CI references and a
state-format inventory. The new generic helper and tests use invented names.
No credentials, configuration file or runtime message database was copied.
The raw existing source still contains private examples and must not be
published wholesale into Plateia's public repository.

The bundle identifies the SQLite inbox and identity diagnostic as new state,
along with changes to existing config/identity/delivery/log formats and the
project-local CLI writer. Plateia must integrate these with its existing
admission and release rules, preserving its own outbox/replay fixes and
configurable identities. It must not replace its bridge with the live file.

Future shared-chat changes should be coordinated with Plateia before deployment,
with a fixed source and explicit capture scope, so parallel capture work is
not invalidated by an unannounced live-source change. This coordinator has
finished its shared edits and handed the settled tree to Plateia's manager.

Plateia confirmed receipt and independently verified the manifest and all seven
live source hashes in #plateia message `9w6dvpcq2749ze3hnyv7h4xq6n`. Its manager
has prepared #31 amendment 2, now under review, using the original pre-change
bridge backup for the owner-led capture. Codekarl routing and the identity repair
remain explicitly reported as uncaptured drift until Plateia's later capture
issue. Their tracking has been reconciled; the public source port is not yet
complete.

## Boundaries at the time (2026-10-14)

Codekarl terminal delivery then stayed paused pending its Copilot adapter, bounded
pilot and mechanical review-eligibility gate. (Since resolved: the native-client pilot ran, the gate is installed, and
delivery is on.) No game/hardening work was started.
The final review records non-blocking diagnostic-error and launcher limitations;
those are not claimed fixed by this incident repair.
