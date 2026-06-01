# preview83-code-switch-8of10 SPEC

## Goal

Make AInput, without changing the current ASR model or deployment, recognize the fixed 10 real user recordings for short Chinese/English mixed commands well enough that at least 8 of 10 pass strict replay acceptance.

Before spending effort on all terms, prove the approach on the single Codex case. If the Codex case cannot be repaired after multiple focused attempts, stop this milestone and report that the current model path is not worth extending for the remaining terms.

## Result

- Released as `1.0.0-preview.83`.
- Codex proof gate passed first on `03-codex.wav`.
- Packaged preview full gate passed twice consecutively at `8/10`, behavior failures = `0`.
- Remaining fixed failures: JSON and Gemini.
- Model, deployment, `language=zh-CN`, and AI rewrite hot path were not changed.

## Current Facts Checked

- Project root: `C:\Users\sai\ainput` on `home-windows`.
- Current release candidate: `1.0.0-preview.82`.
- Current online ASR must remain `nvidia/parakeet-ctc-0_6b-zh-cn`, function id `9add5ef7-322e-47e0-ad7a-5653fb8d259b`, `language=zh-CN`.
- `multi` is not allowed because prior live testing showed it breaks Chinese.
- Fixed baseline folder exists at `user-voice-corpus\baselines\mixed-terms-2026-05-11-200101` with 10 wav files and `manifest.json`.
- Codex proof case: `03-codex.wav`, expected text `让 Codex 改这里。`, user-reported current output `让抠的改这里。`.
- Existing replay entrypoints: `ainput-desktop.exe replay-streaming-wav <wav> [expected]` and `replay-streaming-manifest <manifest.json>`.
- Existing repair layer: `crates\ainput-rewrite`, especially `normalize_transcription()` and `repair_parakeet_code_switch_terms()`.

## Fixed Baseline Cases

| # | Term | Wav | Expected | User-reported current output |
|---:|---|---|---|---|
| 1 | HUD | `01-hud.wav` | 打开 HUD 看一下。 | 打开哈的看一下。 |
| 2 | JSON | `02-json.wav` | 把 JSON 发给我。 | 把这项发给我。 |
| 3 | Codex | `03-codex.wav` | 让 Codex 改这里。 | 让抠的改这里。 |
| 4 | Claude Code | `04-claude-code.wav` | 用 Claude Code 跑一下。 | 用克劳的扣的跑一下。 |
| 5 | OpenAI | `05-openai.wav` | 问 OpenAI 怎么做。 | 问欧奔 AI 怎么做？ |
| 6 | GitHub | `06-github.wav` | 去 GitHub 看版本。 | 去给哈看版本。 |
| 7 | CPA | `07-cpa.wav` | 这个 CPA 对吗。 | 这个 CP A，对吗？ |
| 8 | token | `08-token.wav` | 这个 token 别动。 | 这个偷肯别动。 |
| 9 | Gemini | `09-gemini.wav` | 让 Gemini 复核。 | 让杰曼乃复合。 |
| 10 | VPS | `10-vps.wav` | 把 VPS 重启。 | 把 VPS 重启。 |

## Scope

In scope:

- Add a deterministic mixed-terms replay harness for this baseline folder.
- Add a Codex-only proof mode in that harness, or a separate Codex-only manifest, before the full 10-case loop.
- Improve current-model results by conservative local post-ASR repair and, where useful, low-risk speech-context phrase list updates.
- Add unit tests and negative tests for every new repair rule.
- Package a new preview only after the Codex proof gate, source replay gate, and package replay gate pass.

Out of scope:

- Switching to `multi`, multilingual RNNT, another model, or another deployment.
- Turning on LLM / AI rewrite in the hot path.
- Broad semantic rewriting.
- Committing real user recordings into git.
- Claiming complete bilingual ASR in all contexts. This milestone is a fixed 10-recording regression target plus conservative reusable term repair.

## Codex Proof Gate

The first implementation target is only `03-codex.wav`.

Acceptance for this gate:

1. `03-codex.wav` is replayed through the same streaming path as normal mixed-terms testing.
2. Final normalized replay text equals normalized expected text: `让 Codex 改这里。`.
3. Behavior failures are 0.
4. The repair is protected by positive and negative `ainput-rewrite` tests.
5. The rule must be narrow enough not to rewrite unrelated Chinese such as `抠的地方`, `扣的费用`, or normal words containing similar sounds.

Attempt budget:

- Try up to 3 focused Codex repair loops.
- A loop means: inspect Codex replay output, add the smallest safe rule or phrase-list change, run unit tests, rebuild, and replay the Codex case.
- If Codex still does not pass after 3 focused loops, stop the milestone and report failure instead of expanding to HUD/JSON/Claude/etc.
- If Codex passes, proceed to the full 10-case 8/10 gate.

## Repair Policy

1. Prefer deterministic repair in `ainput-rewrite` when the misrecognition is high-confidence and context-limited.
2. Use ASR speech-context hints only as a secondary assist; do not trust them as the only fix.
3. No global replacements for common Chinese phrases.
4. JSON is high risk: `把这项发给我` can be a valid normal Chinese sentence. Do not globally rewrite `这项` to `JSON`.
5. Any risky rule must be anchored to a short utterance/template and protected by negative tests.
6. Existing Chinese quality is protected by `cargo test -p ainput-rewrite`, explicit Chinese negative tests, and raw-corpus behavior replay.

## Full Acceptance

The full goal is PASS only when all of these are true:

1. Codex proof gate passes first.
2. The mixed-terms baseline runner attempts all 10 fixed wav files.
3. At least 8 of 10 cases pass strict content acceptance.
4. Strict content acceptance means normalized final replay text equals normalized expected text. Keyword-only success is not enough for this gate.
5. Behavior failures are 0: no missing partials, no obvious final tail loss, no duplicate/conflicting punctuation gate failure.
6. The final source build and the packaged preview build both pass the mixed-terms gate.
7. The packaged preview mixed-terms gate passes twice consecutively to reduce online ASR fluke risk.
8. `cargo fmt --all -- --check` passes.
9. `cargo test -p ainput-rewrite` passes and includes the new positive and negative code-switch tests.
10. `cargo test -p ainput-desktop online_parakeet -- --nocapture` passes.
11. `scripts\run-streaming-raw-corpus.ps1` still passes on at least a small user-corpus sample, proving the general streaming behavior gate was not broken.
12. `python .\scripts\readme_closeout_guard.py .` passes before release closeout.
13. README, OPLOG, DECISIONS, and package README document the new preview, final 8/10-or-better score, and any remaining failed terms.

## Expected First Commands For /goal Execution

From `C:\Users\sai\ainput`:

```powershell
cargo build -p ainput-desktop
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-mixed-terms-baseline.ps1 -ExePath .\target\debug\ainput-desktop.exe -BaselineDir .\user-voice-corpus\baselines\mixed-terms-2026-05-11-200101 -OnlyTerm Codex -MinPass 1
```

If `run-mixed-terms-baseline.ps1` does not exist yet, create it first as described in `PLAN.md` Phase 1.

## Risks

- Some English terms are acoustically ambiguous under zh-CN CTC. Text-only repair cannot always know whether the user meant English or normal Chinese.
- The Codex proof gate is intentionally early; if this one cannot be fixed, the rest of the work is likely poor return on time.
- The JSON case is the clearest ambiguity and should not be fixed by a broad `这项 -> JSON` rule.
- Online ASR may drift slightly between runs; this is why the packaged preview must pass twice consecutively.
- Overfitting is possible. The defense is context-limited rules, negative tests, and an 8/10 target instead of forcing unsafe 10/10.
