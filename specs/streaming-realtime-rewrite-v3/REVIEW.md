# Streaming Realtime Rewrite V3 Review

## Spec Review

结论：通过，可进入实施。

检查结果：

- 目标清楚：只重做流式实时改写链路。
- 范围清楚：覆盖 worker、state、rewrite、overlay、ASR、output、config、scripts、docs。
- 非目标清楚：不改 fast mode，不把系统级 TSF 注册作为第一验收阻塞。
- 验收可执行：包含 cargo tests、streaming regression、真实前台验收和质量指标。
- 风险已列出：ASR partial 质量、AI latency、UIA fallback、TSF 后续路径。

需要实施时特别防守：

- 不要把 AI rewrite 放到同步热路径。
- 不要让旧 streaming state 和新 stability manager 长期并存。
- 不要只用“字符数够”判断回归通过。
- 不要修改 `极速语音识别` 的输出质量链路。

## Plan Review

结论：通过，但执行时必须按阶段提交和验证。

阶段映射：

- Phase 1 覆盖稳定边界。
- Phase 2 覆盖 endpointing。
- Phase 3 覆盖 HUD 体感。
- Phase 4 覆盖 AI 尾巴改写。
- Phase 5 覆盖上下文。
- Phase 6 覆盖输出抽象。
- Phase 7 覆盖回归指标。
- Phase 8 覆盖文档和旧链路清理。

最小实施建议：

1. 先做 Phase 1-3，用户体感会最明显改善。
2. 再做 Phase 4-5，把“像大厂”的上下文改写补上。
3. 最后做 Phase 6-8，保证长期维护不再漂移。
