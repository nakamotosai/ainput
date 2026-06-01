# preview.79 Parakeet zh-CN Code-Switch Repair

## Goal

Ship `1.0.0-preview.79` without changing the ASR model or deployment, improving the current online Parakeet zh-CN path for high-frequency Chinese/English mixed dictation failures.

## Scope

- Keep `nvidia/parakeet-ctc-0_6b-zh-cn` as the online ASR model.
- Keep the vps-jp hosted sidecar and current NVIDIA function id.
- Add a conservative online-only repair layer for known dropped English islands from real raw captures.
- Add a raw replay gate that replays the failing preview.78 captures through the live sidecar and then checks the same app repair logic.
- Correct the online config truth from misleading `multi` to the actual hosted language `zh-CN`.

## Non-Goals

- Do not switch to multilingual RNNT.
- Do not switch back to local Qwen as the default.
- Do not self-host NIM/Riva or change deployment topology.
- Do not enable speech context boosts by default.
- Do not touch `cliproxyapi` 8317 or NVIDIA credentials.

## Constraints

- Repairs must be exact-match or narrow high-confidence term rewrites.
- Pure Chinese and pure English behavior must not be made worse.
- preview.78 remains the rollback baseline.
- Real user microphone testing is still required because the ASR model can drop English acoustically before app repair sees it.

## Acceptance

- `cargo fmt --all -- --check` passes.
- `cargo check -p ainput-desktop` passes.
- `cargo test -p ainput-rewrite` passes.
- `cargo test -p ainput-desktop online_parakeet -- --nocapture` passes.
- `scripts\run-online-code-switch-replay.ps1` passes against the preview.78 failing raw captures.
- Windows package `dist\ainput-1.0.0-preview.79\ainput-desktop.exe` is built and launched.
- `run-ainput.bat` and HKCU Run point to preview.79.
- vps-jp `/health` still reports `nvidia/parakeet-ctc-0_6b-zh-cn` and `zh-CN`.
