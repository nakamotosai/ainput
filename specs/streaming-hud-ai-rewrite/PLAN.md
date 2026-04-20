# 流式 HUD AI 改写 PLAN

## 阶段 1：补齐配置与运行时入口

- 确认 `StreamingAiRewriteConfig`、`render_config_file()`、`AppRuntime.ai_rewriter` 已接好。
- 新建 `apps/ainput-desktop/src/ai_rewrite.rs`，实现本地 OpenAI-compatible blocking client、失败退避和提示词构建。
- 验证点：配置默认值可渲染；客户端初始化失败不阻断启动。

## 阶段 2：接入流式 worker

- 给 `StreamingCoreSession` 增加 AI 改写节流与缓存状态。
- 在 `update_streaming_partial_state()` 中，只对最新尾巴尝试 AI 改写，再回写共享 `StreamingState`。
- 在段落 rollover 与松手提交路径里清理缓存，并改成“HUD 即最终结果”。
- 验证点：代码路径能区分冻结前缀与可改写尾巴；final commit 优先 HUD。

## 阶段 3：补测试与文档

- 增加 `ai_rewrite.rs`、`worker.rs`、`ainput-shell` 的最小单测。
- README 说明新增的本地 AI 改写服务与降级行为。
- 验证点：相关测试通过，README 与当前行为一致。
