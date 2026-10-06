## ainput 0.1.3

Green zip for Windows x64 — unpack and run. **No installer.** Default hold key remains **CapsLock**; your custom key is remembered in local `state/config/hotkey-user.toml`.

### Highlights
- **Cross-utterance context** — AI rewrite sees your last 6 dictation entries so it resolves pronouns/titles correctly across turns (姑姑 → 她). Configure in `config/ainput.toml [rewrite]` → `context_history_count` (0 = off)
- **Editable wake phrase** — the voice-command wake phrase itself can be changed in the panel (tray → 语音指令 → 唤醒词). Leave blank to keep the default 老蔡老蔡; a custom phrase matches exactly (spaces inside tolerated)
- 41 build warnings cleaned up along the way; 128 tests green
- Local SenseVoice model still bundled; no cloud ASR, no built-in vendor API key

### Download
| Channel | File |
|---|---|
| This release | `ainput-0.1.3-win64.zip` |
| HF mirror | https://huggingface.co/nakamotosai/cnjp-input/resolve/main/ainput-0.1.3-win64.zip |
| Site | https://input.saaaai.com/ |

### Verify
```
SHA256 ainput-0.1.3-win64.zip
65dd2bd77edd98083e034b463167eaa4119ec9dad60f90713134d92fd1ecba01
```

Do **not** share your `state/` folder after running (keys + history).
