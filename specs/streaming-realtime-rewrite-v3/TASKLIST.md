# Streaming Realtime Rewrite V3 TASKLIST

## Phase 0: Baseline

- [x] 建立 V3 规格包。
- [x] 在根 `TASKLIST.md` 挂载本轮 Round。
- [x] 跑当前 streaming/rewrite/output baseline 测试。

## Phase 1: Stability Manager

- [x] 新增 committed / stable / volatile / rewrite candidate 状态。
- [x] 新增本地 agreement tracker（承担 `HypothesisTracker` 职责）。
- [x] 新增 revision/stale 机制。
- [x] 替换 worker 中直接依赖旧冻结前缀 + 最新一句的路径。
- [x] 补齐状态机单测。

## Phase 2: Endpointing

- [x] 新增应用层 endpoint detector。
- [x] 新增 `[voice.streaming.endpoint]` 配置。
- [x] 将默认 pause 调整到 300-700ms 档。
- [x] 防止空段重复 rollover。
- [x] 补 endpoint 相关验证。

## Phase 3: HUD

- [x] 恢复 HUD target/display 双缓冲。
- [x] final event 立即 flush。
- [x] 沿用并打通 HUD microstream 参数。
- [x] 补 HUD 相关验证。

## Phase 4: AI Tail Rewrite

- [x] request/response 带 revision。
- [x] stale response 丢弃。
- [x] final wait budget 收敛到短等待。
- [x] AI 不可用时只降级，不阻断。
- [x] 补 AI rewrite 单测。

## Phase 5: Input Context

- [x] 扩展 context snapshot。
- [x] 增加窗口标题。
- [x] 尝试读取 caret 前后文。
- [x] Unknown fallback 不阻断。
- [x] 改写 prompt 使用新上下文。

## Phase 6: Output Adapter

- [x] 抽出 `OutputAdapter` 或等价 commit manager。
- [x] 保留 clipboard paste 当前行为。
- [x] 保留 clipboard only fallback。
- [x] streaming worker 不直接依赖 paste 细节。

## Phase 7: Regression

- [x] 回归报告增加 latency 指标。
- [x] 回归报告增加 rollback 指标。
- [x] 固定样本增加 expected/keywords。
- [x] 防止乱码但字符数够的假通过。

## Phase 8: Closeout

- [x] README 更新。
- [x] 根 TASKLIST 更新。
- [x] 旧 streaming specs 标注已被 V3 替代。
- [x] 必要时更新 MISTAKEBOOK：本轮未发现需要沉淀的新反模式。
- [ ] Windows 真机前台验收。
