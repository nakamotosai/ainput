# Streaming First Partial Latency V14 SPEC

更新时间：2026-05-01

## 目标

把“从说话到 HUD 出字”的速度问题从旧的 `audio_start -> first_partial` 指标，修正为更接近真实体验的 `speech_start -> first_partial` 指标，并用真实模型对比决定是否切小模型。

## 范围

- 只处理流式输出速度测量、模型对比和验收门禁。
- 不接 AI 语义改写。
- 不启用 GPU。
- 不改非流式 `Alt+Z`。
- 不改剪贴板 + `Ctrl+V` 主上屏链路。
- 不改变当前稳定的 HUD final truth source / exact delivery 行为。

## 必须验证的问题

1. `preview.46` 的 `first_partial_p95=1860ms` 是否主要来自 raw 文件开头静音或弱语音。
2. 当前 paraformer bilingual 的真实 `speech_start -> first_partial` 是否仍然慢。
3. `streaming-zipformer-small-bilingual-zh-en` 是否能在中英双语和长句上通过内容门禁。
4. 如果小模型快但内容失败，不能切默认模型，只能记录为 rejected candidate。

## 完成标准

- replay report 增加：
  - `speech_start_ms`
  - `first_partial_after_speech_ms`
  - `first_partial_processing_elapsed_ms`
  - `first_partial_processing_lag_ms`
- latency benchmark 的 cases / summary / markdown 输出上述新指标。
- full audit 的 P2 速度判定优先使用 `first_partial_after_speech_*`，没有新字段时才回退到旧 `first_partial_*`。
- 跑 paraformer bilingual vs zipformer small bilingual 对比，并记录结论。
- 若有代码/脚本改动，必须打新 preview，不覆盖 `preview.46`。
