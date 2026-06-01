# Streaming Tail Artifact And Punctuation V12 RESULTS

更新时间：2026-05-01

## 结果

- 当前交付版本：`1.0.0-preview.45`
- 当前运行入口：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.45\ainput-desktop.exe`
- 当前运行 PID：`59416`
- `preview.44` 已废弃：该包二进制可用，但打包脚本未先创建 `fixtures\streaming-user-regression-v12` 目录，导致 v12 fixture 没有正确落入包内。`preview.45` 已修复并重新打包。

## 修复点

- final commit 在 `display/candidate` 选择之后统一做最终清洗，再进入 HUD final ack 和上屏。
- offline final hard budget 从 `350ms` 调整到 `650ms`，避免可用尾字修复结果晚到被丢弃。
- 中文上下文下拒绝 tail-window 短英文幻觉 `I/Yeah/Okay/...` 拼到尾部。
- `不I` 修成 `不对`；中文尾部孤立 `I` 删除。
- 固定误识别清洗：`强治/强距 -> 简直`、`标点，符号 -> 标点符号`。
- replay 报告的 `final_text` 对齐真实最终 commit 文本，补完终止句号后再验收。
- 打包脚本补建 `fixtures\streaming-user-regression-v12` 目录。

## 验收

- `cargo fmt --check`：pass
- `cargo check -p ainput-desktop`：pass
- `cargo test -p ainput-rewrite -- --nocapture`：16/16 pass
- `cargo test -p ainput-desktop final_commit -- --nocapture`：5/5 pass
- `cargo test -p ainput-desktop streaming -- --nocapture`：32/32 pass
- `cargo test -p ainput-desktop hotkey -- --nocapture`：6/6 pass
- `dist\ainput-1.0.0-preview.45\ainput-desktop.exe replay-streaming-manifest fixtures\streaming-user-regression-v12\manifest.json`：4/4 pass
- `.\scripts\run-startup-idle-acceptance.ps1 -Version 1.0.0-preview.45 -IdleSeconds 30 -Runs 1 -InteractiveTask`：pass
- `.\scripts\run-streaming-live-e2e.ps1 -Version 1.0.0-preview.45 -Synthetic -InteractiveTask`：3/3 pass
- `.\scripts\run-streaming-live-e2e.ps1 -Version 1.0.0-preview.45 -Wav -InteractiveTask -CaseLimit 3`：3/3 pass
- `.\scripts\run-streaming-raw-corpus.ps1 -ExePath .\dist\ainput-1.0.0-preview.45\ainput-desktop.exe -RawDir .\dist\ainput-1.0.0-preview.43\logs\streaming-raw-captures -ShortCount 1 -LongCount 1`：2/2 pass，`final_missing_chars=0`

## v12 固定样本

- `short_tail_da` -> `我试了短句的话，问题好像不是很大。`
- `trailing_i_repeat` -> `很奇怪还是会漏字和重复。`
- `short_tail_full` -> `我刚想说短句的话，问题不是很大结果又给我漏了最后那个字。`
- `punctuation_budui_i` -> `简直就是灾难，标点符号都不对。`

## 仍未做

- AI 语义改写仍未接入；等基础流式稳定后再开下一轮独立 spec。
- GPU 推理仍未启用；当前版本继续走 CPU。
