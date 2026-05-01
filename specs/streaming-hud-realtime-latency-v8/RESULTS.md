# Streaming HUD Realtime Latency V8 RESULTS

更新时间：2026-05-01

## 结果

已完成本轮 HUD 实时追帧修复：

- 新增 `voice.streaming.endpoint.soft_flush_ms = 360`。
- 按住 `Ctrl` 时，如果已有 HUD 文本、最近短暂停音、且尾部可能滞后，会触发 `idle_soft_flush`。
- `idle_soft_flush` 只刷新 HUD，不上屏，不强制句末标点。
- replay/live 报告新增 partial 处理耗时、final-vs-HUD 内容字差距。
- replay timeline 补齐真实程序已有的 release 前 final HUD sync，source 为 `release_final_preview`。

## 关键验证

- 源码态 raw corpus：`tmp\streaming-raw-corpus\20260501-021906-777`，2/2 pass，`final_extra_chars=0`
- 包内 raw corpus：`tmp\streaming-raw-corpus\20260501-022325-003`，2/2 pass，`final_extra_chars=0`
- latency smoke：`tmp\streaming-latency-benchmark\20260501-022008-231`
  - `failed_cases=0`
  - `first_partial_avg_ms=660`
  - `final_extra_chars_max=0`
- 手工构造 “raw + 1 秒静音” 测试：timeline 出现 `idle_soft_flush`，证明按住但不说话时会提前刷新尾部 HUD。

## 已通过

- `cargo fmt --check`
- `cargo check -p ainput-desktop`
- `cargo test -p ainput-desktop streaming -- --nocapture`：31/31 pass
- `cargo test -p ainput-shell`：6/6 pass
- `cargo test -p ainput-output`：9/9 pass
- `cargo test -p ainput-rewrite`：16/16 pass
- `scripts\run-streaming-selftest.ps1`：6/6 pass
- 包内 startup idle：`tmp\startup-idle-acceptance\20260501-022233-239`，pass
- 包内 synthetic live E2E：`tmp\streaming-live-e2e\20260501-022247-075`，3/3 pass
- 包内 wav live E2E：`tmp\streaming-live-e2e\20260501-022258-698`，3/3 pass

## 打包

- 成功包：`dist\ainput-1.0.0-preview.38`
- 成功 zip：`dist\ainput-1.0.0-preview.38.zip`
- 当前已启动：`dist\ainput-1.0.0-preview.38\ainput-desktop.exe`，PID `60120`

## 残留

- `first_partial_ms` 仍是固定 fixture 上约 `660ms`。本轮没有证明首字慢来自标点、CPU 线程或等待间隔；当前更像 streaming ASR 首次可用文本节奏。下一轮若继续提首字速度，应单独比较模型/解码策略。
