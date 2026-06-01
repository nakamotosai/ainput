# Streaming Realtime Rewrite V3 PLAN

## 执行原则

- 只改流式模式，不改 `极速语音识别` 的行为。
- 每一阶段必须带测试或可观察日志。
- 新旧流式逻辑不能长期并存成两条真实入口；阶段迁移完成后清理旧入口。
- 先保证 overlay + final commit 的产品级体验，再为 TSF composition 留接口。

## Phase 0: Baseline 和保护线

目标：先固定当前行为和测试边界，避免改坏非流式模式。

修改：

- 新建 `specs/streaming-realtime-rewrite-v3/` 规格包。
- 在 `TASKLIST.md` 增加本轮 Round。
- 标记 fast mode 为保护对象。

验证：

```powershell
cargo test -p ainput-desktop streaming_state
cargo test -p ainput-rewrite
cargo test -p ainput-output
```

完成判定：

- 当前 streaming 状态机测试能跑。
- 当前 rewrite/output 测试能跑。
- worktree 中已有用户改动不被覆盖。

## Phase 1: 新 Streaming 状态模型

目标：把当前 `frozen_prefix / volatile_sentence` 升级成四层状态。

修改：

- `apps/ainput-desktop/src/streaming_state.rs`
  - 新增 `StreamingTextState`。
  - 新增 `HypothesisTracker`。
  - 新增 `StreamingRevision` 或 revision 计数。
  - 保留旧测试对应行为，但改成新模型语义。
- `apps/ainput-desktop/src/worker.rs`
  - 只通过新状态模型生成 HUD display。

关键接口：

```rust
fn observe_partial(raw_text: &str, is_final: bool) -> StreamingDelta
fn apply_local_rewrite(candidate: &str) -> StreamingDelta
fn apply_tail_rewrite(revision: u64, rewritten_tail: &str) -> Option<StreamingDelta>
fn finalize_segment() -> StreamingDelta
fn final_text() -> String
```

验证：

```powershell
cargo test -p ainput-desktop streaming_state
```

新增测试：

- 两次一致前缀才进入 stable。
- committed prefix 不可被 ASR 改写。
- volatile tail 可在 8 字窗口内变化。
- stale rewrite 不可覆盖新 revision。
- segment-only candidate 可追加到 committed prefix 后。

## Phase 2: 应用层 endpointing

目标：把 10 秒级 endpoint 改成语音输入法需要的短停顿分段。

修改：

- `worker.rs`
  - 新增 `StreamingEndpointDetector`。
  - 基于最近 300-700ms 音频活动判断 pause。
  - `maybe_rollover_streaming_segment_core()` 改成应用层 endpoint 优先。
- `main.rs`
  - `build_streaming_recognizer()` 不再把 endpoint 固定为 10s。
- `crates/ainput-shell`
  - 新增 `[voice.streaming.endpoint]` 配置。

验证：

```powershell
cargo test -p ainput-desktop endpoint
powershell -ExecutionPolicy Bypass -File .\scripts\streaming-regression.ps1
```

完成判定：

- 400-700ms 停顿能形成 segment。
- 空段不会重复 rollover。
- 长句超过 max segment 会强制收口。

## Phase 3: HUD 双缓冲恢复

目标：HUD 文字从“整块跳”改成“目标文本 / 当前显示文本”双缓冲。

修改：

- `apps/ainput-desktop/src/overlay.rs`
  - `show_status_hud(..., char_streaming=true)` 不再直接关掉 microstream。
  - `HudMicrostreamState::retarget()` 作为真实路径使用。
  - final event 到来时 flush 到最终文本。
- `config/hud-overlay.toml`
  - 增加或回写 microstream 参数。

验证：

```powershell
cargo test -p ainput-desktop hud_microstream
```

完成判定：

- 新增文字逐字追目标。
- 修正尾巴时不整句闪回。
- final 文本立即对齐。

## Phase 4: Async Tail Rewrite 正式化

目标：把 AI rewrite 改成带 revision/stale 防护的实时尾巴改写层。

修改：

- `apps/ainput-desktop/src/ai_rewrite.rs`
  - request/response 带 revision。
  - prompt 使用 committed/stable/volatile 分层。
  - 支持最终等待预算。
- `worker.rs`
  - inflight rewrite 必须可过期。
  - partial 变化超过阈值时，旧 response 不能覆盖新文本。
  - final 阶段最多等待配置的 `final_wait_ms`。
- `crates/ainput-shell`
  - 新增 `final_wait_ms / max_tail_chars / max_context_chars`。

验证：

```powershell
cargo test -p ainput-desktop ai_rewrite
```

完成判定：

- stale rewrite 被拒绝。
- AI 失败不阻断 streaming。
- final 等待不会超过预算。

## Phase 5: Input Context 扩展

目标：让改写器知道“在哪个应用、什么输入框、光标前后是什么”。

修改：

- `crates/ainput-output/src/lib.rs`
  - `OutputContextSnapshot` 扩展或新增 `InputContextSnapshot`。
  - 增加前台窗口标题读取。
  - 尝试读取 selection、text before caret、text after caret。
  - Unknown fallback 不能报错阻断。
- `ai_rewrite.rs`
  - prompt 使用新上下文字段。

验证：

```powershell
cargo test -p ainput-output
```

手动验证：

- 浏览器输入框。
- 编辑器输入框。
- 终端输入区。

完成判定：

- 至少能稳定拿到 process_name/window_title。
- 支持 UIA 的输入框能拿到 caret 附近文本。
- 不支持 UIA 的目标安全降级。

## Phase 6: Commit Manager / Output Adapter

目标：把 streaming worker 从 clipboard 细节中解耦。

修改：

- `crates/ainput-output`
  - 新增 `OutputAdapter` 接口或等价封装。
  - 当前 clipboard paste 成为一个 adapter。
  - 保留 clipboard only fallback。
- `worker.rs`
  - 只调用 commit manager，不直接构造粘贴细节。

验证：

```powershell
cargo test -p ainput-output
```

完成判定：

- 当前直贴行为保持。
- fallback 行为保持。
- streaming worker 不再绑定具体 paste 实现。

## Phase 7: Regression 指标升级

目标：回归不再只看字符数。

修改：

- `scripts/streaming-regression.ps1`
  - 输出 JSON 或结构化文本。
  - 增加 first_partial_ms、release_to_commit_ms、rollback_count、partial_update_count。
  - 增加关键词命中或 CER 近似检查。
- `apps/ainput-desktop`
  - replay 命令输出 streaming event trace。
- `fixtures/streaming-selftest/manifest.json`
  - 增加 expected_text / keywords / max_rollback。

验证：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\streaming-regression.ps1
```

完成判定：

- 报告能指出是 ASR 错、稳定策略错、HUD 慢，还是上屏慢。
- 固定样本不再因为“乱码但字数够”而通过。

## Phase 8: 文档和旧链路清理

目标：清理旧描述，避免未来又按旧错误方向修。

修改：

- `README.md`
  - 更新流式模式当前架构。
- `TASKLIST.md`
  - 勾选已完成阶段，保留未完成阶段。
- 旧 specs
  - 标注 `streaming-worker-v2` 和 `hud-microstream-smoothing` 已被 V3 替代。
- `MISTAKEBOOK.md`
  - 如实施中发现回归，写入具体防守项。

验证：

```powershell
git diff --stat
cargo test -p ainput-desktop streaming
cargo test -p ainput-rewrite
cargo test -p ainput-output
```

完成判定：

- 文档与代码真实行为一致。
- 没有“HUD 即最终结果”这类误导性旧口径残留为主说明。
- 非流式模式没有被改动。

## 实施顺序

1. Phase 0-1：状态模型先落地。
2. Phase 2：短停顿 endpointing 接入。
3. Phase 3：HUD 双缓冲恢复。
4. Phase 4：AI rewrite revision 化。
5. Phase 5-6：上下文和输出适配层。
6. Phase 7-8：回归和文档收口。

## 风险

- sherpa online paraformer 本身 partial 质量有限，状态机只能降低抖动，不能凭空修正所有识别错词。
- AI rewrite 如果用太小本地模型，可能延迟低但纠错差；如果用大模型，可能纠错好但延迟高。
- UIA 在部分应用里读不到前后文，必须接受 Unknown fallback。
- TSF composition 是后续高级路径，本轮先把接口留出来，不把系统级注册作为第一验收阻塞。

## 最小回滚点

每个 Phase 独立提交时应保证：

- fast mode 可运行。
- streaming mode 至少能 fallback 到当前一次性提交。
- 如果新 stability manager 出问题，可以用 feature flag 临时退回旧 streaming state，直到 Phase 8 清理旧入口。

Phase 8 完成后删除旧入口，不长期保留双实现。
