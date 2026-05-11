# preview83-code-switch-8of10 PLAN

## State

Status: implemented and released as `1.0.0-preview.83`.

This plan was executed without changing model, deployment, language, or AI rewrite behavior.

The first real implementation gate was Codex-only. `03-codex.wav` passed before the other nine terms were expanded.

Final result:

- Codex packaged proof gate: passed, 1/1, behavior failures = 0.
- Packaged full gate: passed twice consecutively with `8/10`, behavior failures = 0.
- Fixed failed terms: JSON and Gemini.
- Live Windows process: `C:\Users\sai\ainput\dist\ainput-1.0.0-preview.83\ainput-desktop.exe`, `SessionId=1`.

## Phase 1 - Build The Gate Harness

1. Add `scripts\run-mixed-terms-baseline.ps1`.
2. Inputs:
   - `-ExePath` defaulting to `target\debug\ainput-desktop.exe`.
   - `-BaselineDir` defaulting to `user-voice-corpus\baselines\mixed-terms-2026-05-11-200101`.
   - `-MinPass 8` for the full 10-case gate.
   - `-OnlyTerm Codex` or `-OnlyCase 03-codex` for the proof gate.
   - `-Repeat 1` for iteration and `-Repeat 2` for release gate.
3. The script must create or reuse a replay manifest in the baseline directory, for example `streaming-fixture-manifest.json`, with all 10 cases and exact `expected_text` values.
4. The script must be able to run only the Codex case without copying or deleting any user recordings.
5. The script must call `replay-streaming-manifest` or `replay-streaming-wav`, capture JSON via `AINPUT_JSON_OUTPUT_PATH`, parse the report, and write a summary under `tmp\mixed-terms-baseline\<timestamp>`.
6. For full mode, the script fails non-zero unless:
   - total cases = 10;
   - passed cases >= `-MinPass`;
   - behavior failures = 0;
   - content failures <= `10 - MinPass`.
7. For Codex-only mode, the script fails non-zero unless:
   - exactly one case runs;
   - case term = `Codex`;
   - content status = pass;
   - behavior status = pass.
8. The summary must show per case: id, term, expected, final_text, behavior_status, content_status, failures, and whether the canonical English term appears exactly.
9. Run the current build once and save the before score for Codex and full 10-case mode. Do not start repairs before the before score is recorded.

## Phase 2 - Codex Proof Loop First

Run this loop before touching other terms:

1. Add positive and negative tests around Codex repair in `crates\ainput-rewrite`.
2. Positive target:
   - `让抠的改这里。` -> `让 Codex 改这里。`
3. Negative targets:
   - `抠的地方需要改。` stays unchanged.
   - `扣的费用不对。` stays unchanged.
   - `这个抠的不是英文词。` stays unchanged unless a narrower context proves it is Codex.
4. Implement the smallest safe Codex rule.
5. Run:

```powershell
cargo test -p ainput-rewrite
cargo build -p ainput-desktop
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-mixed-terms-baseline.ps1 -ExePath .\target\debug\ainput-desktop.exe -BaselineDir .\user-voice-corpus\baselines\mixed-terms-2026-05-11-200101 -OnlyTerm Codex -MinPass 1
```

6. Repeat at most 3 focused Codex loops.
7. If Codex still fails after 3 focused loops, stop and report failure to the user. Do not implement the remaining term rules.
8. If Codex passes, proceed to Phase 3.

## Phase 3 - Add Safe Repair Tests For Other Terms

After Codex passes, add positive tests for the safe user-reported outputs:

- 打开哈的看一下。 -> 打开 HUD 看一下。
- 用克劳的扣的跑一下。 -> 用 Claude Code 跑一下。
- 问欧奔 AI 怎么做？ -> 问 OpenAI 怎么做？
- 去给哈看版本。 -> 去 GitHub 看版本。
- 这个 CP A，对吗？ -> 这个 CPA，对吗？
- 这个偷肯别动。 -> 这个 token 别动。
- 让杰曼乃复合。 -> 让 Gemini 复核。

Add negative tests before implementing risky rules:

- 把这项发给我。 should stay unchanged unless a later explicit narrow JSON rule is justified.
- 这个功能需要复合结构。 must not become `复核`.
- 哈哈的看一下。 must not become `HUD`.
- 给哈尔滨看版本。 must not become `GitHub`.
- 偷看别动。 must not become `token`.

## Phase 4 - Implement Conservative Current-Model Repair

1. Implement a small code-switch repair table in `crates\ainput-rewrite`, separated from generic Chinese cleanup logic.
2. Rule shape:
   - exact short-utterance replacements for very high-confidence cases;
   - context-window replacements where the surrounding Chinese command makes the English term clear;
   - no global replacement for common Chinese words.
3. Initial safe target rules after Codex:
   - `哈的` near `打开...看一下` -> `HUD`.
   - `克劳的扣的` -> `Claude Code`.
   - `欧奔 AI` / `open ai` spacing -> `OpenAI`.
   - `给哈` near `去...看版本` -> `GitHub`.
   - `CP A` -> `CPA`.
   - `偷肯` near `别动` -> `token`.
   - `杰曼乃` near `让...复合/复核` -> `Gemini`, and only in that context fix `复合` -> `复核`.
4. Treat JSON separately:
   - Add `JSON`, `json`, `j s o n` to low-boost phrase lists if exact forms are missing.
   - Do not globally rewrite `这项` to `JSON`.
   - With an 8/10 acceptance gate, JSON can remain failed if fixing it would risk normal Chinese.

## Phase 5 - Speech Context Phrase List, Secondary Only

1. Update both phrase-list copies if needed:
   - `sidecars\parakeet_code_switch_terms.json`
   - `data\terms\parakeet_code_switch_terms.json`
2. Candidate additions:
   - `HUD`, `JSON`, `CPA`, `token`, `VPS`, `Claude Code`, `OpenAI`, `GitHub`.
3. Keep the list small. If the sidecar rejects a phrase or health fails, remove that phrase and rely on app-side repair.
4. Do not change model, function id, deployment, `language=zh-CN`, or `multi`.

## Phase 6 - Iterate Until The 8/10 Gate Passes

For each full loop after Codex has passed:

1. Run unit tests:

```powershell
cargo test -p ainput-rewrite
```

2. Build source exe:

```powershell
cargo build -p ainput-desktop
```

3. Run the 10-case gate:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-mixed-terms-baseline.ps1 -ExePath .\target\debug\ainput-desktop.exe -BaselineDir .\user-voice-corpus\baselines\mixed-terms-2026-05-11-200101 -MinPass 8
```

4. If passed cases >= 8 and behavior failures = 0, stop adding recognition rules.
5. If failed, inspect only the failed cases' `final_text`, `final_online_raw_text`, and `partial_timeline`. Add the smallest safe rule and repeat.
6. Do not add broad JSON or Chinese-breaking rules to chase 9/10 or 10/10.

## Phase 7 - Regression And Release Gate

Run these before packaging:

```powershell
cargo fmt --all -- --check
cargo test -p ainput-rewrite
cargo test -p ainput-desktop online_parakeet -- --nocapture
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-raw-corpus.ps1 -ExePath .\target\debug\ainput-desktop.exe -RandomCount 8
python .\scripts\readme_closeout_guard.py .
```

Package a new preview, expected next version `1.0.0-preview.83`, only after the source gates pass:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1 -Version 1.0.0-preview.83
```

Then run Codex-only and full mixed-terms gates against the packaged exe:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-mixed-terms-baseline.ps1 -ExePath .\dist\ainput-1.0.0-preview.83\ainput-desktop.exe -BaselineDir .\user-voice-corpus\baselines\mixed-terms-2026-05-11-200101 -OnlyTerm Codex -MinPass 1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-mixed-terms-baseline.ps1 -ExePath .\dist\ainput-1.0.0-preview.83\ainput-desktop.exe -BaselineDir .\user-voice-corpus\baselines\mixed-terms-2026-05-11-200101 -MinPass 8 -Repeat 2
```

## Phase 8 - Live Sync And Closeout

1. Update `run-ainput.bat` and HKCU Run to point to preview83 only after package gate passes.
2. Restart the live app and verify the real process path is preview83.
3. Verify `/health` still reports zh-CN Parakeet, same function id, and not `multi`.
4. Update README, OPLOG, DECISIONS, and package README with:
   - Codex proof result;
   - final 10-case score;
   - failed cases if any;
   - model/deployment unchanged;
   - Chinese-quality protection notes.
5. Run final git checks and commit/push if this becomes a release closeout.

## Stop Conditions

Stop and report `SPEC_DRIFT` or blocked result instead of forcing changes if:

- Codex cannot be repaired after 3 focused loops.
- 8/10 cannot be reached without broad Chinese-breaking replacements.
- A sidecar phrase causes request failure or health failure.
- Existing Chinese/rewrite tests start failing and cannot be fixed without weakening the Chinese baseline.
- The live process or package path cannot be verified.
