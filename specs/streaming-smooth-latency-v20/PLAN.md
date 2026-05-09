# Streaming Smooth Latency V20 PLAN

Updated: 2026-05-09
Status: implementation started; punctuation safety and tray version visibility patch in progress

## Objective

Implement the V20 spec in the next preview, expected `ainput-1.0.0-preview.54` unless versioning changes before build.

The new preview must preserve V19's single-chain HUD truth model while improving punctuation, perceived HUD smoothness, first visible text latency, and held-idle tail visibility.

## 计划前校对

- preview.53 remains the running rollback while preview.54 is being built and verified.
- The plan does not resurrect the old offline final, final merge, or second ASR pass.
- The plan keeps HUD as the truth source and requires paste text to equal HUD text.
- The plan treats punctuation, HUD rendering, ASR cadence, and held-tail visibility as separate causes instead of one vague "slow model" issue.
- The plan keeps AI rewrite in roadmap only and disables its runtime hot path for V20 voice sessions.

## 承诺到实现映射

1. 承诺：Preserve V19 single-chain architecture.
   实现：`worker.rs` keeps one recognizer session, no offline final, no final merge, and one commit envelope per hold.

2. 承诺：Replace the visible HUD typewriter behavior.
   实现：`overlay.rs` and `main.rs` switch normal ASR HUD display to immediate/batched update and add partial-visible ack.

3. 承诺：Remove deterministic punctuation insertion around generic marker words.
   实现：`worker.rs` removes old forced marker comma list and old semantic question/exclamation cue lists from the default formatter; `ainput-rewrite` no longer inserts connector commas or terminal punctuation by local word rules.

4. 承诺：Use conservative punctuation.
   实现：`worker.rs` keeps safe punctuation cleanup, gates punctuation-model output by source content alignment and structural spacing, downgrades unanchored terminal `？` / `！` to `。` on final text, and strips unanchored expressive punctuation from unstable preview text.

5. 承诺：Add same-chain held-idle tail chase.
   实现：`worker.rs` adds idle silence pulse on the same recognizer stream with no `input_finished()` and no `reset()`.

6. 承诺：Keep release finalization as same-chain closure and flush HUD before paste.
   实现：`worker.rs` and `main.rs` keep final HUD acknowledgement before paste and preserve equality gates.

7. 承诺：Disable AI rewrite completely in V20 voice runtime.
   实现：`main.rs`, `worker.rs`, and `config\ainput.toml` prevent client init, warmup, requests, and mutation.

8. 承诺：Add latency, HUD lag, tail, punctuation, and same-chain metrics.
   实现：benchmark script, replay fixtures, and runtime logs expose the new V20 metrics.

9. 承诺：Keep duplicate-tail regression guarded.
   实现：acceptance corpus includes the preview.51 duplicate-tail case and preview.53 recent user utterances.

10. 承诺：Expose current package version in tray UI.
    实现：`main.rs` adds a disabled top-level tray menu item `当前版本：<version>` and includes the same package version in the tray tooltip.

## Phase 0: Freeze Evidence Baseline

1. Keep a copy of current evidence:
   - `dist\ainput-1.0.0-preview.53\logs\ainput.log`
   - `dist\ainput-1.0.0-preview.53\logs\voice-history.log`
   - recent `streaming-raw-177831*.wav/json`
2. Create a V20 recent-corpus manifest from the user complaint cases:
   - punctuation examples
   - choppy long sentence
   - held-tail examples
   - typo example `亲，你先是点完整的speck方案。`
3. Add or update a local report command so the acceptance report can compute:
   - first visible partial latency
   - worker partial to HUD visible latency
   - content chars missing from HUD before release final
   - punctuation delta from raw ASR text
   - offline final invoked or not
   - AI rewrite initialized or not

Gate: baseline report reproduces the current problems instead of only reporting final pasted text.

## Phase 1: Remove Mechanical Punctuation

Files:

- `apps/ainput-desktop/src/worker.rs`
- relevant tests in the same file or a focused formatter test module

Steps:

1. Replace default `apply_streaming_semantic_commas()` behavior.
2. Remove deterministic comma insertion before/after generic markers from the default streaming path:
   - `然后`
   - `现在`
   - `还是`
   - `或者`
   - `比如`
   - similar connector words
3. Keep only safe punctuation cleanup:
   - dedupe repeated punctuation
   - normalize full-width punctuation
   - repair impossible punctuation such as `。，`
   - avoid terminal punctuation on unstable live partials
4. Make the offline punctuation restorer conservative:
   - do not run on every unstable tail by default, or filter its output if it inserts many commas into short text.
   - final same-chain formatter may add punctuation only after HUD flush, and the HUD must show the exact final text before paste.
5. Add tests from real examples:
   - `第二他现在的上屏速度还是一卡一卡的`
   - `请你在做下一个版本之前先想办法让他速度更快或者更流畅不要一个字一个字蹦出来`
   - `他会在一些词语后面强行加上逗号非常死板`
   - `这个怎么回事啊` must not become `这个怎么？回事啊？`

Gate: tests prove old forced-marker commas no longer appear by rule.

## Phase 1A: Tray Version Visibility

Files:

- `apps/ainput-desktop/src/main.rs`
- root `Cargo.toml`

Steps:

1. Bump the package version for the new preview build.
2. Add a disabled top-level tray menu item showing `当前版本：<version>`.
3. Include the same version in the tray tooltip.
4. Verify by building/running the preview and checking the live process path/version.

Gate: the user can identify the running version from the tray right-click menu without opening project files.

## Phase 2: Make HUD Display Immediate Or Batched

Files:

- `apps/ainput-desktop/src/overlay.rs`
- `apps/ainput-desktop/src/main.rs`
- `config\hud-overlay.toml` if a config knob is added

Steps:

1. Replace normal ASR HUD microstream default:
   - preferred default: immediate display of the latest target text.
   - acceptable fallback: batch reveal several chars per frame so normal text never looks one-character-at-a-time.
2. Stop resetting a one-char timer on every retarget in a way that stalls display.
3. Keep placeholder/silence HUD behavior unchanged.
4. Add partial HUD acknowledgement logging:
   - worker partial revision
   - overlay target text length
   - overlay visible text length
   - `partial_to_hud_visible_ms`
5. Keep final HUD acknowledgement gate before paste.

Gate: visual mode no longer has one-character typewriter behavior for normal ASR partials, and the report exposes HUD-visible lag.

## Phase 3: Add Same-Chain Held-Idle Tail Chase

Files:

- `apps/ainput-desktop/src/worker.rs`
- `config\ainput.toml`
- `crates/ainput-shell/src/lib.rs` if new config structs are needed

Proposed config:

```toml
[voice.streaming.tail_chase]
enabled = true
idle_ms = 180
cooldown_ms = 160
silence_pulse_ms = 120
max_silence_padding_per_idle_ms = 480
min_visible_chars = 2
```

Steps:

1. Add `maybe_chase_held_idle_tail()` in the live holding loop after normal partial decode.
2. Trigger only while hotkey is held and audio activity has been idle long enough.
3. Feed a bounded silence pulse to the same recognizer stream.
4. Call `decode_available()` and update the same HUD truth state.
5. Do not call `input_finished()`.
6. Do not call `recognizer.reset()`.
7. Do not paste or freeze commit during tail chase.
8. If the user resumes speaking, continue with the same recognizer stream.
9. Log:
   - idle duration
   - pulse ms
   - total injected silence for this idle period
   - before/after raw text
   - before/after HUD content chars

Gate: held-tail examples no longer commonly wait until CtrlUp to show the last 2+ content chars.

## Phase 4: Tune Partial Cadence And Capture Path

Files:

- `apps/ainput-desktop/src/worker.rs`
- `apps/ainput-desktop/src/main.rs`
- `crates/ainput-audio/src/lib.rs`
- `config\ainput.toml`

Steps:

1. Benchmark chunk sizes, starting with `60ms`, `40ms`, and `30ms`.
2. Lower the hard clamp in `streaming_chunk_num_samples()` only if replay/live metrics improve without CPU or stability regression.
3. Keep the empty HUD on hotkey down, so perceived start remains immediate even before first ASR token.
4. Re-check direct mono/16k capture:
   - if Windows device supports it cleanly, use direct mono/16k.
   - if not, keep current device-native capture plus per-chunk ASR-facing mono/16k normalization.
5. Log actual capture format and ASR-facing format for every session.

Gate: first partial target improves against the recent corpus or the report clearly proves the model's lower bound.

## Phase 5: Fully Disable AI Rewrite In Voice Runtime

Files:

- `config\ainput.toml`
- `apps/ainput-desktop/src/main.rs`
- `apps/ainput-desktop/src/worker.rs`

Steps:

1. Set streaming AI rewrite disabled for V20 voice mode.
2. Prevent `ensure_streaming_ai_rewrite_ready()` from initializing a client while rewrite is roadmap-only.
3. Keep future AI rewrite code in the repo, but unreachable from V20 voice sessions.
4. Keep `ai_rewrite_mutation_count=0` in reports.

Gate: runtime logs have no AI rewrite client enabled, warmup, request, adoption, mutation, or cancellation messages during voice sessions.

## Phase 6: Acceptance And Packaging

Commands and checks:

1. Run unit tests:
   - `cargo test -p ainput-desktop`
   - any changed crate-specific tests, especially `ainput-audio` or `ainput-shell` if touched.
2. Run V20 recent-corpus replay/benchmark.
3. Run the previous duplicate-tail regression.
4. Package new preview directory and zip.
5. Launch the new executable in the interactive Windows session.
6. Verify actual PID and path.
7. Run live manual checks:
   - silence hold opens blank HUD.
   - short sentence appears quickly and pastes HUD text exactly once.
   - long sentence does not appear one character at a time.
   - hold after speech idle brings tail onto HUD before release where same-chain recognizer can expose it.
   - punctuation is less mechanical on the user's complaint phrases.

Gate: all acceptance targets in `SPEC.md` pass or the build is not promoted as the next user test preview.

## Rollback

- Do not overwrite `preview.53`.
- Keep `preview.53` as immediate rollback unless `preview.54` is accepted and a newer rollback policy is written.
- Keep `preview.52` documented as the older stable rollback from V19 work.

## Risks And Mitigations

| Risk | Mitigation |
| --- | --- |
| Silence pulse changes recognition if user resumes speaking quickly | Bound pulse length, cooldown, and total padding; treat it as a natural pause in the same stream. |
| Removing comma rules makes punctuation too sparse | Prefer sparse punctuation in HUD; add only safe final punctuation shown on HUD before paste. |
| Lower chunk size increases CPU | Benchmark `60/40/30ms`; keep the fastest stable setting, not the smallest number blindly. |
| Immediate HUD reveal makes text jump on ASR revisions | Preserve stable prefix logic; only make the visible layer immediate after the worker has selected a target. |
| Tail chase cannot expose some final tokens without `input_finished()` | Release same-chain finalization still handles the true end; acceptance target allows proof when model lower bound exists. |

## Commit/Closeout Requirements For Implementation Turn

- Update `README.md` handoff with current preview, rollback path, and V20 status.
- Do not commit unrelated dirty files from previous work without reviewing them.
- Final implementation closeout must report:
  - changed files
  - test commands and results
  - package path
  - running PID/path
  - how the user should test
  - expected visible behavior
