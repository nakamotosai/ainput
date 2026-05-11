# AInput Online Parakeet ASR Preview 73 SPEC

## Goal

Add a temporary third streaming ASR backend named `nvidia_parakeet_online` and ship a new preview package that starts in online ASR mode by default. The new mode must call NVIDIA Parakeet CTC zh-CN online instead of loading local Qwen/SenseVoice streaming models.

## Scope

- AInput config and Rust runtime backend dispatch.
- A lightweight HTTP ASR adapter compatible with the existing Qwen sidecar session API.
- A temporary `vps-jp` deployment of that adapter, using the existing `cliproxyapi` 8317 production NVIDIA key pool as the key truth source.
- Packaging `1.0.0-preview.74` into a new `dist` directory and zip.
- Windows live startup validation.

## Non-Goals

- Do not modify or restart `cliproxyapi` 8317 production.
- Do not put NVIDIA API keys into git, TOML, dist packages, logs, or chat output.
- Do not delete or overwrite `dist/ainput-1.0.0-preview.72`.
- Do not make Parakeet the permanent architecture decision yet; this is a temporary preview experiment.

## Constraints

- `preview.72` remains the rollback point for repaired local Qwen mode.
- Online Parakeet is Riva gRPC/NVCF, not OpenAI-compatible `/v1/audio/transcriptions`.
- The adapter must avoid Windows local GPU usage and must not start the Qwen WSL sidecar.
- The first temporary implementation may return only final text on release; live HUD partials are not required for this experiment.

## Acceptance

- `config/ainput.toml` and packaged config default to `voice.mode = "streaming"` and `voice.streaming.backend = "nvidia_parakeet_online"`.
- Starting the new preview does not log Qwen model preload and does not create a Qwen GPU load.
- Adapter `/health` works from Windows against `vps-jp` over Tailnet and reports 5 configured keys without exposing them.
- A known WAV transcribes through the online adapter.
- `cargo fmt` and `cargo check -p ainput-desktop` pass.
- `dist/ainput-1.0.0-preview.74/` and `.zip` exist.
- Latest Windows interactive process points to `dist\ainput-1.0.0-preview.74\ainput-desktop.exe`.
