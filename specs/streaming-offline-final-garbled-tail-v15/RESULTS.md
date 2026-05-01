# Results

Status: pass.

## What Changed

- Added an offline-final raw guard that rejects short mixed ASCII/CJK tails when the CJK content is already covered by the HUD tail.
- Added a final commit guard that rejects already-polluted candidates shaped like `HUD + garbled duplicate tail`.
- Kept true bilingual appends by requiring the CJK tail content to already be covered by the HUD tail and the ASCII letters to look embedded as noise.

## Regression Coverage

- `We住慎重ong。` after HUD `还是得稳住慎重` is rejected.
- Already-polluted candidate `还是得稳住慎重We住慎重ong。` resolves through HUD text and final delivery adds the normal sentence boundary.
- True bilingual candidate `这个功能支持 Windows 版本。` is preserved.

## Verification

- `cargo fmt --check`: pass
- `cargo check -p ainput-desktop`: pass
- `cargo test -p ainput-desktop garbled -- --nocapture`: pass
- `cargo test -p ainput-desktop streaming -- --nocapture`: pass
- `cargo test -p ainput-desktop hotkey -- --nocapture`: pass
- `cargo test -p ainput-rewrite -- --nocapture`: pass
- `scripts\run-streaming-full-audit.ps1 -Version 1.0.0-preview.49 -LatencyRepeats 1 -LiveCaseLimit 3`: pass, P0=0/P1=0/P2=0

Final audit report:

- `tmp\streaming-full-audit\20260502-000910-084\full-audit-report.json`

## Package

- `dist\ainput-1.0.0-preview.49\`
- `dist\ainput-1.0.0-preview.49.zip`
- Started on the Windows interactive desktop: PID `20096`

