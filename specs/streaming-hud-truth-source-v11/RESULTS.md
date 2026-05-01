# Streaming HUD Truth Source V11 RESULTS

更新时间：2026-05-01

## 结论

已把流式最终提交改成 “HUD 是最终真相源”：

- worker 不再固定等 `48ms` 后直接贴内部 `commit_text`。
- worker 创建 final envelope 后，先向 UI 请求 HUD final ack。
- UI 立即把最终文本完整显示到 HUD，不走逐字动画。
- UI 确认 HUD `display_text == final_text` 且可见后，回传 HUD 当前显示文本。
- worker 只用 HUD ack text 上屏。
- streaming 输出层启用 exact delivery，HUD ack 后不再被输出层自动补标点、删标点或术语修正。

## 关键修正

`preview.42` 验收时发现一个二级问题：

- HUD ack text：`然后，不管我说多少个字，它永远只能显示出来两个字。`
- output request text：同上
- target readback：少了末尾 `。`

根因是 `ainput-output` 在投递前根据编辑上下文再次调整文本，破坏了 HUD 真相源。`preview.43` 已增加 `preserve_text_exactly`，流式 HUD ack 后不再改写输出文本。

## 验收

- `cargo fmt --check`：通过
- `cargo check -p ainput-desktop`：通过
- `cargo test -p ainput-desktop final_commit -- --nocapture`：4/4 通过
- `cargo test -p ainput-desktop streaming -- --nocapture`：31/31 通过
- `cargo test -p ainput-desktop hotkey -- --nocapture`：6/6 通过
- `cargo test -p ainput-desktop -- --nocapture`：86/86 通过
- `cargo test -p ainput-output -- --nocapture`：9/9 通过
- `cargo test -p ainput-shell -- --nocapture`：6/6 通过
- `cargo test -p ainput-rewrite -- --nocapture`：16/16 通过
- `run-startup-idle-acceptance.ps1 -Version 1.0.0-preview.43`：通过
- `run-streaming-live-e2e.ps1 -Version 1.0.0-preview.43 -Synthetic`：3/3 通过
- `run-streaming-live-e2e.ps1 -Version 1.0.0-preview.43 -Wav -CaseLimit 3`：3/3 通过
- `run-streaming-raw-corpus.ps1`：2/2 通过

## 证据

`preview.43` wav E2E `sentence_03`：

- `hud_final_ack`：`然后，不管我说多少个字，它永远只能显示出来两个字。`
- `output_commit_request`：同上
- `target_readback`：同上

## 产物

- `dist\ainput-1.0.0-preview.43`
- `dist\ainput-1.0.0-preview.43.zip`

## 当前运行

- 已启动：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.43\ainput-desktop.exe`
- PID：`4412`
