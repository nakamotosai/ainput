# ainput

**Local-first Windows voice dictation.** Hold a hotkey, speak, get text pasted into the focused app. Optional AI rewrite via **your own** OpenAI-compatible API key.

![logo](assets/logo.svg)

| | |
|---|---|
| Platform | Windows 10/11 x64 |
| ASR | Local SenseVoice (sherpa-onnx), bundled model |
| Rewrite | Optional OpenAI-compatible `chat/completions` (you bring base URL + key + model) |
| Install | Green zip — unpack and run (no installer) |
| License | Application: **MIT** · Model weights: see `THIRD_PARTY_NOTICES` / model `LICENSE` |

## Features (public product)

- **CapsLock hold-to-talk** → local non-streaming transcription
- Optional **AI rewrite** (non-streaming HTTP) when you configure an API
- Tray icon + HUD feedback
- Personal correction rules (local)
- No cloud ASR, no screen recording, no built-in vendor API key

## Quick start

1. Download the release zip (or build from source).
2. Unpack anywhere.
3. Run `ainput.exe` or `run-ainput.bat`.
4. Hold **CapsLock**, speak, release — text pastes into the focused window.
5. (Optional) Tray → **API / 改写设置…** → fill Key → **拉取模型** → pick model → set timeout → enable rewrite → Save.

Runtime state is stored next to the executable under `state/` (config, logs, history).

## Configure AI rewrite

Use the **native settings panel** (tray → `API / 改写设置…`):

| Field | Default / notes |
|---|---|
| Base URL | Prefilled `https://integrate.api.nvidia.com/v1` (any OpenAI-compatible endpoint works) |
| API Key | You provide; stored only in local `state/config/` |
| Model | Type manually, or click **拉取模型** after Key is filled |
| Timeout (ms) | Default `5000` — used for rewrite, model list pull, and save probe |
| Save | Writes **API Key** to local `state/config/api-connections.json` and probes connectivity (HTTP status + latency ms) |

Values hot-reload on Save (no restart). Disable rewrite to keep pure local dictation.

## Build from source

Requirements: Rust (MSVC), Windows SDK.

```powershell
cd F:\ainput
# Place SenseVoice bundle under models\sense-voice\ (see release pack)
cargo build --release
.\target\release\ainput.exe
```

Package a portable folder + zip:

```powershell
.\scripts\make-portable.ps1 -Version 0.1.0
```

## Privacy

- Microphone audio is processed **on device** for ASR.
- Rewrite (if enabled) sends text only to the endpoint **you** configured.
- No default SaaS gateway. Keys stay in local `state/config/`.

## Model attribution

Bundled offline ASR uses **SenseVoice** weights via **sherpa-onnx**.  
See `THIRD_PARTY_NOTICES` and the license files under `models/sense-voice/`.

## Related

- Private prototype lineage: ainput2 (not this public tree)
- Archive of a previous same-name repo: [ainput-archive-20260721](https://github.com/nakamotosai/ainput-archive-20260721)

## Contributing

Issues and PRs welcome. Keep the product local-first; do not add cloud ASR or hard-coded third-party keys.
