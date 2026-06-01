# Streaming Full Bug Audit V13 RESULTS

更新时间：2026-05-01

## 当前状态

- 已收口。
- 基线版本：`1.0.0-preview.45`
- 交付版本：`1.0.0-preview.46`
- 非目标：AI 语义改写、GPU 启用、非流式 `Alt+Z`。

## 发现

### F1 P0: startup idle 会自触发短录音

- 证据：`tmp\streaming-full-audit\20260501-212732-125\full-audit-report.json`
- 基线 `preview.45` 结果：P0=2、P1=0、P2=1。
- startup idle 30 秒内出现 `start microphone recording`、`streaming microphone armed on hotkey press`、`streaming push-to-talk recording started`，并生成短 raw capture。
- 根因：单独 `Ctrl` 已有延迟判定/组合键取消逻辑，但 `keyboard_hook_proc` 后面还残留通用 modifier-only 立即触发分支，导致启动期或组合键残留事件可直接发送 `VoicePressed`。

### F2 P2: 首个 partial p95 偏高

- 证据：`tmp\streaming-full-audit\20260501-213644-392\latency\summary.json`
- `paraformer_bilingual_asr6_chunk60`：`first_partial_avg_ms=1020`、`p50=660`、`p95=1860`、`processing_rtf_avg=0.179`。
- 最快安全候选 `paraformer_bilingual_asr6_chunk80`：`avg=1008`、`p50=640`、`p95=1840`、`processing_rtf_avg=0.165`。
- 结论：候选配置收益太小，本轮不改配置；后续要提速应研究模型、partial emission cadence 或更小模型，而不是继续加 CPU 线程或改标点。

## 修复

- F1 已修复：删除 `apps\ainput-desktop\src\hotkey.rs` 中单独 `Ctrl` 的旧通用立即触发分支，只保留专门的延迟判定/组合键取消路径。
- F1 新增单测：`modifier_only_ctrl_triggers_only_after_delay`，确保单独 `Ctrl` 不会按下瞬间就把 `VOICE_ACTIVE` 置真。
- F2 未改运行配置：保留为下一轮性能优化输入。

## 验收

- Windows 真机 `cargo test -p ainput-desktop hotkey -- --nocapture` 已通过，7/7 pass。
- Windows 真机 `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1` 已通过，产出 `dist\ainput-1.0.0-preview.46\` 与 `dist\ainput-1.0.0-preview.46.zip`。
- Windows 真机 `.\scripts\run-streaming-full-audit.ps1 -Version 1.0.0-preview.46 -LatencyRepeats 1 -LiveCaseLimit 3` 已通过为 `pass_with_p2`：P0=0、P1=0、P2=1。
- `preview.46` 全量审计通过项：package integrity、`cargo fmt --check`、`cargo check -p ainput-desktop`、hotkey tests、streaming tests、rewrite tests、v12 replay、startup idle、streaming selftest、raw corpus、synthetic live E2E、wav live E2E、latency benchmark。
- `preview.46` 已启动到 Windows 交互桌面：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.46\ainput-desktop.exe`，收口时已验证该路径进程在运行。
