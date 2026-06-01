# Streaming Tail Overlap Dedup V10 PLAN

更新时间：2026-05-01

## Phase 1: 证据确认

- 从 `preview.40` 日志确认重复不是两次 paste，而是一次 commit 文本内部重复。
- 锁定真实失败样本 `streaming-86`。

## Phase 2: 实现

- 新增 fuzzy suffix-prefix overlap 合并函数。
- 在 final commit 选择前修复 `display + duplicated tail`。
- 增加日志，标记发生了 tail overlap repair。

## Phase 3: 测试

- 单测覆盖真实失败样本。
- 单测确认正常 segment tail 追加不被误杀。
- 跑 streaming / hotkey / 全量 desktop 回归。

## Phase 4: 打包验收

```powershell
cargo fmt --check
cargo check -p ainput-desktop
cargo test -p ainput-desktop streaming -- --nocapture
cargo test -p ainput-desktop hotkey -- --nocapture
cargo test -p ainput-desktop
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-startup-idle-acceptance.ps1 -Version <new> -IdleSeconds 30 -Runs 1 -InteractiveTask
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Version <new> -Synthetic -InteractiveTask
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Version <new> -Wav -InteractiveTask -CaseLimit 3
```

## Phase 5: 收口

- 写 `RESULTS.md`。
- 更新本轮 tasklist 和根 `TASKLIST.md`。
- 停旧进程，启动最新 preview。
