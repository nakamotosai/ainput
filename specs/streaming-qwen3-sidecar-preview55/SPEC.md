# Qwen3 ASR Sidecar Preview 55 SPEC

Updated: 2026-05-10

## Goal

Switch the next ainput streaming preview to the original `Qwen/Qwen3-ASR-0.6B` running on the user's local Windows GPU through a WSL2 sidecar, while preserving the V19 HUD-truth architecture.

```text
CtrlDown -> blank HUD -> mono/16k capture -> Qwen sidecar streaming session
  -> HUD truth partials -> CtrlUp drain -> finish same sidecar session
  -> flush final text to HUD -> paste exact HUD text once -> close HUD
```

## Current Facts

- Runtime project: `C:\Users\sai\ainput` on `home-windows`.
- Current package baseline: `1.0.0-preview.54`.
- Current accepted architecture: V19 single streaming chain; HUD text is the final truth source.
- Qwen environment lives under WSL Linux disk: `/home/sai/ainput-qwen3-asr`.
- Sidecar API verified on `http://127.0.0.1:8765` with `Qwen/Qwen3-ASR-0.6B`.
- HTTP sidecar eval on five recent failure wavs wrote `tmp/qwen3-asr-0.6b-sidecar-http-eval.json`.

## Product Contract

- The default preview.55 streaming backend is `qwen3_sidecar`.
- The old sherpa backend remains available by config as `backend = "sherpa"`.
- Qwen must run locally through WSL/GPU; no cloud ASR is allowed.
- Do not store the Qwen model or venv under `C:\Users\sai`; keep it under `/home/sai/ainput-qwen3-asr`.
- Do not reintroduce offline final, HUD plus offline-tail merge, or hidden release correction.
- HUD final flush remains mandatory before paste; final pasted text must equal the acknowledged HUD text.
- AI rewrite remains roadmap-only and must not initialize or mutate voice text in this preview.

## Config Contract

```toml
[voice.streaming]
backend = "qwen3_sidecar"
sidecar_url = "http://127.0.0.1:8765"
sidecar_auto_start = true
sidecar_wsl_distro = "Ubuntu"
sidecar_wsl_workdir = "/home/sai/ainput-qwen3-asr"
chunk_ms = 500
```

`chunk_ms = 500` is intentional for the Qwen route because the original streaming API emits stable text around its internal 2s chunk cadence; 60ms chunks only add HTTP overhead.

## Acceptance

- `GET /health` on the sidecar returns `ok=true` before ainput reports the streaming worker ready.
- If the sidecar is not running, ainput can start it through WSL and wait for health.
- Five recent real wav cases pass through the HTTP sidecar with the same results as direct Qwen evaluation.
- Rust checks pass after adding the new backend.
- Packaged preview.55 contains the new config and version, without overwriting preview.54.
- Running tray menu/tooltip reports `1.0.0-preview.55`.
- Live Windows process is launched from `dist\ainput-1.0.0-preview.55\ainput-desktop.exe`.

## Rollback

- Package rollback: `C:\Users\sai\ainput\dist\ainput-1.0.0-preview.54\ainput-desktop.exe`.
- Config rollback inside preview.55: set `[voice.streaming].backend = "sherpa"` and `chunk_ms = 60`.
