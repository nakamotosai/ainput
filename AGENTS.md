# ainput — contributor notes

## 铁律：每轮改动必须存档（2026-09-07 立规）
- 改 src/ 任何代码前：先把当前版本复制到 F:\archive（日期主题子目录）备份——本项目是长项目，没有后悔药不行
- 改完、`cargo build --release` + `cargo test` 通过后，**当轮必须 git commit**，绝不攒着跨会话
- 每次提交同步把 Cargo.toml 的 version 抬一档（如 0.1.4-preview → 0.1.5-preview），保证「exe 版本号 = git 存档点」一一对应
- deploy.ps1 部署后验三样：Path = 根目录 exe、时间戳 = 本次构建、版本号 = 本次提交

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
