# AInput Online Parakeet Realtime Preview 75 SPEC

## Goal

Fix the temporary online Parakeet ASR mode so the HUD receives live partial text while the user is holding the voice hotkey, instead of staying blank until release.

## Scope

- `nvidia_parakeet_online_sidecar.py` session and chunk behavior.
- `vps-jp` live adapter deployment.
- Preview package/version/doc updates for `1.0.0-preview.75`.

## Non-Goals

- Do not modify or restart `cliproxyapi` 8317.
- Do not put NVIDIA keys into git, TOML, dist packages, logs, or chat output.
- Do not change the Windows hotkey/HUD contract unless adapter partials are insufficient.

## Acceptance

- `/health` reports `streaming_partials=true`.
- `/chunk` can return non-empty text before `/finish`.
- A known WAV paced as realtime audio produces multiple partial updates before final.
- Starting the new preview still uses `nvidia_parakeet_online` and does not load local Qwen.
- `dist/ainput-1.0.0-preview.75/` and `.zip` exist.
- Latest Windows interactive process points to `dist\ainput-1.0.0-preview.75\ainput-desktop.exe`.
