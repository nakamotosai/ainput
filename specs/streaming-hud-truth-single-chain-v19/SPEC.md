# Streaming HUD Truth Single Chain V19 SPEC

Updated: 2026-05-09

## Goal

Replace the current streaming voice architecture with one single-chain design:

`hotkey hold -> mic stream -> ASR stream -> HUD truth -> release drain -> paste HUD text once -> close HUD`

The old mode is not preserved as a production fallback. V19 removes the release-time offline final pass, final candidate competition, HUD/offline-tail merge, and release-time final correction that can make pasted text differ from what the HUD showed.

AI rewrite is not part of V19 delivery. It is moved to Roadmap/Future Work so V19 can first make the voice input main chain stable.

## 上游真相源

- User contract from 2026-05-09: old mode is rejected; only the new HUD-truth single-chain mode is acceptable.
- Runtime project: `C:\Users\sai\ainput` on `home-windows`.
- Current rollback version: `C:\Users\sai\ainput\dist\ainput-1.0.0-preview.52\ainput-desktop.exe`.
- Current implementation surfaces discovered by code search: `worker.rs`, `main.rs`, `streaming_fixtures.rs`, `crates/ainput-shell/src/lib.rs`, `run-streaming-latency-benchmark.ps1`, and `ai_rewrite.rs` only as a path to quarantine from V19 main-chain behavior.
- Known regression evidence: `dist/ainput-1.0.0-preview.51/logs/streaming-raw-captures/streaming-raw-1778293694109.wav`, with duplicated phrase pattern `失败案例来实失败的案例来实测`.

## User Contract

- Pressing the voice hotkey opens the HUD immediately, even when there is no speech yet. A blank HUD/small empty panel is valid.
- While the hotkey is held, microphone audio is streamed into ASR and the HUD updates live.
- The HUD text is the session truth source. The final pasted text must be byte-for-byte equal to the final HUD truth snapshot.
- Releasing the hotkey stops accepting new microphone input, but does not close the HUD and does not paste immediately.
- After release, the worker drains already captured/queued audio and pending ASR output until recognition is complete.
- When the drain is complete, the app pastes the current HUD truth exactly once and then closes the HUD.
- No offline final ASR pass is allowed after release.
- No final merge between HUD text and another final candidate is allowed.
- No release-time correction is allowed to change the text after the HUD truth has been established.
- AI rewrite must not be implemented or expanded inside V19. Existing AI rewrite code must be disabled, bypassed, or quarantined for this V19 streaming path if it can mutate HUD truth or final paste.

## Non-goals

- Do not keep the old release-final/offline-final pipeline as the default or as an automatic fallback.
- Do not add a second full-audio/tail-audio ASR pass.
- Do not keep `offline_final_*` as a production decision input.
- Do not paste a worker-private final string that differs from the HUD.
- Do not implement AI rewrite, AI cleanup, rewrite streaming, or rewrite backend probing in V19.
- Do not change provider/model families for AI rewrite in V19.
- Do not change the direct paste/clipboard timing behavior except where needed to paste exactly once after drain.

## 设计承诺清单

1. 承诺: HUD opens on hotkey down before speech/audio is required.
2. 承诺: the ASR-facing stream is mono/16k, preferably captured directly and otherwise normalized per chunk before ASR.
3. 承诺: release stops new mic input only; release does not close HUD and does not paste.
4. 承诺: release enters drain, and drain completion is the only path to final paste.
5. 承诺: final pasted text comes from the HUD truth snapshot only.
6. 承诺: no production streaming commit path may call offline final, tail final, final merge, or release-time correction.
7. 承诺: one hold session creates at most one paste request.
8. 承诺: AI rewrite is out of V19 scope and cannot mutate HUD truth or final paste in V19.
9. 承诺: V19 tests and logs prove the old second-pass/final-merge path was not invoked.
10. 承诺: AI rewrite is documented as future work with its own future spec, not hidden inside V19.

## Current Code Surfaces To Retire Or Replace

- `apps/ainput-desktop/src/worker.rs`: retire production use of `prepare_final_streaming_commit`, `transcribe_streaming_offline_final`, `streaming_offline_final_scope`, `streaming_offline_final_sample_start`, and `resolve_final_streaming_commit`; remove final decision dependence on `final_offline_raw_text`, `offline_final_elapsed_ms`, and `offline_final_timed_out`.
- `apps/ainput-desktop/src/main.rs`: keep HUD display/ack plumbing only if it acknowledges the exact HUD truth snapshot; stop treating HUD as a preview of a separate final string.
- `apps/ainput-desktop/src/streaming_fixtures.rs`: remove or neutralize offline-final report fields; add `hud_truth_text`, `pasted_text`, and equality checks.
- `crates/ainput-shell/src/lib.rs`: remove or deprecate config knobs that only exist for old offline final behavior.
- `scripts/run-streaming-latency-benchmark.ps1`: remove offline-final latency as a success metric; add drain latency and HUD/paste equality metrics.
- `apps/ainput-desktop/src/ai_rewrite.rs`: no V19 rewrite implementation; ensure existing rewrite hooks cannot mutate V19 HUD truth or final paste; leave future rewrite design to a separate Roadmap spec.

## Audio Contract

Target ASR input is mono 16 kHz.

1. Try to open/configure the Windows capture backend directly as mono/16k.
2. Log and verify actual device format at session start: input channels, input sample rate, ASR sample rate, and whether direct mono/16k was achieved.
3. If Windows/WASAPI exposes only device-native format, capture device-native but immediately convert each chunk to mono/16k before it enters ASR. This fallback is acceptable only if the ASR-facing stream is mono/16k and there is still no release-time second pass.

The user-facing architecture remains single-chain in both cases: ASR sees one streaming mono/16k stream, not a later re-read of recorded audio.

## New Session State Machine

```text
Idle
  CtrlDown
  -> open HUD immediately with empty truth text
  -> start capture
  -> start ASR stream
  -> HoldingListening

HoldingListening
  mic chunk
  -> normalize chunk to ASR mono/16k if needed
  -> feed ASR
  -> update HUD truth from ASR partial/final stream

HoldingListening
  CtrlUp
  -> stop accepting new mic input
  -> close/flush capture stream
  -> ReleasedDraining

ReleasedDraining
  pending audio / pending ASR output
  -> continue ASR drain
  -> continue HUD truth updates
  -> do not call offline final
  -> do not close HUD

ReleasedDraining
  ASR drained and HUD truth stable
  -> snapshot HUD truth
  -> Committing

Committing
  paste exact HUD truth once
  -> close HUD
  -> Idle
```

## HUD Truth Rules

- The worker owns a canonical `StreamingHudTruthState` for the active session.
- The overlay displays that state; it is not a separate source of truth.
- Final commit uses a snapshot of the same state after drain completes.
- A commit is rejected if UI readback/ack says the HUD display does not equal the snapshot.
- No partial, punctuation pass, timer, or legacy AI rewrite callback may mutate HUD truth after the commit snapshot is frozen.
- Empty or silence session behavior must be explicit: HUD opens on press; if no recognized text after drain, close without paste or paste nothing according to existing user-safe behavior, but never run offline final.

## AI Rewrite Roadmap / Future Work

AI rewrite is intentionally postponed until after V19 is stable.

Future rewrite must get a separate spec and choose one explicit product mode:

1. Manual command rewrite: user invokes rewrite on already visible/selected text; one snapshot request; not part of ASR commit.
2. Optional post-drain cleanup: ASR drain completes first, then one cleanup request may update HUD before paste only if the user explicitly accepts that product behavior.
3. Realtime duplex rewrite: one hold maps to one WebSocket/WebRTC-style live session that can receive text deltas and stream rewrite deltas. This is not ordinary one-shot HTTP.

Future rewrite must not be hidden as repeated HTTP requests under the label "one request". If the backend is only server-output SSE streaming, it cannot continuously consume future HUD text in the same request.

## Logging And Evidence

Each V19 voice session must log session id, state transitions, actual capture format and ASR-facing format, HUD opened timestamp, first ASR partial timestamp, hotkey release timestamp, drain start/end timestamps, final HUD truth snapshot hash/length, pasted text hash/length and equality with HUD truth, proof that offline final path was not invoked, and proof that AI rewrite did not mutate V19 HUD truth or final paste.

## Acceptance

- Pressing hotkey emits HUD-open before any audio chunk is required.
- Release transitions to `ReleasedDraining`, not immediate paste/close.
- Drain completion is the only path to commit.
- No test path calls offline final during streaming commit.
- Final pasted text equals HUD truth exactly.
- Each session has at most one output commit request.
- The previous duplicate sentence regression passes without any offline-final repair, using `dist/ainput-1.0.0-preview.51/logs/streaming-raw-captures/streaming-raw-1778293694109.wav`.
- Existing packaged raw corpus replay passes under the new no-offline-final path.
- Benchmark/report scripts fail if old `offline_final_*` fields are used as success criteria.
- Tests prove AI rewrite does not run or mutate V19 streaming HUD truth/final paste.

## 验收矩阵

| Area | Required proof |
| --- | --- |
| Hotkey down | HUD opens immediately with empty text before any speech chunk is required. |
| Audio | ASR-facing samples are mono/16k; logs show direct capture or per-chunk normalization. |
| Release | `CtrlUp` transitions to `ReleasedDraining`; no immediate paste/close. |
| Drain | pending audio/resampler/ASR output is drained while HUD stays visible. |
| Commit | `hud_truth_snapshot == ui_display_text == pasted_text`; exactly one paste request. |
| Old path removal | grep/audit proves no production release path calls offline final or final merge. |
| Regression | preview.51 duplicate raw capture replays without the duplicated tail artifact. |
| AI in V19 | AI rewrite is not implemented in V19 and cannot mutate HUD/paste. |
| Package | next preview dist+zip exists; new exe launched; preview.52 remains rollback. |

## 审计门禁

- Code audit gate: search production code for old release-final calls and fail if reachable.
- Report audit gate: replay reports must expose HUD/paste equality and must not use `offline_final_*` as success criteria.
- Runtime audit gate: live logs must include state transitions, audio format, drain timing, offline-final-not-invoked proof, and AI-rewrite-not-mutating proof.
- Packaging audit gate: final closeout must include exe path, PID, preview version, and rollback path.
- Regression audit gate: user's failed raw capture must be part of the acceptance suite.

## Live Acceptance On Windows

- Build a new preview version, expected next version `ainput-1.0.0-preview.53` unless versioning changes before implementation.
- Package a new dist directory and zip without overwriting preview.52.
- Stop the old running `ainput-desktop.exe` process.
- Launch the new executable in the interactive Windows desktop session.
- Report actual exe path and PID.
- Test at least: silence hold opens blank HUD; short Chinese phrase drains then pastes HUD text; user's duplicate-failure style sentence pastes once without duplicated phrase; long speech keeps HUD as truth source and pastes once after drain.

## Rollback

Preview.52 remains the rollback point: `C:\Users\sai\ainput\dist\ainput-1.0.0-preview.52\ainput-desktop.exe`.
