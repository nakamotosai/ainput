# Streaming Final Repair Budget V7 SPEC

更新时间：2026-04-30

## 目标

本轮只修流式松手后的 final repair 阻塞。

目标：

- `offline final` 不再对整段长音频无条件同步识别。
- 超过预算或不适合修复时，直接用 streaming final / HUD 文本提交。
- 长句只对尾部窗口做 final repair；尾部结果不能证明能修复时，不参与最终 commit。

## 非目标

- 不修 first partial / HUD 首字速度。
- 不改非流式 `Alt+Z`。
- 不改流式 `Ctrl` 按住录音、松开上屏。
- 不改 `clipboard + Ctrl+V` 上屏链路。
- 不接入 AI rewrite。
- 不切模型，不启用 GPU。

## 设计

新增 final repair 分流：

- 音频长度 `<= 6000ms`：允许完整 offline final。
- 音频长度 `> 6000ms`：只取最后 `3200ms` 音频做 offline tail repair。
- tail repair 只有在 tail 文本与 streaming final 尾部至少有 2 个内容字重叠时才合并。
- tail repair 无法可靠合并时，使用 streaming final / HUD 文本。
- offline repair 实际耗时超过 hard budget 时，忽略 repair 文本，继续 fallback。

## 验收

- 长句 replay 的 `offline_final_elapsed_ms` 应明显低于整段 SenseVoice 识别。
- `sentence_combo_long` 不能因为 tail repair 丢正文或重复拼接。
- `sentence_05` / raw 样本仍通过内容门禁。
- small bilingual zipformer 不因本轮被启用。
- 当前运行的 `preview.35` 在验证前不被替换。

## 预期影响

- 改善松开 `Ctrl` 后最终上屏速度。
- 不会明显改善“说话到 HUD 首字出现”的速度；该问题属于 first partial 链路，下一轮单独处理。
