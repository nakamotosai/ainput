# Streaming HUD Realtime Latency V8 PLAN

更新时间：2026-05-01

## Phase 1: 文档和基线

- 新增 v8 Spec / Plan / Tasklist。
- 读取 `raw_tail_late` 失败样本的 partial timeline。
- 确认本轮只改流式 HUD 链路。

## Phase 2: 实现 soft flush

- 在 streaming endpoint config 增加 `soft_flush_ms`。
- 在 worker 中增加按住期间尾部 soft flush。
- soft flush 刷 HUD 后 reset recognizer，但不提交、不加句末标点。
- replay partial timeline 增加 `idle_soft_flush` source。

## Phase 3: 指标和门禁

- partial timeline 增加处理耗时字段。
- raw corpus summary 增加最后 HUD/final 差距字段。
- latency benchmark CSV 增加 final-vs-HUD 差距字段。

## Phase 4: 验证和打包

```powershell
cargo fmt --check
cargo check -p ainput-desktop
cargo test -p ainput-desktop streaming -- --nocapture
cargo test -p ainput-shell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-selftest.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-raw-corpus.ps1 -ShortCount 1 -LongCount 1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Version <new> -Synthetic -InteractiveTask
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Version <new> -Wav -InteractiveTask -CaseLimit 3
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-startup-idle-acceptance.ps1 -Version <new>
```

## Phase 5: 收口

- 写 `RESULTS.md`。
- 更新根 `TASKLIST.md`。
- 打包新 preview 并启动到 Windows 交互桌面。
