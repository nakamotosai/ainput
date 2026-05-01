# Streaming Live E2E Acceptance REVIEW

## Spec Review

结论：Spec 可执行，核心方向正确。

关键判断：

- 只继续扩大 core wav 自测不够。用户这次失败集中在 HUD 真实显示、尾部吃字、HUD 与上屏不一致，这些都不在现有 `run-streaming-selftest.ps1` 覆盖面内。
- 必须引入真实桌面会话 broker。Codex 从 SSH 直接启动 GUI 并不能稳定证明用户桌面里看到了同一个 HUD。
- HUD 文字不应靠 OCR 判断。程序已经知道自己给 HUD window 设置了什么文本，OCR 只能作为截图证据。
- 专用目标输入框是第一阶段最低风险方案。直接测微信/浏览器会引入太多第三方不可控变量。

已覆盖验收：

- HUD 实际显示。
- HUD target/display 与 worker partial/final 同源。
- final flush 是否吃字。
- paste 后目标输入框 readback。
- dist 包真实运行。
- 用户真实声音 fixture 的一次采集、多次 replay。

需要实施时特别注意：

- trace 不启用时不能影响常驻工具性能。
- broker 要有超时和单次运行锁，避免多个测试同时抢 HUD/输入框。
- target window 必须在测试结束后关闭或隐藏，不留下前台残留。

## Plan Review

结论：计划顺序合理。

原因：

- Phase 1-2 先补观测，否则后续失败仍然只能猜。
- Phase 3 单独建立目标输入框，可以先闭合上屏 readback，不被真实聊天软件拖住。
- Phase 4 再做 broker，解决 Codex 远程执行与用户桌面会话之间的断层。
- Phase 5-6 分开 wav realtime 和 synthetic partial：前者测真实 ASR，后者稳定压 HUD 边界。
- Phase 7 用户声音 fixture 放后面，避免一开始就让语料采集阻塞测试框架。

最小可交付切片：

1. Trace + HUD event。
2. Target window + readback。
3. Broker ping + synthetic partial。
4. WavRealtime visible run。
5. 用户声音 fixture。

阻塞点：

- 需要确认 Windows 上由常驻 ainput 进程执行 broker 请求，而不是 SSH 子进程直接创建 GUI。
- 若当前 dist 包不是常驻运行，需要脚本先启动/替换用户桌面会话里的 ainput。
