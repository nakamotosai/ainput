# ainput2 Health Check

This document is the release and regression health-check plan for `ainput2`.
The Windows tree `C:\Users\sai\ainput2` is the runtime truth source. The Linux
mirror is useful for code review, but it is not enough to prove the packaged
desktop app.

## Goals

- Prove the current package, process, user state, sidecar, API config, history,
  HUD, rewrite, and paste routes are internally consistent.
- Catch regressions in the areas that have broken before: hotkeys, empty
  streaming sessions, sidecar fallback, HUD visibility, async rewrite, target
  replacement, and suspected-term learning.
- Keep a clear split between automated checks and manual Windows desktop
  acceptance.

## Quick Command

Run this from Windows PowerShell:

```powershell
cd C:\Users\sai\ainput2
.\scripts\healthcheck.ps1
```

For source-level checks too:

```powershell
cd C:\Users\sai\ainput2
.\scripts\healthcheck.ps1 -RunCargo
```

For the full local gate:

```powershell
cd C:\Users\sai\ainput2
.\scripts\healthcheck.ps1 -Deep
```

For Parakeet streaming regression with a real WAV:

```powershell
cd C:\Users\sai\ainput2
.\scripts\healthcheck.ps1 -Wav C:\path\to\sample.wav
```

The script prints JSON. `FAIL` means the release is not healthy. `WARN` means
the app may still be usable, but the issue needs review before calling the
build stable.

## Automated Checks

1. Baseline and package
   - Read `Cargo.toml` version.
   - Check Git HEAD and dirty status.
   - Check `dist\ainput2-<version>\ainput2.exe`.
   - Check whether the live `ainput2.exe` process points at the expected dist.

2. Runtime config and API
   - Parse `state\config\api-connections.json`.
   - Redact inline API key in report output.
   - Read `state\config\rewrite-user.toml`.
   - Read `state\logs\api-setup-status.json`.
   - Probe configured sidecar `/health` and `/v1/settings/asr` unless
     `-SkipNetwork` is used.

3. History and rewrite behavior
   - Parse `state\logs\history.jsonl`.
   - Count modes and skipped reasons.
   - Report `empty_hud_snapshot` rate as background noise from Ctrl shortcut
     overlap. Do not fail or warn on this number by itself.
   - Confirm recent rewrite records include both streaming and non-streaming
     routes when those toggles have been used.

4. Logs and console
   - Check `state\logs\ainput2.log` size.
   - Extract the latest local web-console URL from the log.
   - Probe that URL unless `-SkipNetwork` is used.

5. Sidecar mirror
   - Check `tmp\sidecar\nvidia_parakeet_online_sidecar.py` exists.
   - Report its SHA256 so it can be compared with the live vps-jp sidecar.

6. Optional source gate
   - `-RunCargo` runs `cargo fmt --check` and `cargo test -- --nocapture`.
   - `-Deep` additionally runs `cargo build --release`.
   - `-Wav` runs `scripts\parakeet_streaming_regression.py` against a real WAV.

## Manual Desktop Acceptance

Automated checks do not replace these real-use checks:

1. `Ctrl` streaming, rewrite off
   - Uncheck `流式 AI 改写`.
   - Hold `Ctrl` and dictate a Chinese sentence for at least 3 seconds.
   - Pass: HUD shows partials, release pastes one final raw/finalized text.

2. `Ctrl` streaming, rewrite on
   - Check `流式 AI 改写`.
   - Test in WezTerm and one normal text field.
   - Pass: terminal uses HUD-first final paste; normal text field may use
     raw-first plus async replacement. History records `streaming_asr_*rewrite`.

3. `CapsLock` non-streaming, rewrite on/off
   - Toggle `非流式 AI 改写`.
   - Hold `CapsLock` and dictate a Chinese sentence.
   - Pass off: raw Whisper/fallback text is pasted.
   - Pass on: rewrite uses the same model/fallback policy as streaming rewrite.

4. Short press and rapid repeat
   - Tap `Ctrl`, tap `CapsLock`, then do two valid holds back to back.
   - Pass: taps do not paste garbage; valid holds are not swallowed.

5. Sidecar fallback
   - Confirm `/health` shows Parakeet and Whisper model info.
   - When Whisper is degraded, CapsLock should still return Parakeet final text
     instead of staying stuck at `识别中...`.

6. HUD and console
   - Open tray `打开控制台`.
   - Open HUD, history, debug, prompt, corrections, and suspect pages.
   - Pass: pages load, HUD setting changes affect the real HUD, debug settings
     read back from the sidecar.

7. Suspect terms and corrections
   - Open `疑似错词`.
   - Apply, ignore, and edit one candidate in a controlled test.
   - Pass: rules appear under personal corrections, applied/dismissed entries
     move to archive, protected replacements still block known bad rewrites.

8. Output targets
   - Test WezTerm, browser textarea, standard Win32 edit field, and one unknown
     app surface.
   - Pass: text lands in the intended target; target change prevents final paste
     instead of pasting into the wrong app.

## Pass Criteria

A build can be treated as healthy when:

- `scripts\healthcheck.ps1 -RunCargo` has zero `FAIL`.
- Sidecar `/health` is `ok=true`.
- `history.jsonl` parses without meaningful malformed lines.
- `empty_hud_snapshot` is understood as expected Ctrl shortcut-overlap noise
  unless a deliberate hold-to-talk test reproduces "held Ctrl, spoke, got no
  text."
- Both `Ctrl` and `CapsLock` pass manual desktop acceptance.
- Streaming and non-streaming rewrite are verified to use the same rewrite
  model/fallback/protection path.
- No API key is printed in logs or health-check output.

## Known Watch Items

- `empty_hud_snapshot` is normal when Ctrl overlaps copy/paste and other
  shortcuts. It becomes actionable only when the user deliberately holds Ctrl to
  dictate and still gets no text.
- The sidecar code should match the mirror by hash, while runtime settings are
  allowed to drift because the debug panel edits live ASR settings.
- The local web console binds to a random `127.0.0.1` port; the latest URL is
  discoverable from `state\logs\ainput2.log`.
- Windows package proof must be done against `C:\Users\sai\ainput2`; Linux-only
  checks are not enough for release acceptance.
