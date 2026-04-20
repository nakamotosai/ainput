# Streaming Worker V2 SPEC

## 目标

把 `ainput` 的流式语音模式改成“在线流负责 HUD 预览 + 同模型整段 rescore 负责最终提交”的稳态方案，并同时压缩松手后的最终提交延迟，让长句不再只显示前几个字。

## 范围

- 只处理 `流式语音识别` 模式。
- 保留 `极速语音识别` 现有链路，不改模型、不替换原模式。
- 保留现有 HUD 入口、托盘入口、配置入口和术语/学习机制。
- 本轮不做 HUD 视觉重设计，不做学习系统重构。

## 问题事实

- 当前 `apps/ainput-desktop/src/worker.rs` 的流式 worker 仍使用 `SenseVoiceRecognizer` 对累计录音反复整段重跑。
- `crates/ainput-asr/src/lib.rs` 已经存在 `StreamingZipformerRecognizer` 和 `StreamingZipformerStream`，但桌面流式 worker 没接上。
- 在线 zipformer partial 在真实口述里会出现长句严重截断，导致 HUD 和最终提交都只剩前几个字。
- 松手后的最终提交目前还缺少稳定的同模型整段 rescore 策略，导致在线结果一旦偏短，就会直接把残句贴上屏。

## 目标行为

- 按住热键时，worker 持续采集增量音频并喂给 `StreamingZipformerRecognizer` 在线流式解码。
- HUD 预览继续只显示单行文本，不增加多余状态行。
- HUD 预览优先使用在线流；当在线 partial 明显偏短时，按节流频率触发同模型整段 preview rescue。
- 松手时不再重新切回离线 `SenseVoice` 全量识别；最终提交统一使用同一个 streaming zipformer 模型对整段采样做 final rescore。
- 最终提交继续沿用现有文本整理、术语修正、上下文标点策略和直贴/剪贴板降级。
- 为流式模式单独压缩上屏等待，避免沿用偏保守的固定延迟。

## 约束

- 继续使用 Rust 实现。
- 继续使用当前本地模型目录和 sherpa-onnx Rust 封装。
- 不引入 Ollama、不引入额外在线服务。
- 改动尽量限制在流式 worker 和输出链路，避免牵连极速模式。

## 验收

- 流式 worker 编译通过，相关测试或等价验证通过。
- 日志里能看出流式链路已切到在线解码，并记录 `final_drain` / `rewrite` / `output` 等关键耗时。
- 代码层面不再存在“流式模式松手后再用 `SenseVoiceRecognizer` 全量转写累计音频”的路径。
- 日志里能区分 `online_raw_text`、`final_rescore_text`、`preview_rescue_count`，方便定位长句截断。
- 流式模式的最终上屏等待显著缩短：固定粘贴稳定等待单独瘦身，热键释放等待不再沿用旧的 300ms 常量。
- README、错题本和回归脚本同步回写本轮关键事实与风险。
