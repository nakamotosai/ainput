# preview.80 Parakeet Safe Code-Switch

## Goal

Ship `1.0.0-preview.80` without changing the ASR model or deployment, improving short English islands in Mandarin-English mixed dictation while preserving the current Chinese recognition behavior.

## Scope

- Keep `nvidia/parakeet-ctc-0_6b-zh-cn`, function id `9add5ef7-322e-47e0-ad7a-5653fb8d259b`, and `language_code=zh-CN`.
- Keep the vps-jp sidecar deployment and Windows `online_streaming` mode.
- Enable only a low-boost, Riva-encodable speech context list for high-value English terms.
- Add conservative app-side repairs for observed Parakeet mixed-language failures:
  - `猫底` / `某体` in the known `multi` contexts.
  - `扣代斯` and nearby Codex mishears.
- Extend replay and text fixtures so mixed failures are covered without broadening Chinese rewrites.

## Non-Goals

- Do not switch to multilingual RNNT or `language=multi`.
- Do not switch deployment or self-host NIM/Riva.
- Do not add broad Chinese semantic rewriting.
- Do not enable high boost values such as `18.0`.
- Do not touch `cliproxyapi` 8317 except reading NVIDIA keys through the existing sidecar path.

## Constraints

- Pure Chinese output must not be changed by the new `multi` repair rules unless it exactly matches an already known dropped-English fixture.
- Pure English behavior must not be harmed by casing/product-name normalization.
- Any speech-context phrase must be verified as accepted by the current Riva endpoint before becoming a default boost phrase.
- If sidecar boost behavior becomes unstable, rollback is disabling `PARAKEET_ENABLE_SPEECH_CONTEXTS` or returning to preview.79.

## Acceptance

- `cargo fmt --all -- --check` passes.
- `cargo check -p ainput-desktop` passes.
- `cargo test -p ainput-rewrite` passes, including Chinese non-regression cases.
- `cargo test -p ainput-desktop online_parakeet -- --nocapture` passes.
- `scripts\run-online-code-switch-replay.ps1` passes against the raw captures and text-only regression cases.
- vps-jp `/health` reports `nvidia/parakeet-ctc-0_6b-zh-cn`, `zh-CN`, `boost_enabled=true`, and low `boost`.
- Windows package `dist\ainput-1.0.0-preview.80\ainput-desktop.exe` is built and launched.
- `run-ainput.bat` and HKCU Run point to preview.80.
