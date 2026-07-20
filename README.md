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
- Tray icon, HUD feedback, local web console (loopback only)
- Personal correction rules (local)
- No cloud ASR, no screen recording, no built-in vendor API key

## Quick start

1. Download the release zip (or build from source).
2. Unpack anywhere.
3. Run `ainput.exe` or `run-ainput.bat`.
4. Hold **CapsLock**, speak, release — text pastes into the focused window.
5. (Optional) Open tray → console → set OpenAI-compatible **base URL**, **API key**, **model**, enable rewrite.

Runtime state is stored next to the executable under `state/` (config, logs, history).

## Configure AI rewrite

Copy `config/api.example.json` to `state/config/api.json` (created on first run) and fill:

```json
{
  "version": 1,
  "openai_compatible": {
    "base_url": "https://api.example.com",
    "api_key": "sk-...",
    "chat_completions_path": "/v1/chat/completions",
    "models_path": "/v1/models"
  },
  "rewrite": {
    "enabled": true,
    "model": "your-model-id",
    "timeout_ms": 5000
  }
}
```

Empty `base_url` / key keeps pure local dictation (rewrite off).

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
