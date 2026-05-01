# Streaming Duplicate Final Merge V5 SPEC

更新时间：2026-04-30

## 目标

本轮第一优先级只修流式松手后的重复拼接问题。

已复现真实日志：

```text
last_hud_target_text=你最些分辨率有问
final_offline_raw_text=你这些分辨率有问题。
candidate_display_text=你最些分辨率有问你这些分辨率有问题。
```

正确结果应为：

```text
你这些分辨率有问题。
```

## 范围

- 只修流式 final merge / rollover prefix merge。
- 继续保留 `Ctrl` 按住录音、松开上屏。
- 不动非流式 `Alt+Z`。
- 不改 clipboard + Ctrl+V 输出主链路。
- 不接入 AI rewrite。
- 不把模型切换作为本问题的修复前提。

## 根因判断

这不是旧剪贴板，也不是同一 release 真的触发了两次 `Ctrl+V`。

日志显示同一 `StreamingCommitEnvelope` 里，离线 final 已经正确识别出完整句子，但 merge 逻辑把完整 final 当成“rollover 后的下一段 tail”，于是把 `rolled_over_prefix` 和完整 final 拼接在一起。

## 验收标准

- `merge_rolled_over_prefix("你最些分辨率有问", "你这些分辨率有问题。")` 必须返回 `你这些分辨率有问题。`。
- 真正的分段 tail 仍要保留拼接能力，例如 `你这些分辨率` + `有问题。` 返回 `你这些分辨率有问题。`。
- replay 真实 raw `streaming-raw-1777561249467.wav` 时，最终文本不得包含 `你最些分辨率有问你这些分辨率有问题`。
- delivered session 仍只能一轮一次，不允许为了修复文本而新增第二次上屏。
- HUD final 和最终上屏文本必须一致；输出层补的末尾句号不能只出现在目标框里。
- 当前默认流式模型继续使用小型中英双语 streaming paraformer；本轮不切到中文单语模型。
- 修复后开始跑流式延迟测试；模型候选必须中英双语优先，不能选过大模型作为默认候选。

## 本轮验证记录

- 用户失败 raw `streaming-raw-1777561249467.wav` 回放后，最终文本为 `你这些分辨率有问题。`，不再出现 `你最些分辨率有问你这些分辨率有问题。`
- 包内 raw corpus 抽样、synthetic live E2E、wav live E2E、startup idle 均已通过。
- 最新可测版本：`dist\ainput-1.0.0-preview.35\ainput-desktop.exe`。
