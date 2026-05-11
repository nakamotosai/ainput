# AInput preview77: Online Parakeet Multilingual RNNT

## Goal

Ship `1.0.0-preview.77` with the independent `online_streaming` mode using NVIDIA `nvidia/parakeet-1_1b-rnnt-multilingual-asr` instead of the previous zh-CN-only Parakeet CTC model.

Primary user languages are Japanese, Chinese, and English.

## Scope

- Update the vps-jp NVIDIA Parakeet adapter model defaults.
- Update the Windows packaged sidecar model defaults.
- Set online streaming language to `multi` in source config and generated defaults.
- Keep the app default mode as `online_streaming`.
- Package and launch a new preview build: `dist\ainput-1.0.0-preview.77\`.
- Update README, OPLOG, DECISIONS, TASKLIST, launcher, and startup entry.

## Non-Goals

- Do not modify or restart `cliproxyapi` 8317.
- Do not write NVIDIA API keys into repo, config, dist, logs, or responses.
- Do not remove or rewrite local Qwen/Sherpa modes.
- Do not delete rollback packages `preview.76` or `preview.72`.

## Runtime Truth

- Windows app calls `http://vps-jp.tail4b5213.ts.net:18765`.
- vps-jp adapter service is `ainput-parakeet-asr.service`.
- Expected health model: `nvidia/parakeet-1_1b-rnnt-multilingual-asr`.
- Expected function id: `71203149-d3b7-4460-8231-1be2543a1fca`.
- Expected language: `multi`.

## Acceptance

- `/health` from Windows returns the multilingual model, function id, `language=multi`, `key_count=5`, and `streaming_partials=true`.
- Package config has `mode = "online_streaming"`.
- Package local streaming backend remains `qwen3_sidecar`.
- Package online streaming backend remains `nvidia_parakeet_online` with `language = "multi"`.
- Running Windows process path is `C:\Users\sai\ainput\dist\ainput-1.0.0-preview.77\ainput-desktop.exe` in the interactive desktop session.
- Startup launcher and HKCU Run point to preview77.
- Runtime log shows `voice_mode=OnlineStreaming`, backend `NVIDIA Parakeet online ASR`, the multilingual model, and `local model preload skipped`.
- `cargo fmt --all -- --check`, `cargo check -p ainput-desktop`, `cargo test -p ainput-shell`, `git diff --check`, and `scripts\readme_closeout_guard.py .` pass.

## Rollback

- `dist\ainput-1.0.0-preview.76\` is the independent online zh-CN CTC rollback.
- `dist\ainput-1.0.0-preview.72\` is the local Qwen baseline rollback.
