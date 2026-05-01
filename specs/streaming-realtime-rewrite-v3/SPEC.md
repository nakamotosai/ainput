# Streaming Realtime Rewrite V3 SPEC

## 目标

把 `ainput` 的 `流式语音识别` 改造成真正的“实时改写流式输出”链路。

最终用户体感应该是：

- 按住热键后，HUD 很快开始出字。
- 已经稳定的前文不再跳、不再被改坏。
- 当前尾巴可以实时修正、补标点、替换同音错词。
- 短暂停顿后，一个自然语义片段会被稳定下来。
- 松手后，最终上屏文本与 HUD 收敛后的文本一致或只发生可解释的最后同步。
- 目标聊天框不被频繁回删、重贴、闪烁或破坏光标。

## 范围

本任务只改 `流式语音识别`。

必须覆盖：

- `apps/ainput-desktop/src/worker.rs` 的 streaming worker。
- `apps/ainput-desktop/src/streaming_state.rs` 的状态模型。
- `apps/ainput-desktop/src/ai_rewrite.rs` 的实时尾巴改写协议。
- `apps/ainput-desktop/src/overlay.rs` 的 HUD 呈现策略。
- `crates/ainput-asr` 的 streaming result 接口和 endpoint 参数。
- `crates/ainput-rewrite` 的轻量规则整理。
- `crates/ainput-output` 的上下文快照与输出适配层。
- `config/ainput.toml` 和 `config/hud-overlay.toml` 的新配置项。
- `scripts/*streaming*` 回归脚本与固定样本验收。
- `README.md`、`TASKLIST.md`、相关 `specs/` 文档的当前事实回写。

明确不改：

- `极速语音识别` 的 SenseVoice 整段识别主链路。
- 截图、录屏、按键精灵主链路。
- 已经验证稳定的非流式输出质量策略，除非它被抽成共享接口且行为保持不变。

## 背景判断

公开资料和当前代码共同指向同一个结论：

- 流式 ASR 的 partial 本来就会反悔，不能把每次 partial 当最终文本。
- 专业方案必须区分 stable text 和 volatile tail。
- Windows 目标输入框行为不统一，不能默认直接在聊天框里频繁回删改写。
- 好的语音输入体感来自稳定边界、短停顿分段、上下文感知、个人词典和尾巴改写，而不是单纯缩小音频 chunk。

当前代码已经做对的部分：

- 使用 HUD overlay 展示流式文本，而不是直接实时改目标输入框。
- 有 `frozen_prefix` 和 `volatile_sentence` 的初步分层。
- 有尾巴改写入口，并限制 AI 只改冻结前缀之后的内容。
- 最终上屏使用一次性提交，避免目标输入框被高频修改。

当前主要差距：

- 稳定边界主要依赖标点和启发式，不依赖多次 hypothesis 一致性。
- endpointing 仍偏向长段，无法在 300-700ms 停顿处自然收敛。
- HUD 当前偏向整块跳字，不是真正的双缓冲逐字追目标。
- AI rewrite 是可选尾巴实验，缺少 revision/cancel/stale 防护和稳定接入策略。
- 上下文快照只有进程名和光标末尾判断，缺少窗口标题、输入框前后文、选中文本、个人词典和近期纠错。
- 回归脚本主要验字符数，不验稳定性、语义质量、回滚次数和延迟指标。

## 目标架构

新的 streaming 链路分成 8 层：

```text
Audio Capture
  -> VAD / Endpoint Detector
  -> Streaming ASR
  -> Stability Manager
  -> Local Rewrite
  -> Async Tail Rewrite
  -> HUD Renderer
  -> Commit Manager / Output Adapter
```

### 1. Audio Capture

继续使用当前 `ActiveRecording` 和增量采样机制，但 streaming 模式必须显式记录：

- input sample rate
- ASR sample rate
- chunk_ms
- captured_samples
- voiced/silent frame counters
- first_audio_at
- first_partial_at
- last_voice_activity_at
- release_started_at

### 2. VAD / Endpoint Detector

新增应用层 endpointing，不再只依赖 sherpa endpoint。

默认参数：

- `voice.streaming.endpoint.pause_ms = 480`
- `voice.streaming.endpoint.min_segment_ms = 700`
- `voice.streaming.endpoint.max_segment_ms = 16000`
- `voice.streaming.endpoint.tail_padding_ms = 240`
- `voice.streaming.endpoint.preroll_ms = 180`

行为：

- 连续静音超过 `pause_ms` 后，当前 segment 进入 finalize。
- segment finalize 后继续保留已提交 prefix，下一段从新 stream 开始。
- 如果 ASR endpoint 比应用层 endpoint 先触发，可以提前 finalize，但不能产生空段重复 rollover。
- 松手时走同一 finalize 路径，不走另一套特殊文本逻辑。

### 3. Streaming ASR

`StreamingRecognitionResult` 需要扩展为：

```rust
pub struct StreamingRecognitionResult {
    pub text: String,
    pub is_final: bool,
}
```

如果 sherpa 后续能暴露更多稳定度字段，再追加：

```rust
pub stable_prefix_chars: Option<usize>
pub confidence: Option<f32>
```

ASR 层只负责输出 raw hypothesis，不负责决定什么可以提交。

### 4. Stability Manager

替换当前过于简单的 `frozen_prefix / volatile_sentence` 规则，升级为：

```rust
struct StreamingTextState {
    committed_prefix: String,
    stable_tail: String,
    volatile_tail: String,
    rewrite_candidate: Option<RewriteCandidate>,
    display_text: String,
    revision: u64,
}
```

新增 local agreement：

```rust
struct HypothesisTracker {
    recent: VecDeque<String>,
    min_agreement_count: usize,
    max_rollback_chars: usize,
}
```

默认策略：

- `min_agreement_count = 2`
- `max_rollback_chars = 8`
- 连续两次 partial 的最长公共前缀可以进入 `stable_tail`。
- `committed_prefix` 一旦提交，不允许被 ASR 或 AI 改写。
- `volatile_tail` 允许变化，但变化范围不得超过最近尾巴窗口。
- 当 ASR 候选明显短于当前显示，默认保留当前 display，除非它是新 segment。

状态语义：

- `committed_prefix`：已经跨 segment 稳定，最终提交时必须保留。
- `stable_tail`：本段内已多次一致，HUD 可显示为稳定文本。
- `volatile_tail`：本段末尾仍在变化，HUD 可被实时替换。
- `rewrite_candidate`：AI 或规则对 volatile tail 的候选修正，必须带 revision。

### 5. Local Rewrite

本地轻量整理继续保留，但只允许作用于未提交区域：

- whitespace cleanup
- filler prefix trim
- duplicate collapse
- number normalization
- common product aliases
- punctuation restoration
- personal term replacement

本地标点模型只处理 `stable_tail + volatile_tail`，不能把标点引入 `committed_prefix` 后再反复触发误冻结。

### 6. Async Tail Rewrite

AI 改写从“实验性尾巴补丁”升级成正式但可降级的实时改写层。

协议必须有：

```rust
struct TailRewriteRequest {
    revision: u64,
    committed_prefix: String,
    stable_tail: String,
    volatile_tail: String,
    input_context: InputContextSnapshot,
    user_terms: Vec<TermHint>,
}

struct TailRewriteResponse {
    revision: u64,
    rewritten_tail: String,
}
```

规则：

- 每次请求只改 `stable_tail + volatile_tail` 的尾部窗口。
- 返回时 revision 已过期则丢弃。
- 同一时刻最多一个 inflight rewrite。
- 新 partial 到来后，如果文本变化超过阈值，旧 rewrite 直接标记 stale。
- AI 不可用时，链路退回 Local Rewrite，不阻塞 HUD 和最终上屏。
- 松手 final 阶段最多等待 `final_wait_ms`，默认 `280ms`，超时用当前稳定文本提交。

默认参数：

- `enabled = false` 作为安全默认。
- 当用户明确打开高质量模式后，`enabled = true`。
- `debounce_ms = 180`
- `timeout_ms = 320`
- `final_wait_ms = 280`
- `min_visible_chars = 6`
- `max_tail_chars = 120`
- `max_context_chars = 200`

### 7. Input Context

`OutputContextSnapshot` 扩展为 `InputContextSnapshot`：

```rust
struct InputContextSnapshot {
    process_name: Option<String>,
    window_title: Option<String>,
    control_kind: InputControlKind,
    caret_position: CaretPositionKind,
    selected_text: Option<String>,
    text_before_caret: Option<String>,
    text_after_caret: Option<String>,
}
```

上下文来源优先级：

1. UI Automation TextPattern / TextPattern2。
2. ValuePattern 只读当前值。
3. 前台窗口进程名和标题。
4. Unknown fallback。

上下文用途：

- 判断光标是否在末尾。
- 决定 emoji 语音触发是否安全。
- 给 AI rewrite 提供 app 和前后文。
- 选择输出策略。

### 8. HUD Renderer

HUD 必须回到双缓冲：

```rust
hud_target_text
hud_display_text
hud_committed_prefix
hud_stable_suffix
hud_volatile_suffix
```

规则：

- 新 target 到来时做 diff。
- 稳定前缀不回退。
- 后缀修正直接替换变化点之后的尾巴，不做逐字回删。
- 新增字符按 12-18ms 逐字追目标。
- Final 到来时立即 flush 到最终文本。
- HUD 只展示文字本身，不新增解释性状态文案。

可选视觉区分：

- 第一版可以不做颜色差异。
- 如果要区分稳定/可变，应只在 HUD 内部轻微灰度，不改变最终文本。

### 9. Commit Manager / Output Adapter

第一里程碑仍采用：

```text
HUD 实时预览 -> 松手/segment final -> 一次性提交到目标输入框
```

目标输入框不做高频实时回删。

输出适配层必须抽象为：

```rust
trait OutputAdapter {
    fn commit_text(&self, text: &str, context: &InputContextSnapshot) -> Result<OutputDelivery>;
}
```

适配器优先级：

1. 未来高级模式：TSF composition / text service。
2. 当前主路径：clipboard + Ctrl+V。
3. fallback：clipboard only。

本任务第一阶段不强制注册系统级 IME/TSF，但接口设计必须允许后续加 TSF composition，不再把 clipboard paste 写死在 streaming worker。

## 配置变更

新增或调整：

```toml
[voice.streaming]
chunk_ms = 60
panel_enabled = true
rewrite_enabled = true

[voice.streaming.stability]
min_agreement_count = 2
max_rollback_chars = 8
min_stable_chars = 2
hold_shortfall_tolerance_chars = 3

[voice.streaming.endpoint]
pause_ms = 480
min_segment_ms = 700
max_segment_ms = 16000
tail_padding_ms = 240
preroll_ms = 180

[voice.streaming.ai_rewrite]
enabled = false
endpoint_url = "http://127.0.0.1:8080/v1/chat/completions"
model = "Qwen3-0.6B"
timeout_ms = 320
debounce_ms = 180
final_wait_ms = 280
min_visible_chars = 6
max_tail_chars = 120
max_context_chars = 200

[voice.streaming.hud]
microstream_enabled = true
char_interval_ms = 14
flush_on_final = true
```

兼容要求：

- 旧配置缺字段时必须用默认值补齐。
- 不因为 AI rewrite 配置不可用导致 streaming 模式启动失败。
- 不改变 fast mode 配置语义。

## 验收标准

### 自动化验收

必须通过：

```powershell
cargo test -p ainput-desktop streaming
cargo test -p ainput-rewrite
cargo test -p ainput-output
powershell -ExecutionPolicy Bypass -File .\scripts\streaming-regression.ps1
```

新增 streaming regression 指标：

- first_partial_ms
- partial_update_count
- rollback_count
- committed_chars
- stable_chars
- volatile_chars
- final_commit_ms
- release_to_commit_ms
- final_text
- expected_text
- char_error_rate 或最小关键词命中率

固定 wav 不再只验“可见字符数”。

### 真实前台验收

至少覆盖：

- 微信或聊天输入框。
- 浏览器输入框。
- VS Code / Cursor / 普通编辑器。
- Windows Terminal 或终端类输入区。

每类至少验证：

- 连续长句不会只出两三个字。
- HUD 期间文字持续出现，不是大块跳。
- 已稳定前文不被后续 partial 改坏。
- 当前尾巴允许修正，但不会整句闪回。
- 松手后 450ms 内完成最终上屏，目标值 220ms 内。
- 上屏文本与 HUD final 一致。

### 质量验收

至少准备 20 条自用语音样本：

- 中文日常句。
- 中英混合技术句。
- 数字/验证码。
- 产品名和模型名。
- 自我修正句，例如“不是 A 是 B”。
- 长句，至少 15 秒。

验收目标：

- 首字目标：`<= 300ms`，硬上限 `<= 450ms`。
- 松手到提交目标：`<= 220ms`，硬上限 `<= 450ms`。
- 回滚范围：默认不超过最后 8 个可见字符。
- 已提交 prefix 回写次数：0。
- 用户手动修改率持续下降，作为后续版本主指标。

## 失败和降级

- 麦克风不可用：明确前台错误，streaming worker 回 idle。
- ASR 初始化失败：streaming 模式不可用，但 fast mode 不受影响。
- Punctuation 模型缺失：只降级为无标点 streaming，不阻断启动。
- AI rewrite 不可用：本地规则继续工作，日志记录一次性退避。
- UIA 上下文读取失败：上下文标记 Unknown，不做高风险 emoji/上下文改写。
- 直贴失败：写剪贴板并前台提示。

## 非目标

本规格不要求一次性完成完整系统级 IME 注册。

原因：

- 当前项目历史约束是不做系统级 IME/TSF 注册。
- TSF 是正确的高级方向，但需要安装、注册、权限、兼容性和回滚设计。
- 第一验收目标是把现有 overlay streaming 体验做到产品级稳定。

但本规格要求输出层接口不再阻碍未来 TSF composition。

## 完成定义

本任务完成时必须满足：

- 旧 `StreamingState` 的冻结逻辑被新 stability manager 取代或封装为兼容外壳。
- streaming worker 不再把 raw partial 直接等同于 display final。
- 400-700ms 短停顿可稳定切段。
- AI rewrite 有 revision/stale 防护。
- HUD 恢复目标/显示双缓冲，不再大块跳字。
- 上下文快照扩展到窗口标题和输入框前后文。
- 回归脚本输出延迟、回滚、稳定指标。
- README/TASKLIST/相关 specs 回写当前事实。
- fast mode 行为保持不变，并有最小回归证明。
