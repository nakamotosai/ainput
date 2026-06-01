# Streaming Ghost Yeah V9 SPEC

更新时间：2026-05-01

## 目标

修掉流式输入偶发的幽灵英文短词上屏：

- 用户没有有效语音时，不能把低信号音频识别成 `Yeah` / `Okay` 之类并提交。
- 误触发或极短触发即使产生 raw capture，也必须在最终上屏前被拦截。
- 保持流式交互不变：按住 `Ctrl` 录音，松开 `Ctrl` 上屏。

## 非目标

- 不改非流式 `Alt+Z`。
- 不改流式 `Ctrl` 热键规则。
- 不改 `clipboard + Ctrl+V` 上屏主链路。
- 不接入 AI rewrite。
- 不换模型，不启 GPU。

## 已确认现象

`preview.38` 日志中出现 4 条同类幽灵提交：

- `Yeah 。` 两次，音频时长 `570-600ms`。
- `Okay 。` 两次，音频时长 `560-570ms`。
- 这些 session 的 HUD 文本为空，最终由 `offline_final` 直接给出英文短词。
- raw 音频 rms/active_ratio 很低，20ms 活跃帧最长连续区间只有约 `20-60ms`，更像键盘/环境短脉冲，不像持续语音。

## 设计

### 1. 提交前短英文幻听门禁

在现有低信号过滤上增加一个专门规则：

- 文本只包含 ASCII 英文短填充词，例如 `yeah`、`okay`、`ok`、`yes`、`yep`、`uh`、`um`、`hmm`。
- 音频缺少持续语音证据，例如 rms 偏低、active_ratio 偏低，或连续活跃帧不足。
- 满足以上条件时直接丢弃，走 `IgnoredSilence`，不刷新最终 HUD，不上屏，不写 voice history。

### 2. 持续语音指标

补充 frame-level 指标：

- 以 20ms frame 计算 frame rms。
- 记录最长连续活跃语音 frame 毫秒数。
- 日志里输出该指标，方便下一次区分“真实短语音”和“短脉冲幻听”。

### 3. 验收增强

新增/更新单测覆盖：

- `Yeah.` / `Okay.` 在低持续语音指标下必须 drop。
- 中英文混合、较长英文短语、清晰语音不得被误 drop。
- startup idle 仍必须通过：不按热键不启动录音、不保存 raw、不上屏。

## 验收

- `cargo fmt --check` 通过。
- `cargo check -p ainput-desktop` 通过。
- `cargo test -p ainput-desktop streaming -- --nocapture` 通过。
- `cargo test -p ainput-desktop hotkey -- --nocapture` 通过。
- `cargo test -p ainput-shell` / `ainput-output` / `ainput-rewrite` 通过。
- `scripts\run-startup-idle-acceptance.ps1 -Version <new>` 通过。
- `scripts\run-streaming-live-e2e.ps1 -Version <new> -Synthetic` 通过。
- `scripts\run-streaming-live-e2e.ps1 -Version <new> -Wav -CaseLimit 3` 通过。
- 打新 preview，不覆盖旧版，并启动最新 exe 到 Windows 交互桌面。

## 风险控制

- 本轮只在最终提交前拦截低置信短英文填充词，不拦截正常中文、正常中英混合、正常较长英文短语。
- 不触碰热键吞吐和上屏路径，避免复发 `Ctrl+C/Ctrl+V` 被破坏的问题。
