# Streaming Live E2E Acceptance SPEC

## 目标

把流式语音输入的验收从“用户真实试用后反馈”升级成 Codex 可独立执行的前台闭环测试。

完成后，Codex 应该能在 Windows 真桌面会话里自动回答这些问题：

- HUD 是否真的显示出来，而不是代码里以为显示了。
- HUD 每一帧实际显示的文字是什么，是否抖动、闪烁、回退、尾部吃字。
- worker 发出的 partial/final、HUD target/display、最终上屏文本是否来自同一条状态链。
- 最终聊天框/输入框里真实出现的文字是否等于本轮 final text。
- 出错时能定位是 ASR、稳定策略、HUD 渲染、提交上屏、还是目标输入框读写的问题。

## 范围

本任务先建立验收与观测体系；凡是验收直接证明的 HUD 稳定性、上屏一致性、打包入口可靠性问题，必须在同轮修掉。非流式 `极速语音识别` 不进入本轮改动范围。

必须覆盖：

- `apps/ainput-desktop` 的流式 worker 事件、主线程事件消费、HUD overlay、最终 commit。
- `crates/ainput-output` 的 paste/readback 结果。
- `scripts/` 下可由 Codex 远程触发的验收脚本。
- `fixtures/` 下固定 wav、合成 partial、用户真实声音样本三类输入。
- `tmp/streaming-live-e2e/...` 下机器可读的报告、时间线、截图和目标输入框快照。
- `dist` 包内的同一套验收入口，不能只在 cargo debug 模式可用。
- HUD 稳定性门禁：流式期间位置、宽高、alpha、visible 状态必须可量化并进入 pass/fail。
- 中英文混合 fixture：至少覆盖一条 `good`/`HUD` 这类混合文本，验证不会因为英文 token 造成奇怪改写或上屏不一致。
- 旧剪贴板防污染门禁：每次 commit 前写入旧剪贴板哨兵，目标框读到哨兵必须失败为 `clipboard_stale_paste`。
- exactly-once 上屏门禁：一次录音 release 只能产生一次最终 commit；目标框不得出现 `final+final`、`final+错误片段` 或程序粘贴 `Ctrl+V` 反向触发下一轮语音。
- startup idle 门禁：启动新版 dist/source 后，在没有任何语音热键输入时不得进入录音、HUD、识别提交或上屏链路；日志不得出现 microphone armed、push-to-talk started、transcription delivered、output delivery timing 或 raw capture 新文件。
- 热键一致性门禁：流式模式不得把用户配置的 `hotkeys.voice_input` 偷偷覆盖成高误触的单修饰键；托盘提示、实际 hook 配置、日志里的 `voice_hotkey` 必须一致。
- 热键来源归因门禁：每次 `VoicePressed` 必须在日志里能区分 keyboard primary、modifier-only 或 mouse middle 来源，便于排查误触。
- HUD 残影门禁：每个 case reset 后必须采样 HUD target/display，新一句开始前不得残留上一句文字。
- 松手收尾门禁：真实热键路径必须先 drain 尾音、final decode、flush 最终 HUD，再提交上屏；HUD 不能在松手瞬间直接消失。
- 原始录音留样门禁：流式真实录音路径必须异步写入 `logs\streaming-raw-captures\`，自动只保留最近 20 组 wav + json。
- 按住停顿补尾字门禁：用户按住热键但已经停说时，HUD 必须在停顿窗口内 flush 当前片段尾音，最后几个字不能只等松手后出现。
- 停顿不是句末门禁：短暂停顿只能 flush 尾音和 reset 识别器，不能靠 pause_ms 把当前尾巴硬切成句子，不能强制追加 `。！？；`，也不能把未完成句子变成不可修改前缀。
- 语义标点门禁：标点模型常驻；实时 preview 可以根据文本语义插入逗号或明确的句末标点，但句末标点必须来自语义判断，不得来自停顿事件本身。
- 冻结边界门禁：只有已经出现 `。！？；` 这类明确句末标点的前缀才允许冻结；句号以前的内容冻结后不可再被后续 partial 改写，句号以后的 live tail 必须继续可改。
- 重复标点门禁：HUD partial、final 和上屏文本不得出现 `，，`、`,,`、`。。`、`！！`、`？？`、`，。`、`。？`、`？！` 等连续或冲突标点。
- 尾字保护门禁：final text 不能比最后一个非空 HUD partial 少任何内容字；尤其 `了/啊/呢/吧/吗/呀/嘛/哦/噢/诶` 这类尾字零容忍丢失。
- 真实 raw 抽样门禁：Codex 可以从最近 raw captures 中抽短句和长句各若干条回放，不要求每轮全跑 20 条。
- AI rewrite 边界：本轮不接入 AI 语义改写；`voice.streaming.ai_rewrite.enabled = false` 时验收不应期待 AI 改写效果。
- HUD 胶囊外观门禁：流式 HUD 必须是单行黑色半透明胶囊；短文本不得出现大白面板，长文本从任务栏上方中心向两边扩展，不自动换行。

明确不做：

- 不把测试依赖接入生产热路径。
- 不常驻录音，不做无提示后台采集。
- 不允许仅因启动、预热、托盘初始化或普通 Ctrl/Alt 操作启动录音。
- 不把 OCR 当主判据；HUD 文字以程序实际 SetWindowText/UI 状态为准，截图只做视觉存在性证据。
- 不要求第一版接入 TSF/IME composition。
- 不改已经稳定的非流式 `极速语音识别` 主链路。

## 当前缺口

已有自动化：

- `scripts/run-streaming-selftest.ps1`
  - 能跑固定 wav。
  - 能生成 `tmp/streaming-selftest-latest.json`。
  - 覆盖 ASR + streaming core + final text。
  - 不覆盖真实 HUD 窗口、真实目标输入框、真实 paste readback。

- `scripts/live-streaming-acceptance.ps1`
  - 能引导人工按热键说话。
  - 能 tail 日志和 voice history。
  - 仍然依赖用户肉眼判断 HUD 是否正常。

缺失的关键面：

- HUD 内部只有 trace 日志，没有可供验收脚本消费的逐帧实际显示时间线。
- `WorkerEvent::StreamingPartial/Final` 到 HUD 和 commit 之间没有统一 correlation id。
- `deliver_text()` 只报告是否发起 paste，没有在目标输入框里读回最终文本。
- SSH 启动的 Windows GUI 进程不一定属于用户当前桌面会话，不能把“SSH 里启动成功”当成真实前台显示成功。
- HUD 布局会随 partial 文字长度反复改宽、改高、改位置，肉眼表现为弹出过程抖动和闪烁。
- Windows 前台/输入法状态会污染测试目标框；commit 前必须有 focus 和 pre-commit dirty 观测。

## 核心方案

测试体系分三层，每层解决不同问题。

### L1 Core Replay

目的：继续保留当前固定 wav 回归，用来快速判断 ASR 和 streaming core 有没有退化。

输入：

- 固定 wav manifest。
- expected text / keywords。

输出：

- partial timeline。
- final text。
- rollback、first partial、final commit 等指标。

状态：

- 已有基础能力，后续只补 correlation id 和更完整的 trace 字段。

### L2 Visible Desktop Harness

目的：验证真实 Windows 桌面上的 HUD、目标输入框、上屏结果。

关键原则：

- 测试必须由正在用户桌面会话里的 ainput 进程执行。
- Codex 通过请求文件或 named pipe 发起测试，不直接假装 SSH 进程就是前台桌面。
- 目标输入框使用专用测试窗口，避免污染微信、浏览器、IDE 等真实应用。

需要新增：

- Acceptance Broker
  - 常驻在真实桌面会话中的 ainput 主进程里。
  - 监听 `tmp/acceptance-requests/*.json` 或 `\\.\pipe\ainput-acceptance`。
  - 收到请求后在 UI 线程创建 HUD、测试输入框、执行用例、写报告。

- Dedicated Target Window
  - 一个本进程 Win32 测试窗口，内部放 multiline EDIT 控件。
  - 支持 focus、Ctrl+V、WM_GETTEXT/UIA readback。
  - 窗口标题包含 `ainput acceptance target <run_id>`。
  - 执行前必须停掉旧的 `ainput-desktop.exe` 托盘进程，避免旧版本全局键盘钩子拦截测试进程自己的 `Ctrl+V`。

- Acceptance Trace
  - 统一写 `timeline.jsonl`。
  - 每条事件带 `run_id / case_id / revision / monotonic_ms`。
  - 覆盖 worker、main、HUD、output、target readback、screenshot。

### L3 Real Voice Corpus

目的：以后不再每次让用户现场试，但仍然保留用户真实声音样本。

做法：

- 增加一次性录音脚本 `scripts/record-streaming-fixtures.ps1`。
- Codex 发起录音任务后，桌面 HUD 显示一句要读的文本和倒计时。
- 用户只需要在采集语料时读一次；之后所有回归都 replay 这批 wav。
- 每个样本保存：
  - wav
  - expected text
  - speaker metadata
  - sample rate
  - device name
  - created_at

安全约束：

- 只在显式 fixture recording 模式下录音。
- 不做持续后台录音。
- 录音文件只保存在项目本地 `fixtures/user-voice/`。

## 事件模型

新增 `AcceptanceTrace`，通过环境变量或 runtime config 启用：

```text
AINPUT_ACCEPTANCE_TRACE_DIR=<repo>\tmp\streaming-live-e2e\<run_id>
```

最少事件：

```json
{"event":"case_started","run_id":"...","case_id":"...","monotonic_ms":0}
{"event":"audio_chunk","offset_ms":120,"samples":960}
{"event":"streaming_started","revision":1}
{"event":"worker_partial","revision":3,"raw_text":"...","prepared_text":"...","source":"asr"}
{"event":"main_partial_received","revision":3,"prepared_text":"..."}
{"event":"hud_retarget","revision":3,"target_text":"...","display_text":"...","hwnd":"...","rect":[0,0,0,0]}
{"event":"hud_display_sample","revision":3,"target_text":"...","display_text":"...","visible":true,"alpha":220}
{"event":"worker_final","revision":8,"final_text":"..."}
{"event":"hud_final_flush","revision":8,"display_text":"..."}
{"event":"output_commit_request","revision":8,"text":"..."}
{"event":"output_commit_result","revision":8,"delivery":"direct_paste"}
{"event":"target_readback","revision":8,"text":"..."}
{"event":"case_finished","status":"pass"}
```

采样频率：

- HUD event-driven：每次 retarget / set_text / final flush 必写。
- HUD polling：录音期间每 16-33ms 采一条轻量 sample。
- target polling：commit 后 30ms 起，每 50ms 读取一次，最多 1s。
- screenshots：默认只在 case start、first HUD visible、final flush、target readback 后保存；debug 模式可 10-20fps。

## 音频输入方案

### WavRealtime Source

用于主要自动化。

- 从 wav 文件读取样本。
- 按 `voice.streaming.chunk_ms` 分块。
- 按真实时间 sleep 后喂给 streaming core。
- 走与生产相同的 streaming 状态机、HUD、commit 路径。

这比当前 `replay-streaming-manifest` 更进一步：它不只生成报告，还要让 HUD 和目标输入框真实参与。

### Synthetic Partial Source

用于专门压测 HUD 抖动和实时改写。

manifest 直接描述 partial 时间线：

```json
{
  "id": "hud-correction-tail",
  "events": [
    {"t_ms": 0, "prepared_text": "你好你"},
    {"t_ms": 120, "prepared_text": "你好你好"},
    {"t_ms": 240, "prepared_text": "你好你好，显示还是不是很"},
    {"t_ms": 360, "prepared_text": "你好你好，显示还是不是很好"},
    {"t_ms": 620, "final_text": "你好你好，显示还是不是很好。"}
  ]
}
```

用途：

- 不依赖 ASR 是否刚好出错。
- 稳定复现“尾部修正、回退、追加、final flush”。
- 直接验证 HUD 显示算法和上屏一致性。

### Microphone Fixture Recording

用于建立用户真实声音库。

命令建议：

```powershell
.\scripts\record-streaming-fixtures.ps1 -Set user-sai-baseline
```

输出建议：

```text
fixtures\user-voice\user-sai-baseline\manifest.json
fixtures\user-voice\user-sai-baseline\001.wav
fixtures\user-voice\user-sai-baseline\002.wav
```

## 验收指标

### 必须通过

- `target_readback.text == output_commit_request.text`
- 单个 case 的 `output_commit_request` 必须为 1 次；提交后至少 1500ms 额外观察窗口内 `target_readback.text` 仍必须等于 final text，不能包含重复 final 或额外提交片段。
- 上屏前必须清理目标控件里的中文 IME composition；`target_readback.text` 不得包含 `wan`、`ngl`、`us`、`gxi` 这类未完成拼音残留。
- 源码态 live E2E 必须先 build 最新 debug exe；启动前必须停掉旧 `ainput-desktop.exe` 并复查无残留。
- `hud_final_flush.display_text == output_commit_request.text`
- `hud_after_case_reset.target_text/display_text` 必须为空，防止新一句先显示上一句内容。
- `hud_after_commit_hold.display_text == output_commit_request.text`，且此时 HUD 仍可见，防止松手后 HUD 立即消失。
- `target_readback.text` 不能等于本轮 commit 前写入的旧剪贴板哨兵。
- HUD active streaming 期间允许宽度随字数变化，但 `max_center_x_delta_px <= 3`、`max_top_delta_px <= 3`、`max_height_delta_px <= 3`。
- HUD active streaming 期间 `alpha_drop_count == 0` 且 `invisible_sample_count == 0`。
- HUD active streaming 期间 `white_panel_sample_count == 0`、`multiline_panel_sample_count == 0`、`short_text_wide_panel_count == 0`。
- final text 不能比最后一个非空 HUD target 少任何内容字；标点差异允许单独归一化，但内容字不允许丢。
- timeline 顺序必须表现为 final HUD flush 发生在 output commit request 之前；生产路径还必须在 release drain 和 final decode 之后才提交。
- `logs\streaming-raw-captures\` 必须能生成真实录音 wav/json，连续保存后 wav 数量不得超过 20。
- raw corpus 抽样必须覆盖至少 1 条短句和 1 条长句；默认选择最近 raw capture 中短样本和长样本各 2 条。
- raw corpus 中非空语音的 final content chars 比最后一个 HUD partial 多出的内容字数不得超过 1。
- raw corpus 中非空语音的 final content chars 不能少于最后一个 HUD partial；若最后一个 HUD partial 以 `了/啊/呢/吧/吗/呀/嘛/哦/噢/诶` 结尾，final 必须保留该尾字。
- raw corpus 中时长超过 1200ms 且 final 有标点的样本，final 前至少一个 partial 必须已经显示标点。
- raw corpus 中 `endpoint_rollover` 来源的 partial 不得在上一条 partial 没有句末标点时凭停顿新增尾部 `。！？；`。
- raw corpus 任意 partial/final 不得出现重复或冲突标点。
- soft finalize / endpoint rollover 只能更新 HUD 和稳定前缀，不能提前把文本提交到目标输入框。
- startup idle 期间禁止出现 `start microphone recording`、`streaming microphone armed on hotkey press`、`streaming push-to-talk recording started`、`streaming transcription delivered`、`output delivery timing`、`voice hotkey matched in keyboard hook` 或新的 raw capture 文件。
- 流式模式的实际按住说话热键必须等于 `ainput.toml` 中的 `hotkeys.voice_input`；默认配置为 `Alt+Z` 时，单独 `Ctrl` 不得启动录音。
- AI rewrite 必须在本轮保持关闭，trace/log 只能报告 disabled/skipped，不能把 AI 改写混入基础功能验收。
- 每个 HUD target 必须能追溯到同 revision 的 `worker_partial.prepared_text` 或 `worker_final.final_text`。
- 首个 partial 之后 HUD 不能长期空白。
- commit 后 1s 内必须能从目标输入框读回文本。
- case 失败时必须输出失败类别，不允许只有“exit 1”。
- 打包版 `dist\ainput-<version>\scripts\run-streaming-live-e2e.ps1` 必须等交互任务完整退出后再读 report，不能读半成品报告。

### 建议阈值

这些阈值第一版可以配置化，默认先用偏宽松值，避免误杀。

- `first_hud_after_partial_ms <= 100`
- `hud_final_flush_after_final_ms <= 80`
- `release_to_commit_ms <= 900`
- `target_readback_after_commit_ms <= 1000`
- `hud_catchup_after_latest_partial_ms <= 400`
- `max_hud_lag_chars <= 24`
- `max_unexplained_rollback_chars <= 8`
- `full_reset_count == 0`，除非 trace 标记为 new segment。

### 失败分类

报告必须把失败归到下面之一：

- `audio_input_failure`
- `asr_no_partial`
- `stability_regression`
- `hud_not_visible`
- `hud_text_diverged`
- `hud_tail_drop`
- `hud_stale_text`
- `hud_final_hold_missing`
- `output_commit_failed`
- `clipboard_stale_paste`
- `target_duplicate_commit`
- `target_extra_commit_fragment`
- `output_commit_count_mismatch`
- `output_self_hotkey_trigger`
- `startup_idle_auto_recording`
- `startup_idle_auto_output`
- `voice_hotkey_binding_mismatch`
- `voice_hotkey_unattributed`
- `old_tray_process_still_running`
- `target_readback_mismatch`
- `target_readback_unavailable`
- `raw_capture_missing`
- `raw_capture_retention_failed`
- `raw_no_partial`
- `raw_tail_late`
- `raw_punctuation_late`
- `raw_replay_failed`
- `desktop_session_unavailable`

## 报告格式

每次运行写入：

```text
tmp\streaming-live-e2e\<timestamp>-<run_id>\
  report.json
  timeline.jsonl
  summary.txt
  screenshots\
    001-case-start.png
    002-first-hud.png
    003-final-flush.png
    004-target-readback.png
  logs\
    ainput.log
    voice-history.log
  target-before.txt
  target-after.txt
```

`report.json` 必须适合脚本判定：

```json
{
  "overall_status": "pass",
  "run_id": "...",
  "cases_total": 6,
  "cases_passed": 6,
  "failures": [],
  "report_dir": "..."
}
```

## Codex 使用入口

默认入口：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Version 1.0.0-preview.24
```

参数：

- `-Version`：测试 dist 包。
- `-Manifest`：指定 wav 或 synthetic manifest。
- `-UseBroker`：默认 true，让真实桌面会话中的 ainput 执行。
- `-Direct`：仅在确认当前进程就是交互桌面时使用。
- `-DebugScreenshots`：保存高频截图。

脚本职责：

- 找到 packaged exe。
- 启动或连接 acceptance broker。
- 投递 request。
- 等待 report。
- 打印 summary。
- 根据 `overall_status` 返回 exit code。

## 完成判定

本任务完成后，流式模式的收口标准改为：

```powershell
cargo check -p ainput-desktop
cargo test -p ainput-desktop streaming
cargo test -p ainput-output
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-selftest.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Version <current>
```

只有最后一条真实前台 E2E 通过，才允许说“HUD 和实际上屏都正常”。
