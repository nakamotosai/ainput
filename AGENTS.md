# ainput — contributor notes

## Product name
**ainput** (not ainput2).

## Build
cargo build --release
cargo test

## Package
.\scripts\make-portable.ps1 -Version 0.1.0 -Overwrite

## Rules
- Local SenseVoice only (no cloud ASR)
- No screen recording
- No hard-coded third-party API keys
- Rewrite = user OpenAI-compatible base_url + api_key + model
- Suspect-term auto analysis out of scope for now
- Do not modify C:\Users\sai\ainput2
- 运行位置/部署形态变更（dist ↔ target/release 等）= 视同新交付物：托盘图标、HUD、模型加载等用户可见面必须逐项验证后才能报完成（2026-08-25 托盘图标丢失教训，详见 .scold/ledger.md）
