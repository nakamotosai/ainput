# ainput Health Check

Release and regression health-check plan for the public product **ainput** (`F:\ainput` / green zip).

## Goals

- Prove package, process, local state, API config, history, HUD, rewrite, and paste are consistent.
- Catch regressions in hotkeys, empty sessions, HUD, rewrite, target paste.
- Split automated checks from manual Windows desktop acceptance.

## Quick command

```powershell
cd F:\ainput
cargo test
.\scripts\make-portable.ps1 -Version 0.1.0
```

## Automated

| Check | How |
|-------|-----|
| Unit/integration | `cargo test` — expect all green |
| Brand | No user-facing `ainput2` in tray/UI strings |
| Package | Zip has `ainput.exe`, `models/sense-voice/**/*.onnx`, `LICENSE`, `THIRD_PARTY_NOTICES`, **no** `state/` |
| Config example | `config/api-connections.example.json` has empty key |

## Manual desktop (release candidate)

1. Cold start from unpacked zip (empty `state/`).
2. Hold **CapsLock**, speak, release → text pastes into focused app.
3. Tray → **API / 改写设置…** opens loopback browser page → Key → **拉取模型** (must not crash) → pick model → **保存并测连通** (HTTP + latency; key on disk).
4. Enable rewrite, dictate once → tray **听写历史…** opens loopback browser page with before/after when rewrite completed.
5. Kill process cleanly via tray **退出**.

## Logs

- Runtime log: `state\logs\ainput.log*`
- History: `state\logs\history.jsonl` (local-only; contains full text + target app title)

## Notes

- Private prototype tree `C:\Users\sai\ainput2` is **not** this product; do not dual-run CapsLock hooks.
- Startup failures in Release show a MessageBox and write the error to the log when possible.
