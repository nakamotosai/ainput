## ainput 0.1.2

Green zip for Windows x64 — unpack and run. **No installer.** Default hold key remains **CapsLock**; your custom key is remembered in local `state/config/hotkey-user.toml`.

### Highlights
- **Custom voice hotkey**: CapsLock / MouseX1 / MouseX2 / F13–F24 / combos (tray → 自定义语音快捷键…, restart after save)
- **Mouse side buttons**: system Back/Forward stripped via `WH_MOUSE_LL` (same idea as CapsLock)
- **Safer AI rewrite**: short-wipe guard, light preset alignment, editable rewrite prompt
- **Voice command** (optional): wake phrase + editable instruction prompt from tray
- Local SenseVoice model still bundled; no cloud ASR, no built-in vendor API key

### Download
| Channel | File |
|---|---|
| This release | `ainput-0.1.2-win64.zip` |
| HF mirror | https://huggingface.co/nakamotosai/cnjp-input/resolve/main/ainput-0.1.2-win64.zip |
| Site | https://input.saaaai.com/ |

### Verify
```
SHA256 ainput-0.1.2-win64.zip
e6393cd99ccb92a1d268e5cc75a3c9d0a0ef8ccc5ab25eb57c0cda2daa0e5ef4
```

Do **not** share your `state/` folder after running (keys + history).
