# Streaming HUD Truth Single Chain V19 PLAN

Updated: 2026-05-09

## 计划前校对

- Current repo is `C:\Users\sai\ainput` on `home-windows`, branch `main`.
- Current latest commit observed: `d523b86 Enable streaming AI HUD rewrite v16`.
- Existing dirty work from preview.52 and tray v17 must be preserved, not reverted.
- Old architecture entry points found by code search include `prepare_final_streaming_commit`, `transcribe_streaming_offline_final`, `resolve_final_streaming_commit`, `StreamingFinalHudCommitRequest`, `offline_final_*`, `rewrite_tail`, and `stream: false`.
- User requirement has changed from patching duplicate merge bugs to replacing the architecture. This plan must delete old production behavior instead of repairing it again.
- AI rewrite is moved out of V19 current scope and into roadmap/future work.

## 承诺到实现映射

| Commitment | Implementation phase |
| --- | --- |
| HUD opens immediately on press | Phase 2 |
| ASR-facing stream is mono/16k | Phase 1 |
| Release stops capture but keeps HUD visible | Phase 2 |
| Drain before paste | Phase 2 |
| HUD truth is final paste source | Phase 4 |
| Old offline final and merge path removed | Phase 3 |
| One paste per hold | Phase 4 and Phase 6 |
| AI rewrite cannot mutate V19 HUD/paste | Phase 5 |
| AI rewrite future design documented | Phase 5 |
| Audit/log evidence proves behavior | Phase 6 and Phase 7 |

## Phase 0 - Freeze Truth And Preserve Current Worktree

1. Record current branch, commit, dirty files, running exe path, and PID.
2. Keep existing preview.52 work and `specs/tray-sticky-error-v17/` untouched.
3. Treat this v19 task as a full architecture replacement, not an extension of the v18 offline-tail bugfix.
4. Add a visible implementation marker in logs so sessions can be identified as `hud_truth_single_chain_v19`.

Exit criteria: current dirty state documented; rollback path to preview.52 known; no unrelated dirty file reverted.

## Phase 1 - Audio Capture: Mono/16k At The Earliest Possible Point

1. Inspect the recording backend and Windows device negotiation path.
2. Attempt direct mono/16k capture configuration.
3. Add startup/session logging for actual capture format.
4. If direct mono/16k is not accepted by the backend/device, keep device-native capture but move per-chunk conversion to the boundary before ASR ingestion.
5. Add tests for chunk normalization and metadata reporting.

Exit criteria: ASR-facing stream is always mono/16k; logs state direct capture or per-chunk normalization; no release-time full-audio conversion is part of final commit.

## Phase 2 - Replace The Streaming State Machine

1. Introduce explicit session states: `Idle`, `HoldingListening`, `ReleasedDraining`, `Committing`.
2. On hotkey down, emit HUD-open with empty truth text before audio is required.
3. During hold, feed ASR with streaming chunks and update `StreamingHudTruthState`.
4. On hotkey up, stop capture only; do not paste and do not close HUD.
5. Drain queued audio, resampler tail, ASR partial/final output, and HUD update queue.
6. When drain is stable, freeze one HUD truth snapshot and enter commit.

Exit criteria: release no longer calls old final commit preparation; HUD remains visible during drain; state transition tests cover silence, short speech, long speech, quick tap, and release during pending ASR.

## Phase 3 - Delete Old Final-Merge Production Path

1. Remove production calls to `prepare_final_streaming_commit`.
2. Remove production calls to `transcribe_streaming_offline_final`.
3. Remove production calls to `resolve_final_streaming_commit`.
4. Remove old final candidate merge behavior from commit decisions.
5. Remove or isolate old offline-final helper tests so they cannot pass while production still uses the old path.
6. Keep old functions only temporarily if needed for replay comparison, behind test-only or explicitly dead/debug code. They must not be reachable from normal streaming mode.

Exit criteria: code search proves streaming release commit has no production call path to offline final; no `offline_final_*` value can influence pasted text; regression tests fail if old merge path is reintroduced.

## Phase 4 - Make HUD Truth The Commit Source

1. Add/rename a canonical `StreamingHudTruthState` struct.
2. All ASR display updates write through that state.
3. Overlay rendering reads the same state or receives exact snapshots from it.
4. Commit freezes `hud_truth_snapshot` only after drain complete.
5. UI ack/readback must match the snapshot before paste.
6. DirectPaste receives the snapshot text exactly once.
7. After commit starts, reject or count any mutation attempt as a test failure.

Exit criteria: `hud_truth_snapshot == ui_display_text == pasted_text`; one session creates no more than one paste request; HUD closes only after successful commit or safe empty-session close.

## Phase 5 - Quarantine AI Rewrite And Move It To Roadmap

1. Do not implement streaming rewrite, cleanup rewrite, backend probing, or rewrite model routing in V19.
2. Identify existing streaming AI rewrite hooks that can mutate HUD display or final text.
3. Disable, bypass, or guard those hooks for the V19 streaming path.
4. Add tests/log assertions proving AI rewrite does not mutate V19 HUD truth or final paste.
5. Keep a future roadmap note with three valid product modes: manual command rewrite, explicit optional post-drain cleanup, and realtime duplex rewrite using WebSocket/WebRTC-style live transport.
6. Leave future rewrite to a separate spec after V19 is stable.

Exit criteria: V19 has no AI rewrite implementation requirement; existing AI rewrite cannot affect V19 HUD truth or final paste; future rewrite requirements are documented but not included in V19 acceptance.

## Phase 6 - Update Tests, Fixtures, Benchmarks, And Reports

1. Update unit tests around worker state transitions.
2. Update replay fixtures to remove offline-final success dependence.
3. Add HUD/paste equality fields to reports.
4. Update latency benchmark output: track first HUD partial, release-to-drain, drain-to-paste, and HUD/paste equality.
5. Add exact regression replay for the user's preview.51 duplicate failure.
6. Add grep/audit test that fails if old offline-final production calls return.
7. Add guard test that fails if AI rewrite mutates V19 HUD truth or final paste.

Exit criteria: `cargo test -p ainput-desktop` passes; raw replay corpus passes under no-offline-final mode; scripts no longer present offline final latency as a success path.

## Phase 7 - Package Preview And Launch On The Real Windows Desktop

1. Bump/package next preview version, expected `1.0.0-preview.53`.
2. Create `dist\ainput-1.0.0-preview.53` and zip.
3. Stop existing `ainput-desktop.exe` process.
4. Launch new exe in interactive Windows session.
5. Verify startup idle state.
6. Run live hold/release tests and inspect logs.
7. Report exe path, PID, version, and rollback path.

Exit criteria: new preview is running on `home-windows`; preview.52 remains available for rollback; live evidence proves one paste, HUD truth equality, release drain, and no offline final invocation.

## Engineering Diagram

```text
NEW V19 VOICE PATH

CtrlDown
  -> HUD.open(blank)
  -> mic capture
  -> direct mono/16k if possible, otherwise chunk-normalize to mono/16k
  -> Streaming ASR
  -> HUD truth state
CtrlUp
  -> stop capture, keep HUD visible
  -> drain queued audio + ASR output into HUD truth state
  -> freeze HUD truth snapshot
  -> paste snapshot once
  -> close HUD
```

```text
DELETED OLD PATH

CtrlUp
  X--> full/tail offline final ASR pass
  X--> online/offline final candidate competition
  X--> HUD text + offline final tail merge
  X--> release-time final correction after HUD display
  X--> paste text that differs from HUD
```

```text
ROADMAP AFTER V19: AI REWRITE OPTIONS

Option A: manual command rewrite on existing text.
Option B: explicit optional post-drain cleanup, only after product decision.
Option C: realtime duplex rewrite with WebSocket/WebRTC-style live transport; not ordinary one-shot HTTP.
```

## Implementation Order

1. State machine and HUD truth first.
2. Remove old final/offline merge path second.
3. Audio mono/16k direct-or-earliest-normalization third.
4. Quarantine AI rewrite from V19 fourth.
5. Reports/tests/audit fifth.
6. Package/live launch last.

Reason: duplicate-send correctness depends first on eliminating the second final path and making HUD truth the only commit source. AI rewrite is explicitly deferred so it cannot delay or destabilize V19.

## Known Technical Risk

The main V19 risk is stale legacy code still mutating HUD/final text after the new HUD truth snapshot exists. The implementation must gate every mutation path after freeze and explicitly prove that AI rewrite is not part of V19 commit behavior.
