# Streaming HUD Truth Source V11 SPEC

更新时间：2026-05-01

## 目标

让流式模式遵守“HUD 是最终真相源”：

- 松开 `Ctrl` 后，HUD 不能立刻消失。
- final ASR / 标点 / 尾部修正全部完成后，最终文本必须先完整显示在 HUD。
- 只有 HUD 已完整显示最终文本并返回确认后，才允许上屏。
- 最终粘贴到聊天框的文本必须等于 HUD 确认后的 display text。

## 非目标

- 不改非流式 `Alt+Z`。
- 不改流式 `Ctrl` 热键规则。
- 不改 `clipboard + Ctrl+V` 上屏主链路。
- 不接 AI rewrite。
- 不换模型、不启 GPU。

## 当前问题

`preview.41` 仍然是 worker 内部文本直接上屏：

1. worker 算出 `commit_text`。
2. worker 发一次 final preview 给 HUD。
3. 固定等待 `48ms`。
4. worker 直接把内部 `commit_text` 上屏。

这意味着 HUD 只是预览，不是最终提交依据；如果 HUD 还没完整显示、还在逐字动画、或 UI 事件还没处理完，就可能出现“HUD 看到的”和“实际上屏的”不一致。

## 目标协议

新协议：

1. worker 创建 final commit envelope。
2. worker 锁定本轮 commit，拒绝迟到 partial/final mutation。
3. worker 向 UI 发送 `StreamingFinalHudCommitRequest(final_text, response_tx)`。
4. UI 立即把 `final_text` 作为完整最终文本显示到 HUD，不走逐字动画。
5. UI 连续 tick，直到 HUD `display_text == final_text` 且窗口可见。
6. UI 返回 `StreamingHudCommitAck { text: hud_display_text, visible, elapsed_ms }`。
7. worker 只用 ack 的 `text` 上屏。
8. 如果 HUD ack 超时或显示文本不一致，本轮不粘贴，报错并保留日志。

## 验收

- 自动测试必须覆盖：`hud_final_ack` 先于 `output_commit_request`。
- `hud_final_display == output_commit_request.text == target_readback`。
- final HUD 不走逐字流式动画，必须立即完整显示。
- `post_hud_flush_mutation_count == 0`。
- 每轮只有一次 commit envelope、一次 output commit request。
- 现有 `streaming` / `hotkey` / `final_commit` 回归通过。
- 包内 startup idle / synthetic / wav / raw corpus 通过。
- 打新 preview，不覆盖旧版，并启动最新版。
