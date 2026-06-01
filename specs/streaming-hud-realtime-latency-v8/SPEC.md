# Streaming HUD Realtime Latency V8 SPEC

更新时间：2026-05-01

## 目标

本轮只修流式 HUD 实时性：

- 说话到 HUD 首字/首段出现的链路必须有可解释的 timeline 指标。
- 按住 `Ctrl` 不松时，尾部内容不能长期等到 release/final 才显示。
- raw replay 不能再出现 final 比最后 HUD partial 多明显内容字的 `raw_tail_late`。

## 非目标

- 不改非流式 `Alt+Z`。
- 不改流式按住 `Ctrl` 录音、松开上屏。
- 不改 `clipboard + Ctrl+V` 上屏链路。
- 不接入 AI rewrite。
- 不换模型，不启用 GPU。
- 不把停顿当成句子结束；停顿只允许触发 HUD 尾部 flush，不允许强制补句末标点。

## 设计

### 1. Timeline 指标

在 replay/live 报告中补充更细的 HUD 时序：

- 每条 partial 的处理耗时。
- 首次 partial 的 audio offset。
- 最后 HUD partial 到 final 的内容字差距。
- final 比最后 HUD 多出的内容字数量。

### 2. 按住期间尾部 soft flush

新增 `voice.streaming.endpoint.soft_flush_ms`：

- 默认 `360ms`。
- 只在已有足够 HUD 内容、最近没有明显语音、且距离上次 HUD 更新已有一小段时间时触发。
- soft flush 会给 streaming recognizer 补短静音并 `input_finished()`，读取尾部识别结果后刷新 HUD。
- soft flush 后 reset streaming recognizer，让后续继续说的话作为下一段接上已有 HUD 文本。
- soft flush 不上屏，不提交 commit，不强制句末标点。

### 3. 首字/首段速度

本轮不盲目换模型。先把 `first_partial_ms` 和 partial timeline 固化进测试报告；如果 soft flush 不影响首字速度，结果中必须明确写出瓶颈仍在 streaming ASR 首次可用文本。

## 验收

- `cargo test -p ainput-desktop streaming -- --nocapture` 通过。
- `cargo test -p ainput-shell` 通过。
- `scripts\run-streaming-selftest.ps1` 通过。
- raw corpus 至少短句 1 条、长句 1 条通过，不能出现 `raw_tail_late`。
- live E2E synthetic / wav 通过，HUD 不闪、不抖、不回白面板。
- startup idle 通过，不按热键不自动识别。
- 打新 preview，旧 preview 可回滚，最终启动最新版到 Windows 交互桌面。

## 风险控制

- soft flush 最小内容字数限制，避免一个字/两个字就被切段。
- soft flush 不使用标点模型判断句末。
- commit exactly-once 仍由既有门禁覆盖。
