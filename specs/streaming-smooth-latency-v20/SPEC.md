# Streaming Smooth Latency V20 SPEC

Updated: 2026-05-09
Status: implementation started; Phase 1 punctuation safety and tray version visibility in progress

## Goal

V20 keeps the V19 architecture contract, but fixes the remaining user-visible feel problems:

```text
CtrlDown -> blank HUD -> one streaming ASR chain -> HUD truth -> CtrlUp drain/finalize same chain -> paste HUD text once -> close HUD
```

V20 must make the HUD feel faster and smoother, reduce mechanical punctuation insertion, and make held-after-speaking tail text appear on the HUD before release whenever the streaming model can produce it safely.

## 上游真相源

- Runtime project: `C:\Users\sai\ainput` on `home-windows`.
- Currently running executable: `C:\Users\sai\ainput\dist\ainput-1.0.0-preview.53\ainput-desktop.exe`, PID `77552`.
- Current accepted architecture spec: `C:\Users\sai\ainput\specs\streaming-hud-truth-single-chain-v19\SPEC.md`.
- Current config surfaces: `config\ainput.toml`, `config\hud-overlay.toml`.
- Current source surfaces investigated: `apps/ainput-desktop/src/worker.rs`, `apps/ainput-desktop/src/overlay.rs`, `apps/ainput-desktop/src/main.rs`, `apps/ainput-desktop/src/streaming_fixtures.rs`.
- Recent real user evidence:
  - `dist\ainput-1.0.0-preview.53\logs\ainput.log`
  - `dist\ainput-1.0.0-preview.53\logs\voice-history.log`
  - `dist\ainput-1.0.0-preview.53\logs\streaming-raw-captures\streaming-raw-177831*.wav/json`
- Rollback point remains `C:\Users\sai\ainput\dist\ainput-1.0.0-preview.52\ainput-desktop.exe` unless a newer validated rollback is created.

## Investigation Findings

### 1. 标点现在确实偏死板

Current code has two punctuation layers:

- The offline punctuation model runs on streaming preview/final candidates when `rewrite_enabled = true`.
- `apply_streaming_semantic_commas()` then hard-codes comma insertion around discourse markers:
  - leading markers: `另外`, `然后`, `而且`, `但是`, `不过`, `所以`, `现在`
  - middle markers: `但是`, `不过`, `而且`, `然后`, `还是`, `尤其是`, `或者`, `比如`

This means some commas are not model judgement. They are deterministic code rules. Examples from real user output:

- `第二，他现在的上屏速度，还是一卡一卡的。`
- `请你在做下一个版本之前，先想办法，让他速度更快，或者更流畅，不要一个字，一个字蹦出来。`
- `他会在一些词语后面强行加上逗号，非常死板。`

Conclusion: V20 must remove or heavily constrain forced semantic comma insertion. The formatter should prefer fewer commas over mechanical commas.

Additional real complaint after the plan was written:

- `这个怎么？回事啊？`

Conclusion: this is not just comma handling. The default streaming path must not infer expressive punctuation such as `？` / `！` from fixed word lists or from an unsafe punctuation-model proposal. Generated punctuation must be source-aligned and structurally gated; when uncertain, use fewer punctuation marks.

### 2. HUD 卡顿有代码级原因

The overlay currently has a microstream typewriter layer:

```text
HUD_TICK_INTERVAL = 16ms
HUD_CHAR_STREAM_INTERVAL = 14ms
advance_one_char() = only one character per tick
```

So even after the worker already has a longer candidate, the HUD can intentionally show it one character at a time. `show_status_hud(..., char_streaming=true)` also resets `hud_last_char_tick_at` on each retarget, so frequent ASR retargets can make the visible text feel more stalled.

Current log metrics from 15 recent real sessions:

- `first_partial_elapsed_ms`: min `759`, p50 `1357`, max `2096`.
- 14 of 15 sessions exceeded the current `900ms` hard target.
- Final decode itself is fast: p50 around `59ms`.
- Release-to-commit is acceptable: p50 around `467ms`.

Conclusion: perceived lag is not mostly paste/final cost. It is first visible partial latency plus the overlay typewriter reveal.

### 3. 最后 1-2 个字松手才出现，是当前单链设计的副作用

V19 currently bypasses old pause soft-flush/endpoint rollover in the live loop:

```text
v19 keeps one continuous streaming recognizer chain; pause soft-flush and endpoint rollover are bypassed
```

That preserved the no-merge, no-duplicate architecture. But the streaming Paraformer often holds the final token until the stream is finished. Real evidence from `ainput.log`:

| HUD before release final | Final HUD/paste after release |
| --- | --- |
| `我现在来试试` | `我现在来试试看啊。` |
| `根本看不懂他们在干` | `根本看不懂他们在干什么。` |
| `但是，依旧有两个问` | `但是，依旧有两个问题。` |
| `请你...不要一个字，一个字蹦` | `请你...不要一个字，一个字蹦出来。` |
| `哪怕我一直按着他...一直在等一` | `哪怕我一直按着他...一直在等一样。` |
| `亲，你先是点完整的speck` | `亲，你先是点完整的speck方案。` |

Conclusion: V20 needs a while-held tail chase that does not reintroduce the old second pass, offline final, or HUD/offline merge.

### 4. AI rewrite is still initialized even though it is roadmap-only

Runtime logs show:

```text
streaming AI rewrite client enabled ...
streaming AI rewrite warmup completed ...
streaming AI rewrite cancelled reason="hotkey_release"
```

`ai_rewrite_mutation_count=0`, so it is not changing text, but it is still in the runtime hot surface and adds confusion/noise. V20 should fully disable it for the voice path until the future rewrite spec is implemented.

## Product Contract

- HUD opens immediately on hotkey down, including silence.
- HUD text is the truth source. Final pasted text must equal final HUD text exactly.
- No release-time offline final ASR pass.
- No HUD plus offline-tail merge.
- No old endpoint rollover that resets the recognizer and stitches segments.
- Releasing Ctrl stops new mic input only; HUD stays visible until same-chain finalization finishes.
- V20 may run a same-chain release finalization after CtrlUp because that final text is flushed to the HUD before paste.
- AI rewrite is not part of V20. It must not initialize, call network, mutate HUD, or mutate paste.

## New Engineering Flow

```text
                          ┌─────────────────────────────────────────────┐
CtrlDown                  │ HUD opens immediately                        │
─────────────────────────>│ empty small HUD is valid                     │
                          └─────────────────────┬───────────────────────┘
                                                │
                                                v
┌────────────────────┐   mono/16k ASR stream   ┌───────────────────────┐
│ Mic capture         │────────────────────────>│ Streaming recognizer   │
│ direct mono/16k if  │                         │ one live chain only    │
│ supported, else     │                         └───────────┬───────────┘
│ per-chunk normalize │                                     │
└────────────────────┘                                     │ raw partial
                                                            v
                                                 ┌───────────────────────┐
                                                 │ Conservative formatter │
                                                 │ no forced comma list   │
                                                 │ no AI rewrite          │
                                                 └───────────┬───────────┘
                                                             │
                                                             v
                                                 ┌───────────────────────┐
                                                 │ HUD truth immediate    │
                                                 │ no 1-char typewriter   │
                                                 └───────────┬───────────┘
                                                             │
Hotkey still held + recent audio idle                         │
─────────────────────────────────────────────────────────────┘
    same recognizer accepts bounded silence pulse
    decode_available()
    update the same HUD truth
    no input_finished()
    no reset()
    no paste()

CtrlUp
  -> stop accepting mic input
  -> drain already queued audio
  -> input_finished() on the same recognizer chain
  -> conservative formatter
  -> flush final HUD truth
  -> paste that exact HUD text once
  -> close HUD
```

## 设计承诺清单

1. 承诺： Preserve V19 single-chain architecture. No offline final, no merge, no second ASR truth source.
2. 承诺： Replace the visible HUD typewriter behavior with immediate or batched display that cannot lag behind the worker target by more than one frame under normal conditions.
3. 承诺： Remove deterministic comma insertion around generic words like `然后`, `现在`, `还是`, `或者`, `比如` from the default formatter.
4. 承诺： Use conservative punctuation: fewer commas are better than wrong mechanical commas.
5. 承诺： Add a same-chain held-idle tail chase: bounded silence is fed to the current recognizer while Ctrl is still held, without `input_finished()` and without `reset()`.
6. 承诺： Keep release finalization only as same-chain closure, then flush HUD before paste.
7. 承诺： Disable AI rewrite completely in V20 voice runtime.
8. 承诺： Add metrics for partial latency, HUD visible lag, tail-missing-before-release, punctuation changes, and same-chain proof.
9. 承诺： Keep existing duplicate-tail regression guarded.

## Non-goals

- Do not implement the future AI rewrite product.
- Do not switch provider/model family or add GPU as part of V20.
- Do not refactor direct paste/clipboard behavior unless HUD/paste equality requires it.
- Do not hide a second recognizer or raw-audio replay behind the name "tail chase".
- Do not optimize by pasting before HUD final text is visible.

## Acceptance Targets

| Area | Required proof |
| --- | --- |
| Architecture | Runtime logs prove `offline_final_invoked=false`, no production final merge, one commit envelope per hold. |
| HUD/paste equality | `hud_paste_equal=true` for all acceptance cases. |
| HUD smoothness | Live HUD no longer reveals normal ASR text one character per frame; target-to-visible lag p95 <= `100ms` after worker partial event. |
| First visible text | Recent raw/live corpus p50 `first_partial_elapsed_ms <= 1000ms`; no case over `1600ms` without an explicit low-signal/no-speech reason. |
| Held tail | If speech has been idle while Ctrl remains held for `450ms`, HUD should not be missing more than 1 content character when same-chain recognizer can expose it. |
| Release latency | Release-to-commit p95 <= `900ms`, hard <= `1200ms`. |
| Punctuation | No deterministic punctuation insertion from old marker/word lists in default mode; no `这个怎么？回事啊？`, `。，`, `。，但是`, repeated punctuation, or comma inserted only because a generic marker word appeared. |
| Version visibility | Tray right-click menu and tooltip show `env!("CARGO_PKG_VERSION")`, so the running preview can be identified without opening files. |
| AI rewrite | No AI rewrite client initialization, warmup, request, mutation, or cancellation log in V20 voice sessions. |
| Regression | preview.51 duplicate-tail case and preview.53 recent real utterances replay without duplicate phrase tails. |
| Package | New preview is packaged separately; preview.53 remains rollback unless superseded by a newer validated rollback. |

## 验收矩阵

1. 承诺：Preserve V19 single-chain architecture.
   验收：Runtime logs prove `offline_final_invoked=false`, no production final merge, and one commit envelope per hold.

2. 承诺：Replace the visible HUD typewriter behavior.
   验收：Live HUD no longer reveals normal ASR text one character per frame; target-to-visible lag p95 <= `100ms` after worker partial event.

3. 承诺：Remove deterministic punctuation insertion around generic marker words.
   验收：Default formatter no longer inserts commas only because `然后`, `现在`, `还是`, `或者`, or `比如` appeared; it also no longer infers `？` / `！` from fixed question/exclamation cue lists.

4. 承诺：Use conservative punctuation.
   验收：No `这个怎么？回事啊？`, `。，`, repeated punctuation, or dense comma output on recent user examples; fewer punctuation marks are accepted when uncertain.

5. 承诺：Add same-chain held-idle tail chase.
   验收：If speech has been idle while Ctrl remains held for `450ms`, HUD should not be missing more than 1 content character when same-chain recognizer can expose it.

6. 承诺：Keep release finalization as same-chain closure and flush HUD before paste.
   验收：`hud_paste_equal=true`; release-to-commit p95 <= `900ms`, hard <= `1200ms`.

7. 承诺：Disable AI rewrite completely in V20 voice runtime.
   验收：No AI rewrite client initialization, warmup, request, mutation, or cancellation log in V20 voice sessions.

8. 承诺：Add latency, HUD lag, tail, punctuation, and same-chain metrics.
   验收：Benchmark report includes all V20 metrics and fails if old success criteria hide these issues.

9. 承诺：Keep duplicate-tail regression guarded.
   验收：preview.51 duplicate-tail case and preview.53 recent real utterances replay without duplicate phrase tails.

10. 承诺：Expose the running version in the tray.
    验收：Right-click tray menu contains `当前版本：<package version>` and tray tooltip begins with `ainput <package version>`.

| Case | Evidence source | Pass condition |
| --- | --- | --- |
| Silence hold | Live Windows test | Blank HUD opens on press and closes without paste after release. |
| Short phrase | Live Windows test + log | First visible text <= target, paste equals HUD. |
| Long phrase | Recent raw corpus | HUD remains smooth; no character-by-character visible backlog. |
| User punctuation complaint | Recent voice-history examples | Output uses conservative punctuation and avoids forced commas around generic markers. |
| Tail delay examples | Recent log/raw cases | Tail text appears during held idle where possible, and release final no longer commonly adds 2+ content chars. |
| Duplicate regression | Old preview.51 raw capture | No repeated `失败案例来实失败的案例来实测` style artifact. |
| AI rewrite quarantine | Runtime log grep | No `streaming AI rewrite client enabled`, no warmup, no request. |

## 审计门禁

- Code audit:
  - Production streaming commit path must not call offline ASR.
  - Held-idle tail chase must not call `input_finished()` or `recognizer.reset()`.
  - AI rewrite initialization must be gated off for voice streaming.
  - Old hard-coded semantic comma list must not run in default streaming formatter.
- Metrics audit:
  - Benchmark report must include `first_partial_elapsed_ms`, `partial_to_hud_visible_ms`, `tail_missing_content_chars_before_release`, `release_to_commit_elapsed_ms`, `hud_paste_equal`, `offline_final_invoked`, `ai_rewrite_mutation_count`.
- Runtime audit:
  - Logs must prove actual input format, ASR-facing format, chunk size, tail-chase events, HUD visible ack, and commit equality.
- Packaging audit:
  - New preview directory, launched PID, rollback path, and test command outputs must be reported.

## 计划前校对

- This plan does not resurrect the old mode.
- This plan does not make HUD a preview of a hidden final string.
- This plan does not use AI rewrite to solve punctuation or smoothness.
- This plan treats punctuation as formatter policy, not ASR truth.
- This plan treats smoothness as both model cadence and overlay rendering, not only a model speed issue.
- This plan treats the last-two-chars problem as a same-chain recognizer finalization/cadence issue, not as user timing error.

## 承诺到实现映射

| Commitment | Implementation surface |
| --- | --- |
| No forced comma list | `worker.rs`: replace `apply_streaming_semantic_commas()` default behavior with conservative formatter policy and tests. |
| Smooth HUD | `overlay.rs`: replace one-char microstream default with immediate or batched reveal; add partial-visible acknowledgement. |
| Faster partials | `worker.rs`, `config\ainput.toml`: tune chunk minimum and preview path; measure before/after. |
| Held tail appears before release | `worker.rs`: add `maybe_chase_held_idle_tail()` using same recognizer, bounded silence, no finish/reset. |
| HUD remains truth | `worker.rs`, `main.rs`: final HUD flush still required before paste; commit uses HUD snapshot. |
| AI rewrite deferred | `main.rs`, `worker.rs`, `config\ainput.toml`: disable init and calls in streaming voice runtime. |
| Reproducible acceptance | `scripts\run-streaming-latency-benchmark.ps1`, replay fixtures, recent raw corpus manifest. |
