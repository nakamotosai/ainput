# Streaming Ghost Yeah V9 PLAN

更新时间：2026-05-01

## Phase 1: 调查和基线

- 从 `preview.38` 运行日志提取 `Yeah` / `Okay` 的提交链路。
- 读取对应 raw capture，确认音频时长、rms、active_ratio 和连续活跃帧。
- 明确本轮不改非流式、不改流式 Ctrl 热键、不改粘贴链路。

## Phase 2: 实现门禁

- 在 `AudioActivity` 中增加最长连续活跃帧毫秒数。
- 调整低信号提交过滤，新增短英文填充词幻听拦截。
- 日志增加 sustained voice 指标。

## Phase 3: 测试

- 增加单测覆盖 `Yeah.` / `Okay.` 低信号 drop。
- 增加单测确认中英混合和清晰语音不会被误杀。
- 跑 streaming / hotkey / shell / output / rewrite 回归。

## Phase 4: 打包和真实验收

```powershell
cargo fmt --check
cargo check -p ainput-desktop
cargo test -p ainput-desktop streaming -- --nocapture
cargo test -p ainput-desktop hotkey -- --nocapture
cargo test -p ainput-shell
cargo test -p ainput-output
cargo test -p ainput-rewrite
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-startup-idle-acceptance.ps1 -Version <new>
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Version <new> -Synthetic -InteractiveTask
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Version <new> -Wav -InteractiveTask -CaseLimit 3
```

## Phase 5: 收口

- 更新本轮 `TASKLIST.md` / `RESULTS.md`。
- 更新根 `TASKLIST.md`。
- 停止旧进程，启动最新 preview 到 Windows 交互桌面。
