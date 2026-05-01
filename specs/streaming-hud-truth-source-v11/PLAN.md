# Streaming HUD Truth Source V11 PLAN

更新时间：2026-05-01

## Phase 1: 事件协议

- 新增 `StreamingFinalHudCommitRequest` worker event。
- 新增 `StreamingHudCommitAck`。
- worker 在上屏前阻塞等待 HUD ack。

## Phase 2: UI/HUD ack

- UI 收到 final commit request 后，立即完整显示最终文本。
- tick HUD 到 `display_text == final_text` 且 visible。
- 返回 HUD 当前 display text。

## Phase 3: worker 上屏

- 移除固定 `48ms` 作为真实门禁的旧逻辑。
- 上屏文本改为 HUD ack text。
- ack 失败则不粘贴，避免 HUD 和上屏不一致。

## Phase 4: 验收增强

- live E2E 中把 output text 改为 HUD ack text。
- trace 增加 `hud_final_ack`。
- 保持旧的重复上屏、脏剪贴板、HUD 抖动、尾字丢失门禁。

## Phase 5: 打包与启动

- 跑 Rust 单测和包内验收。
- 打新 preview。
- 停旧进程并启动最新版到 Windows 交互桌面。
