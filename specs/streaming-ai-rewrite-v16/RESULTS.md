# Results

Status: complete for `1.0.0-preview.50`.

## Package

- Package dir: `dist\ainput-1.0.0-preview.50\`
- Zip: `dist\ainput-1.0.0-preview.50.zip`
- Started exe: `C:\Users\sai\ainput\dist\ainput-1.0.0-preview.50\ainput-desktop.exe`
- Started PID: `64444`

## Model and Config

- Endpoint: `http://vps-jp.tail4b5213.ts.net:8317/v1/chat/completions`
- Model: `qwen/qwen3.5-122b-a10b`
- API key source: Windows User env `AINPUT_CLIPROXYAPI_8317_KEY`
- Key storage rule: no key value in TOML / README / git text artifacts.

## Verification

- `cargo fmt` passed.
- `cargo fmt --check` passed.
- `cargo check -p ainput-desktop` passed.
- `cargo test -p ainput-desktop ai_rewrite -- --nocapture` passed, 12/12.
- `cargo test -p ainput-desktop streaming -- --nocapture` passed, 32/32.
- `cargo test -p ainput-desktop hotkey -- --nocapture` passed, 7/7.
- `cargo test -p ainput-rewrite -- --nocapture` passed, 16/16.
- `cargo test -p ainput-shell streaming_ai_rewrite -- --nocapture` passed, 2/2.
- Remote model probe passed:
  - Input: `我觉的这个工能不太队`
  - Output: `我觉得这个功能不太对`
- Full streaming audit passed:
  - Command: `.\scripts\run-streaming-full-audit.ps1 -Version 1.0.0-preview.50 -LatencyRepeats 1 -LiveCaseLimit 3`
  - Result: `overall_status=pass`, P0=0, P1=0, P2=0
  - Report: `tmp\streaming-full-audit\20260502-013018-999\full-audit-report.json`

## Acceptance Mapping

- AI rewrites only while `Ctrl` is held: covered by runtime epoch/closed state and release cancellation tests.
- HUD remains truth source: AI result merges only into current HUD tail; final commit still uses HUD final ack.
- Release cancels AI immediately: `cancel_streaming_ai_rewrite(..., "hotkey_release")` closes the session and increments epoch.
- Late results after release cannot update HUD/final text: covered by `ai_rewrite_release_epoch_drops_late_results`.
- Stale-compatible results can be accepted: covered by suffix-preserving stale-compatible unit test.
- Stale-incompatible results are dropped: covered by stale-incompatible unit test.
- True bilingual tail is preserved: covered by `ai_rewrite_keeps_true_bilingual_tail`.
- AI unavailable fallback: client backoff/unavailable path keeps local streaming preview; full audit passed with normal streaming gates.

## Remaining Risk

- Human semantic satisfaction still needs real microphone use. The automated gates prove request/cancel/merge/commit behavior and one real remote rewrite probe, but they do not score every possible spoken semantic correction.
