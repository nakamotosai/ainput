# Streaming Output Foundation V4 SPEC

更新时间：2026-04-30

## 目标

本轮只修 `ainput` 的流式输出体验，不做 AI 语义改写，不改非流式主链路。

目标体感：
- 按住 `Ctrl` 后，HUD 很快开始显示当前识别文本。
- 说话过程中 HUD 稳定、单行、居中、黑色半透明胶囊，不闪烁、不乱抖、不残留上一句。
- 标点可以在说话过程中逐步出现和修正，但不能因为短暂停顿硬切句。
- 语义已经明确结束的一句话可以尽早加句号；句号以前的内容冻结后不再被后续 partial 改坏。
- 尾字、语气词和短尾巴不能丢，尤其是 `了/啊/呢/吧/呀/吗/诶` 这类尾字。
- 松开 `Ctrl` 后，必须先 drain 尾音、final decode、flush 最终 HUD，再一次性上屏。
- HUD 最终文本和实际上屏文本必须同源、同内容，不允许 HUD 一套、上屏另一套。

## 本轮硬边界

### 只修流式

- 流式热键：按住 `Ctrl` 录音，松开 `Ctrl` 收尾并上屏。
- 非流式/极速热键：`Alt+Z`。本轮不能改、不能挪、不能被流式配置污染。
- 所有实现、配置、测试和日志必须区分：
  - `fast_hotkey = Alt+Z`
  - `streaming_effective_hotkey = Ctrl`
- 如果日志继续只写 `voice_input = Alt+Z`，但实际流式是 `Ctrl`，视为可观测性 bug，必须改成明确字段，不得改热键语义。

### 不修剪贴板输出链路

本轮继续使用：

```text
final text -> clipboard -> Ctrl+V -> target input
```

禁止事项：
- 不把主输出机制改成 TSF/IME。
- 不重写 `paste_via_clipboard` 主链路。
- 不引入新的“直接写目标输入框”默认路径。
- 不把输出适配层抽象作为本轮目标。

允许事项：
- 可以在流式 commit 前后加 trace、时序保护、exactly-once 检查。
- 可以修“何时 commit、commit 什么文本、是否重复 commit”的流式状态问题。
- 可以验证 `Ctrl+V` 是否仍然正常，但不能借机改输出机制。

### 不修单独 Ctrl 的天然风险

本轮承认单独 `Ctrl` 做流式 push-to-talk 天然高风险，但不改策略。

保留 `preview.31` 的正确原则：
- `Ctrl` 只监听，不吞掉系统原生 `Ctrl down/up`。
- `Ctrl+A/C/V` 必须继续正常。
- injected keyboard event 必须放行，不能误触发录音。
- 组合键期间不能启动流式录音。

本轮不做：
- 不把流式热键改成 `Alt+Z`。
- 不把流式热键改成 `Ctrl+其他键`。
- 不增加要求用户改变使用习惯的热键方案。

### 不做 AI rewrite

本轮不接入 AI 语义改写。

允许做：
- 本地标点恢复。
- 本地重复标点清理。
- 本地 tail merge / final merge。
- 本地轻量文本整理。

禁止做：
- 不调用在线大模型改写文本。
- 不引入 AI rewrite 的 revision 协议作为本轮主线。
- 不把“没有 AI 改写”作为本轮失败原因。

### 不覆盖当前版本

当前可回滚基线为：

```text
dist\ainput-1.0.0-preview.31\
dist\ainput-1.0.0-preview.31.zip
```

本轮实施时必须生成新版本，例如：

```text
dist\ainput-1.0.0-preview.32\
dist\ainput-1.0.0-preview.32.zip
```

规则：
- 不允许在原地覆盖 `preview.31`。
- 不允许把新 exe 复制进 `preview.31` 目录。
- 新版本验收失败时，`preview.31` 仍是回滚备份。
- 只有新版本通过门禁后，才停止旧进程并打开新版本。

## 当前事实和问题

基于 `preview.31` 的日志和 raw corpus 抽样，当前主要问题不是“能不能编译”，而是流式状态链路仍不够像真正的语音输入法。

已观察到的问题：
- first partial 偏慢，常见约 `1.3s - 1.9s`。
- release 到 commit 偶发超过 `1.5s`。
- 短句 raw 样本里 final 有标点，但 partial 期间完全没有标点。
- 长句 raw 样本里最后一个 HUD partial 含 `啊`，final 反而少了这个内容字。
- 用户多次观察到 HUD 上看到的文字和最终上屏文字不一致，推断松手后又重新生成了一份 final 文本并直接上屏。
- `preroll_ms = 180` 当前基本无效，因为麦克风只在按下 `Ctrl` 后才开始采集，没有按键前 ring buffer。
- endpoint rollover 仍偏机械，容易把还没语义结束的尾巴稳定下来。
- synthetic 测试比真实 raw 样本理想，不能替代用户真实声音抽样。
- 部分日志仍容易混淆配置热键和流式有效热键。

当前性能证据：
- first partial 日志平均约 `1479ms`，最小约 `1330ms`，最大约 `1933ms`。
- release-to-commit 日志平均约 `1143ms`，最大约 `1505ms`。
- release tail drain 是松手上屏最大固定等待来源，常见 `245-932ms`，平均约占 release-to-commit 的 `52%`。
- final decode 本身通常不是瓶颈，常见 `0-99ms`，平均约 `44ms`。
- clipboard + Ctrl+V 输出常见 `165-231ms`，其中 `direct_paste` 约 `140ms`。本轮不改输出链路，只做计时和防重复。
- 机器硬件有 CPU 提速空间：12 核 24 线程；当前 ASR 全局 `num_threads=4`，punctuation `num_threads=1`。
- GPU 本轮暂不纳入方案，避免 provider、依赖、包体和启动稳定性引入新变量。

## 目标架构

本轮只做流式基础链路，分成 6 条并行但有主次的链。

```text
Hotkey Ctrl
  -> Audio capture + optional preroll ring buffer
  -> Streaming ASR
  -> Stability / tail merge
  -> Async punctuation
  -> HUD single-line capsule
  -> Final drain + clipboard Ctrl+V commit
```

### 1. 音频和预滚

问题：
- 当前 `preroll_ms` 配置存在，但实际没有按键前音频，因为录音在 `Ctrl` press 后才启动。
- 首个 partial 慢，会直接造成 HUD 空白等待。

方案：
- 新增流式专用的轻量 standby audio ring buffer。
- standby 只缓存最近 `150-300ms` 音频，不启动识别、不写 raw、不显示 HUD、不提交文本。
- `Ctrl` press 后，把 ring buffer 音频拼到本轮 session 开头，再进入 streaming ASR。
- standby 必须可关闭，默认仅用于 streaming mode。
- standby 不能被当成“自动录音识别”，没有热键时绝不 ASR、绝不上屏。

验收：
- startup idle 期间不能出现 ASR、HUD、commit、raw capture。
- first partial 目标进入 `<= 900ms`，优秀目标 `<= 700ms`。
- 按键前 ring buffer 不得超过配置长度，不得永久保存。

### 2. Streaming ASR 主线程减阻

原则：
- ASR partial 生成后，HUD target 更新不能等标点、final merge、raw capture 写盘。
- 主热路径只做必要的文本归一化和状态推进。

方案：
- 保持 audio chunk 足够小，但不要为了追速度盲目降到过小导致 CPU 抖动。
- ASR partial 进入状态机后立即产生 HUD target。
- raw capture 保存走后台 writer，不阻塞 partial/HUD/final。
- final decode 和 tail drain 可以与 HUD final hold 并行调度，但 commit 必须等最终文本确定。

验收：
- first partial 后 `HUD retarget <= 100ms`。
- `hud_target` 不能因为 punctuation worker 慢而空等。
- raw capture 写盘失败不能阻塞 commit，但必须记录失败。

### 3. 稳定区和尾巴合并

问题：
- 流式 ASR partial 会反悔，不能把每次 partial 全部当成确定文本。
- final 不能无理由比 HUD 最后一版少内容字，尤其不能丢尾字。

状态模型：

```text
committed_prefix: 已经跨句冻结的前缀
stable_live: 本句内多次一致的稳定部分
volatile_tail: 本句末尾仍可变化部分
display_text: 当前 HUD target
final_candidate: 松手/endpoint 后的最终候选
commit_text: 已经决定上屏、必须先 flush 到 HUD 的唯一文本
```

规则：
- `committed_prefix` 一旦冻结，后续 partial 不能改。
- 只有明确句末标点 `。！？；` 之前的内容允许冻结成 committed prefix。
- 没有句末标点时，停顿只能 flush 尾音，不能冻结整句。
- final candidate 如果比最后一个非空 HUD target 少内容字，默认保留 HUD 尾字，除非有明确的 ASR replacement 证据。
- 对 `了/啊/呢/吧/呀/吗/诶` 这类尾字启用零容忍保护：final 不能静默删除。
- `commit_text` 是唯一上屏文本。任何 offline final、punctuation、tail merge 的结果都必须先解析成 `commit_text`，再同步 HUD final flush，最后 commit。
- HUD final flush 之后，除非开启新 session，否则不允许再有任何 ASR、punctuation、final repair 改写本轮 `commit_text`。

验收：
- `final_content_chars >= last_hud_content_chars`。
- 标点差异可归一化比较，内容字不可丢。
- tail particle 被删除时必须有 trace 原因，否则 case fail。
- `hud_final_flush_text == commit_text == clipboard_write_text == output_commit_request_text`。

### 4. 异步标点

问题：
- 当前标点经常 final 才出现。
- 标点不能靠停顿硬插，否则会出现“让。不？”、“，，”、短暂停顿被截句。

方案：
- 标点模型或规则作为常驻后台 worker。
- 输入为 `committed_prefix + stable_live + volatile_tail` 的当前 revision。
- 输出只允许改当前 live tail 的标点，不允许改 frozen prefix。
- 每个 punctuation request 带 `revision`；过期结果丢弃。
- punctuation worker 慢时，HUD 先显示未加标点文本，随后异步 retarget。
- 句末标点只能由语义/文本模式触发，不能由 pause event 单独触发。
- 重复/冲突标点统一清理：`，，`、`。。`、`，。`、`？！` 等不允许进入 HUD 或 final。

冻结规则：
- 一旦标点 worker 判断出明确句末 `。！？；`，该标点以前的文本可以进入冻结候选。
- 冻结必须等一次后续 partial 或 final 确认，避免刚插的句号立刻把错句锁死。
- 句号后的 live tail 继续可变。

验收：
- 时长超过 `1200ms` 且 final 有标点的样本，至少一个 final 前 partial 已出现合理标点。
- 短暂停顿不能单独触发句末标点。
- 任意 partial/final 不得出现重复或冲突标点。

### 5. HUD 显示

本轮 HUD 目标：
- 单行。
- 居中。
- 不换行。
- 从任务栏上方中心向两边扩展。
- 黑色半透明胶囊。
- 每个字背后都有面板，短文本不再出现大白面板。
- 新一轮录音开始前清空上一轮 target/display。
- 文字更新时不闪烁、不整块乱跳。

方案：
- HUD 使用稳定高度和中心锚点。
- 宽度随文字变化，但中心点和 top 不抖。
- 新字符可以 microstream 追 target。
- 尾部修正直接替换变化点后的尾巴，不逐字回删整个句子。
- final 到来后先 flush HUD 到最终文本，并保持可见到 commit 完成后再消失。

验收：
- active 期间 `max_center_x_delta_px <= 3`。
- active 期间 `max_top_delta_px <= 3`。
- active 期间 `alpha_drop_count == 0`。
- active 期间 `invisible_sample_count == 0`。
- `white_panel_sample_count == 0`。
- `multiline_panel_sample_count == 0`。
- case reset 后 HUD target/display 必须为空。

### 6. 松手收尾和上屏

问题：
- 松手瞬间不能暴力截断。
- final decode 没完成前不能直接粘贴旧剪贴板或半成品。
- 同一轮 release 只能 commit 一次。
- HUD 上显示过的最终文字和实际上屏文字不能来自两条不同计算链。
- 松手后允许 offline final repair 修正文本，但它不能绕过 HUD 直接上屏。

方案：
- release 后进入 `Finalizing` 状态。
- 建立本轮唯一 `StreamingCommitEnvelope`：

```text
session_id
revision
last_hud_target_text
final_online_raw_text
offline_final_raw_text
final_candidate_text
resolved_commit_text
commit_source
created_at
```

- `resolved_commit_text` 一旦生成，就是本轮唯一可上屏文本。
- HUD final flush、clipboard write、Ctrl+V commit、target readback 都必须引用同一个 `StreamingCommitEnvelope`。
- 如果 final repair 结果和当前 HUD 不同，必须先发 `HUD final flush = resolved_commit_text`，并保持 HUD 可见到 commit 完成。
- HUD final flush 之后，本轮 session 进入 `CommitLocked`，所有迟到的 ASR/标点/final result 必须丢弃并写 trace。
- 顺序必须是：

```text
release
  -> drain tail audio
  -> final decode
  -> final merge + punctuation cleanup
  -> create StreamingCommitEnvelope
  -> HUD final flush
  -> clipboard + Ctrl+V commit
  -> target readback / trace
  -> HUD hide
```

规则：
- commit 文本必须来自本轮 final text，不得读取旧剪贴板作为源。
- commit 前写入剪贴板的是 final text；旧剪贴板只能作为恢复对象或测试哨兵，不能作为 commit 源。
- exactly-once：一轮 release 只能有一个 commit request。
- 如果 final decode 超时，使用当前 `display_text` 经 final merge 后提交，并记录 `final_timeout_fallback`。
- 如果 `hud_final_flush_text` 和 `resolved_commit_text` 不一致，禁止 commit，case 直接失败为 `hud_commit_diverged`。

验收：
- `hud_final_flush.text == output_commit_request.text`。
- `target_readback.text == output_commit_request.text`。
- `output_commit_request_count == 1`。
- 额外观察窗口内不允许出现第二次粘贴。
- release-to-commit 硬门禁 `<= 1200ms`，目标 `<= 900ms`。
- `post_hud_flush_mutation_count == 0`。
- 迟到的 punctuation/final/offline repair 结果必须被拒绝，不得改变已上屏文本。
- trace 必须能解释 HUD 最后一版和 commit 文本差异来自哪一步；不能解释则失败。

### 7. 性能提速和固定等待

问题：
- 当前松手上屏慢主要不是 GPU/CPU 算不动，而是 release tail drain 固定等待过长。
- 当前首字慢主要来自 ASR 首个 hypothesis 晚、预滚无效和真实语音强弱差异，不是单纯线程数不足。
- 当前标点晚一部分来自策略主动压住 preview 句末标点，不一定是标点模型算得慢。
- 当前输出链路固定约 `165-231ms`，本轮不改主链路，只保留计时和稳定性保护。

方案：
- 增加全链路性能 trace：

```text
hotkey_pressed_at
mic_started_at
first_audio_at
first_decode_step_at
first_partial_at
first_hud_target_at
punctuation_request_at
punctuation_result_at
release_at
tail_drain_started_at
tail_drain_finished_at
online_final_finished_at
offline_final_started_at
offline_final_finished_at
commit_envelope_created_at
hud_final_flush_at
clipboard_write_at
ctrl_v_sent_at
target_readback_at
```

- release drain 从固定 `900ms` 改为自适应：
  - 默认最小等待 `120-180ms`。
  - 静音稳定 `120-180ms` 后结束。
  - 只有检测到 release 后仍有真实语音活动，才延长到 `350-500ms`。
  - 超过硬上限必须停止等待，并记录 `tail_drain_timeout_fallback`。
- `tail_padding_ms` 不再无脑叠加到所有路径；pause flush 和 release final 使用不同配置。
- final repair 单独计时，超过预算时使用 `commit_text` fallback，不阻塞上屏。
- punctuation 单独计时和异步化，不阻塞 HUD target。
- HUD microstream 动态追赶：短文本逐字，长文本按批追赶，final flush 直接对齐。
- CPU 线程拆分，不再所有模型共用一个全局 `asr.num_threads`。
- GPU 本轮不做，不做 provider probe，不改依赖。

验收：
- `release_tail_elapsed_ms` 目标 `<= 500ms`，硬门禁 `<= 650ms`。
- `offline_final_elapsed_ms` 目标 `<= 180ms`，硬门禁 `<= 350ms`；超时必须 fallback。
- `punctuation_elapsed_ms` 目标 `<= 120ms`，硬门禁 `<= 220ms`；超时不阻塞 HUD。
- `hud_final_flush_after_commit_envelope_ms <= 80ms`。
- `release_to_commit_ms` 目标 `<= 900ms`，硬门禁 `<= 1200ms`。
- 每次失败报告必须拆出慢在 `tail_drain / offline_final / punctuation / output / unknown` 哪一段。

## 异步执行策略

主线程 / 热路径：
- keyboard hook 状态判断。
- audio chunk 读取。
- ASR partial 取出。
- streaming state 推进。
- HUD target 更新。

后台 worker：
- punctuation worker：常驻、revision/stale 防护。
- raw capture writer：异步保存 wav/json，保留最近 20 条。
- acceptance trace writer：异步写 jsonl，避免 UI 卡顿。
- final decode/tail drain：release 后优先级提高，但不阻塞 HUD final hold。
- report analyzer：测试脚本离线分析，不进生产热路径。

原则：
- HUD 不等 punctuation。
- commit 等 final text。
- raw capture 不挡 commit。
- trace 不挡 UI。
- 任何异步结果必须带 revision，过期直接丢弃。
- HUD final flush 之后，异步结果必须额外检查 `CommitLocked`，不能再改本轮显示或上屏文本。

## CPU / 固定等待提速建议

本轮先不管 GPU。提速优先级是：固定等待和串行路径优先，CPU 线程调优其次，GPU 以后单独开实验。

### CPU

建议拆分线程配置：

```toml
[voice.streaming.performance]
asr_num_threads = 6
punctuation_num_threads = 1
final_num_threads = 8
background_writer_threads = 1
```

原则：
- streaming ASR 优先拿足 CPU，但要给 UI/hook 留余量。
- punctuation 小模型常驻，通常 `1-2` 线程足够。
- final decode/recheck 可以使用更多线程，但只在 release 后短时间抢占。
- 不把所有线程都打满，避免 HUD 和键盘 hook 卡顿。
- 当前机器是 12 核 24 线程，建议 benchmark `asr_num_threads = 4/6/8`，`final_num_threads = 4/8/12`。

实施时需要脚本自动测：
- 当前 CPU 核心数。
- `asr_num_threads = 2/4/6/8` 的 first partial 和 CPU 占用。
- punctuation worker 的平均耗时。
- release-to-commit 耗时。
- release drain 参数 sweep：`max=350/500/650ms`，`idle_settle=120/160/220ms`。
- HUD microstream 追赶参数 sweep：`14ms/字`、`8ms/字`、批量追赶。

### 固定等待

本轮必须重点处理这些等待：
- `STREAMING_RELEASE_GRACE_MS=900`：当前最大嫌疑，应改成自适应。
- `STREAMING_RELEASE_IDLE_SETTLE_MS=220`：可按真实静音活动缩短。
- `STREAMING_PASTE_STABILIZE_DELAY=120`：本轮不改输出主链路，但可保留计时；除非证明它是上屏问题根因，否则不动。
- `HUD_CHAR_STREAM_INTERVAL=14ms`：长文本追赶时需要动态加速。
- `chunk_ms=60`：先保守不小于 60，避免 CPU 抖动；如需更快，必须通过 benchmark 证明收益。

## 配置建议

新增或明确以下配置，名称可按现有 config 风格调整：

```toml
[voice.streaming.hotkey]
effective = "Ctrl"
configured_fast_hotkey = "Alt+Z"

[voice.streaming.audio]
chunk_ms = 60
standby_preroll_enabled = true
standby_preroll_ms = 220
tail_padding_ms = 480

[voice.streaming.stability]
min_agreement = 2
max_rollback_chars = 8
tail_particle_guard = true
freeze_requires_sentence_punctuation = true

[voice.streaming.punctuation]
enabled = true
async = true
num_threads = 1
debounce_ms = 120
timeout_ms = 220
semantic_sentence_end_only = true
dedupe_conflicting_punctuation = true

[voice.streaming.finalize]
release_drain_min_ms = 160
release_drain_idle_settle_ms = 160
release_drain_max_ms = 500
final_decode_timeout_ms = 900
release_to_commit_hard_ms = 1200
allow_display_fallback_on_timeout = true

[voice.streaming.performance]
asr_num_threads = 6
punctuation_num_threads = 1
final_num_threads = 8
gpu_enabled = false

[voice.streaming.commit]
single_commit_envelope = true
reject_post_hud_flush_mutations = true
require_hud_flush_before_commit = true
```

## 验收门禁

### 必须跑

源码级：

```powershell
cargo check -p ainput-desktop
cargo test -p ainput-desktop streaming
cargo test -p ainput-output
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-selftest.ps1
```

dist 级：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-startup-idle-acceptance.ps1 -Version <new-preview>
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Version <new-preview> -Synthetic
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-raw-corpus.ps1 -Version <new-preview> -ShortCount 1 -LongCount 1
```

真实热键门禁：
- `Ctrl` 按住说话，松开上屏。
- `Alt+Z` 非流式仍可用。
- `Ctrl+A/C/V` 在 Notepad 或测试输入框里正常。
- 没按任何热键时不得自动录音。

### 内容门禁

- final 不少于最后一个非空 HUD target 的内容字。
- 尾字 `了/啊/呢/吧/呀/吗/诶` 不允许静默消失。
- final 和 HUD final flush 一致。
- 上屏文本和 final 一致。
- HUD final flush、clipboard write、output commit、target readback 必须来自同一个 `StreamingCommitEnvelope`。
- HUD final flush 后不得再有迟到异步结果改写本轮文本。
- 没有重复提交。
- 没有旧剪贴板内容被贴上屏。
- 没有重复/冲突标点。
- 标点至少能在部分长句 final 前出现。

### 性能门禁

- `first_partial_ms <= 900`，优秀目标 `<= 700`。
- `release_tail_elapsed_ms <= 650`，目标 `<= 500`。
- `offline_final_elapsed_ms <= 350`，目标 `<= 180`；超时必须 fallback。
- `punctuation_elapsed_ms <= 220`，目标 `<= 120`；超时不阻塞 HUD。
- `release_to_commit_ms <= 1200`，目标 `<= 900`。
- 每次性能失败必须给出分段耗时，不允许只写“超时”。

### HUD 门禁

- 单行、不换行。
- 黑色半透明胶囊。
- 没有白色大面板。
- active 期间不闪烁、不隐藏。
- 中心点稳定。
- 新一轮开始前不显示上一句。
- 松手后 HUD 等 final flush 和 commit 完成后再消失。

### 版本门禁

- 新版本目录和 zip 必须存在。
- `preview.31` 目录和 zip 不被修改。
- 打开新版本前先记录旧版本路径。
- 新版本启动后报告实际 exe 路径和 PID。

## 完成定义

本轮只有在以下条件全部满足后，才算完成：

- 新 preview 包已生成，旧 `preview.31` 可回滚。
- 流式 `Ctrl` 规则未被改坏。
- 非流式 `Alt+Z` 未被改动。
- 输出仍走 clipboard + Ctrl+V。
- startup idle、真实快捷键、HUD、raw corpus、exactly-once、尾字、标点门禁通过。
- 新版已在用户 Windows 交互桌面启动。
- SPEC/PLAN/TASKLIST 回写实际结果。
