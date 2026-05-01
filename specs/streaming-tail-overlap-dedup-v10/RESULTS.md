# Streaming Tail Overlap Dedup V10 RESULTS

更新时间：2026-05-01

## 结论

已修复 `preview.40` 中“看起来双重上屏”的一类明确根因：final commit 阶段把 HUD 已显示尾巴和 offline final tail 直接拼接，导致一次上屏文本内部重复。

关键样本 `streaming-86`：

- HUD / display：`我都已经设置了多跳思考的`
- offline final tail：`设置了多跳思考了。`
- 旧输出：`我都已经设置了多跳思考的设置了多跳思考了。`
- 新规则目标输出：`我都已经设置了多跳思考了。`

## 改动

- 在流式 final commit 文本选择阶段增加 fuzzy suffix-prefix overlap repair。
- 当 `display + appended_tail` 的拼接边界存在 4 字以上重叠，且最多只有 1 个字不同，改为尾部替换而不是硬追加。
- 保留正常追加场景，例如 `你这些分辨率` -> `你这些分辨率有问题。`。
- 不改非流式 `Alt+Z`。
- 不改流式 `Ctrl` 热键。
- 不改 `clipboard + Ctrl+V` 上屏主链路。

## 验收结果

- `cargo fmt --check`：通过
- `cargo check -p ainput-desktop`：通过
- `cargo test -p ainput-desktop final_commit -- --nocapture`：4/4 通过
- `cargo test -p ainput-desktop streaming -- --nocapture`：31/31 通过
- `cargo test -p ainput-desktop hotkey -- --nocapture`：6/6 通过
- `cargo test -p ainput-desktop -- --nocapture`：86/86 通过
- `cargo test -p ainput-shell -- --nocapture`：6/6 通过
- `cargo test -p ainput-output -- --nocapture`：9/9 通过
- `cargo test -p ainput-rewrite -- --nocapture`：16/16 通过
- `run-startup-idle-acceptance.ps1 -Version 1.0.0-preview.41`：通过
- `run-streaming-live-e2e.ps1 -Version 1.0.0-preview.41 -Synthetic`：3/3 通过
- `run-streaming-live-e2e.ps1 -Version 1.0.0-preview.41 -Wav -CaseLimit 3`：3/3 通过
- `run-streaming-raw-corpus.ps1`：2/2 通过

## 产物

- `dist\ainput-1.0.0-preview.41`
- `dist\ainput-1.0.0-preview.41.zip`

## 当前运行

- 已启动：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.41\ainput-desktop.exe`
- PID：`68884`
