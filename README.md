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

- **Hold-to-talk** (default **CapsLock**; tray can set **MouseX1/X2**, F13–F24, or combos — custom choice is remembered in `state/config/hotkey-user.toml`)
- Optional **AI rewrite** (non-streaming HTTP) when you configure an API; editable rewrite prompt + light/standard presets
- **Cross-utterance context** — AI rewrite sees your last 6 dictation entries so it correctly resolves pronouns/titles across turns (e.g. 姑姑 → 她). Configurable in `config/ainput.toml [rewrite]` → `context_history_count` (0 = off)
- **Voice command** wake phrase (optional tray toggle + custom instruction prompt; wake phrase itself editable via the voice-command panel — tray → 语音指令)
- Tray icon + HUD feedback
- Local dictation history (raw vs rewrite) under `state/logs/history.jsonl`, viewed via loopback web UI
- Personal correction rules (local)
- No cloud ASR, no screen recording, no built-in vendor API key

## Official site

https://input.saaaai.com/

## Download

Latest release: see [GitHub Releases](https://github.com/nakamotosai/ainput/releases/latest).

| Channel | Link |
|---|---|
| **GitHub Releases** | https://github.com/nakamotosai/ainput/releases |
| **Hugging Face mirror** | https://huggingface.co/nakamotosai/cnjp-input |
| Site | https://input.saaaai.com/ |

## Quick start

**安装包（推荐）**：运行 `ainput-<version>-setup.exe`，按向导装好即可（含开始菜单、可选开机自启、卸载器）。
**绿色包**：下载 zip，解压到任意目录，运行 `ainput.exe` 或 `run-ainput.bat`。

> **首次运行提示**：本程序未做代码签名，Windows 可能弹出「Windows 已保护你的电脑 / SmartScreen」——点「更多信息」→「仍要运行」即可（这是未签名程序的通用提示，非病毒告警）。

1. 按住语音热键（默认 **CapsLock**）说话，松开后文字自动贴进当前窗口。托盘 → **自定义语音快捷键…** 可改键（侧键/F 键，改后重启生效）。
2. （可选）托盘 → **API / 改写设置…** 打开本地浏览器页 → 填 Key → **拉取模型** → 选模型 → 设超时 → 开启改写 → **保存并测连通**。
3. 托盘 → **听写历史…** 打开另一个本地页，查看条数与改写前后。

Both UIs bind loopback only (`http://127.0.0.1:<ephemeral-port>/`). Runtime state is stored next to the executable under `state/` (config, logs, history). History is local-only JSONL: `state/logs/history.jsonl`.

## Configure AI rewrite

Tray → **API / 改写设置…** (loopback web form, no native Win32 panel):

| Field | Default / notes |
|---|---|
| Base URL | Prefilled `https://integrate.api.nvidia.com/v1` (any OpenAI-compatible endpoint works) |
| API Key | You provide; stored only in local `state/config/` |
| Model | Type manually, or click **拉取模型** after Key is filled |
| Timeout (ms) | Default `15000` — used for rewrite, model list pull, and save probe |
| Save | Writes **API Key** to local `state/config/api-connections.json` and probes connectivity (HTTP status + latency ms) |

Values hot-reload on Save (no restart). Disable rewrite to keep pure local dictation. No Python helper process.

## Build from source

Requirements: Rust (MSVC), Windows SDK.

```powershell
cd <repo>
# Place SenseVoice bundle under models\sense-voice\ (see release pack)
cargo build --release
.\target\release\ainput.exe
```

Package a portable folder + zip:

```powershell
.\scripts\make-portable.ps1  # version auto-derived from Cargo.toml
```

## Privacy

- Microphone audio is processed **on device** for ASR.
- Rewrite (if enabled) sends text only to the endpoint **you** configured.
- No default SaaS gateway. Keys stay in local `state/config/` (plain JSON on disk).
- Dictation history is local-only at `state/logs/history.jsonl`. Each line may include full raw/rewrite text plus target process name and window title. **Do not share your `state/` folder** (keys + history). Delete `history.jsonl` or the whole `state/` tree to wipe local archives.
- Release logs do **not** contain your dictated text (only character counts). Set `AINPUT_LOG_TEXT=1` to opt in to full-text logging when debugging.
- Raw-utterance audio dumps are **off by default**; enable only for regression work with `AINPUT_DUMP_AUDIO=1` (they land under `state/logs/audio/`).
- Green release zips never include `state/`.

## Model attribution

Bundled offline ASR uses **SenseVoice** weights via **sherpa-onnx**.  
See `THIRD_PARTY_NOTICES` and the license files under `models/sense-voice/`.

## Related

- Private prototype lineage (not this public tree)
- Archive of a previous same-name repo: [ainput-archive-20260721](https://github.com/nakamotosai/ainput-archive-20260721)

## Contributing

Issues and PRs welcome. Keep the product local-first; do not add cloud ASR or hard-coded third-party keys.

