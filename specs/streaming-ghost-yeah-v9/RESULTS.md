# Streaming Ghost Yeah V9 RESULTS

更新时间：2026-05-01

## 结果

已修复 `preview.38` 中偶发 `Yeah 。` / `Okay 。` 幽灵上屏问题。

根因不是 rewrite 重跑，也不是剪贴板重复提交；日志显示这些都是极短流式 session：

- 音频时长只有 `560-600ms`。
- HUD 文本为空，最终由 `offline_final` 幻听成 `Yeah.` / `Okay.`。
- raw 音频的 20ms 活跃帧最长连续区间很短，缺少持续语音证据。

本轮改动：

- `AudioActivity` 增加 `sustained_voice_ms`，用于区分持续语音和短脉冲噪声。
- 最终提交前新增短英文填充词低置信过滤：`yeah / okay / ok / yes / yep / uh / um / hmm` 等在低持续语音证据下直接丢弃。
- 日志增加 `sustained_voice_ms`。
- 热键来源增加 `modifier-only voice hotkey matched/released` 日志，后续若再有误触发能直接定位来源。

## 已通过

- `cargo fmt --check`
- `cargo check -p ainput-desktop`
- `cargo test -p ainput-desktop`：83/83 pass
- `cargo test -p ainput-desktop streaming -- --nocapture`：31/31 pass
- `cargo test -p ainput-desktop hotkey -- --nocapture`：6/6 pass
- `cargo test -p ainput-desktop low_confidence -- --nocapture`：1/1 pass
- `cargo test -p ainput-desktop low_signal_filter -- --nocapture`：1/1 pass
- `cargo test -p ainput-shell`：6/6 pass
- `cargo test -p ainput-output`：9/9 pass
- `cargo test -p ainput-rewrite`：16/16 pass
- 包内 startup idle：`tmp\startup-idle-acceptance\20260501-030903-111`，pass
- 包内 synthetic live E2E：`tmp\streaming-live-e2e\20260501-030936-957`，3/3 pass
- 包内 wav live E2E：`tmp\streaming-live-e2e\20260501-030948-711`，3/3 pass
- 包内 raw corpus 抽样：`tmp\streaming-raw-corpus\20260501-031101-723`，2/2 pass

## 打包

- 最终包：`dist\ainput-1.0.0-preview.40`
- 最终 zip：`dist\ainput-1.0.0-preview.40.zip`
- 已启动：`dist\ainput-1.0.0-preview.40\ainput-desktop.exe`，PID `55276`

## 残留

- 本轮没有改 `Ctrl` 作为流式热键的天然误触风险；只保证误触发/低信号幻听不会再上屏成 `Yeah/Okay`。
- 如果用户真实想单独输入很轻、很短的 `yeah/okay`，在低信号条件下可能会被当成幻听丢弃；清晰语音或中英混合句不受影响。
