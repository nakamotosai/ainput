# Streaming Worker V2 SPEC

## 目标

把 `ainput` 的流式语音模式从“累计音频反复跑离线整段识别”改成真正的在线流式解码，并同时压缩松手后的最终提交延迟，让 HUD 上已经看到的文字能在松手后接近瞬时上屏。

## 范围

- 只处理 `流式语音识别` 模式。
- 保留 `极速语音识别` 现有链路，不改模型、不替换原模式。
- 保留现有 HUD 入口、托盘入口、配置入口和术语/学习机制。
- 本轮不做 HUD 视觉重设计，不做学习系统重构。

## 问题事实

- 当前 `apps/ainput-desktop/src/worker.rs` 的流式 worker 仍使用 `SenseVoiceRecognizer` 对累计录音反复整段重跑。
- `crates/ainput-asr/src/lib.rs` 已经存在 `StreamingZipformerRecognizer` 和 `StreamingZipformerStream`，但桌面流式 worker 没接上。
- 松手后的最终提交目前包含二次整段 ASR、热键释放等待和粘贴稳定等待，导致 HUD 已经出字但最终上屏还有明显延迟。

## 目标行为

- 按住热键时，worker 持续采集增量音频并喂给 `StreamingZipformerRecognizer` 在线流式解码。
- HUD 预览继续只显示单行文本，不增加多余状态行。
- 松手时不再重新对整段音频做离线全量识别，只做在线流的 `input_finished + drain` 收尾。
- 最终提交继续沿用现有文本整理、术语修正、上下文标点策略和直贴/剪贴板降级。
- 为流式模式单独压缩上屏等待，避免沿用偏保守的固定延迟。

## 约束

- 继续使用 Rust 实现。
- 继续使用当前本地模型目录和 sherpa-onnx Rust 封装。
- 不引入 Ollama、不引入额外在线服务。
- 改动尽量限制在流式 worker 和输出链路，避免牵连极速模式。

## 验收

- 流式 worker 编译通过，相关测试通过。
- 日志里能看出流式链路已切到在线解码，并记录 `final_drain` / `rewrite` / `output` 等关键耗时。
- 代码层面不再存在“流式模式松手后再用 `SenseVoiceRecognizer` 全量转写累计音频”的路径。
- 流式模式的最终上屏等待显著缩短：固定粘贴稳定等待单独瘦身，热键释放等待不再沿用旧的 300ms 常量。
- README 和错题本同步回写本轮关键事实与风险。
