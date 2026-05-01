# Streaming First Partial Latency V14 RESULTS

更新时间：2026-05-01

## 当前状态

- 已完成。
- 交付版本：`1.0.0-preview.48`
- 非目标：AI 语义改写、GPU、非流式 `Alt+Z`、上屏链路重写。

## 初始发现

- `preview.46` v13 latency 中，fixture 三条首个 partial 都是 `660ms`，拖高 p95 的是真实 raw：
  - `raw_01_streaming-raw-1777611420719`: `first_partial_ms=1860`
  - `raw_02_streaming-raw-1777612195536`: `first_partial_ms=1260`
- `zipformer_small_bilingual` 处理墙钟更快，但 5 条中 3 条失败，不能切默认模型：
  - `sentence_01`: partial updates 不足。
  - `sentence_05`: 内容明显短缺。
  - `sentence_combo_long`: 长句内容明显短缺。

## 修复

- `StreamingReplayReport` 增加：
  - `speech_start_ms`
  - `first_partial_after_speech_ms`
  - `first_partial_processing_elapsed_ms`
  - `first_partial_processing_lag_ms`
- `worker.rs` 增加 `estimate_speech_start_ms`，按 20ms frame 和连续语音帧估算真实开始说话时间。
- `scripts\run-streaming-latency-benchmark.ps1` 增加 speech-start 指标输出、summary CSV/JSON/Markdown 字段，并修复旧报告缺字段兼容与 `0ms` 被误判为空值的问题。
- `scripts\run-streaming-full-audit.ps1` 的 latency P2 规则改为优先看 `speech_start -> first_partial`：
  - p95 > 1200ms 才报 P2。
  - avg > 800ms 才报 P2。
  - 无新字段时才回退旧 `audio_start -> first_partial`。

## 验收

- 代码门禁：
  - `cargo check -p ainput-desktop` pass
  - `cargo test -p ainput-desktop streaming -- --nocapture` pass
  - `cargo test -p ainput-desktop hotkey -- --nocapture` pass
  - `cargo test -p ainput-rewrite -- --nocapture` pass
- 发包：
  - `dist\ainput-1.0.0-preview.48\`
  - `dist\ainput-1.0.0-preview.48.zip`
- 全量审计：
  - 命令：`.\scripts\run-streaming-full-audit.ps1 -Version 1.0.0-preview.48 -LatencyRepeats 1 -LiveCaseLimit 3`
  - 报告：`tmp\streaming-full-audit\20260501-223721-227\full-audit-report.json`
  - 结果：`overall_status=pass`，P0=0，P1=0，P2=0。
- 速度结果：
  - `paraformer_bilingual_asr6_chunk80`: failed=0/5，`speech->first avg=588ms / p50=540ms / p95=900ms`。
  - `paraformer_bilingual_asr6_chunk60`: failed=0/5，`speech->first avg=600ms / p50=540ms / p95=920ms`。
  - `chunk80` 比默认 `chunk60` 只快约 12ms avg / 20ms p95，收益太小，本轮不改默认配置。
- 最小 smoke：
  - `tmp\streaming-latency-v14-smoke\summary.json` pass。
  - `first_partial_processing_lag_avg_ms=0`，确认 summary 不再把 `0ms` 当空值。
- 启动：
  - `C:\Users\sai\ainput\dist\ainput-1.0.0-preview.48\ainput-desktop.exe` 已启动到 Windows 交互桌面，PID `38548`。
