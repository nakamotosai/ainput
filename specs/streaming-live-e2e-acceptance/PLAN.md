# Streaming Live E2E Acceptance PLAN

## 执行原则

- 先补观测，不先改识别策略。
- 先让失败可定位，再让测试变严格。
- 用户明确指出的可见体验问题必须进门禁；HUD 抖动、闪烁、上屏污染都不算“肉眼小问题”。
- 测试必须跑在用户真实桌面会话里；SSH 只负责发起和收报告。
- 所有验收入口必须能测试 `dist` 包，不能只测源码调试版。
- 非流式模式不进入本轮改动范围。

## Phase 0: Spec 和当前缺口冻结

目标：把“为什么之前编译通过但用户仍然无法用”写成可执行验收缺口。

修改：

- 新建 `specs/streaming-live-e2e-acceptance/`。
- 在根 `TASKLIST.md` 增加 Round 12。
- 明确已有 `run-streaming-selftest.ps1` 只覆盖 core，不覆盖真实 HUD/上屏。

验证：

```powershell
Test-Path .\specs\streaming-live-e2e-acceptance\SPEC.md
Test-Path .\specs\streaming-live-e2e-acceptance\PLAN.md
```

完成判定：

- 后续实现有清晰验收目标。
- 不再把人工 `live-streaming-acceptance.ps1` 当自动化通过。

## Phase 1: Acceptance Trace

目标：先让 HUD、worker、output 的真实状态可被脚本读取。

修改：

- 新增 `apps/ainput-desktop/src/acceptance_trace.rs`。
- 通过 `AINPUT_ACCEPTANCE_TRACE_DIR` 启用 JSONL trace。
- 在以下位置写事件：
  - worker partial/final。
  - main 收到 `WorkerEvent::StreamingPartial/Final`。
  - overlay `show_status_hud` / retarget / tick / final flush。
  - output commit request/result。
- 每条事件带 `run_id / case_id / revision / monotonic_ms`。

验证：

```powershell
cargo test -p ainput-desktop acceptance_trace
cargo test -p ainput-desktop streaming
```

完成判定：

- 不启动 GUI 时也能写最小 trace。
- trace 不启用时生产路径无额外文件输出。

## Phase 2: HUD 可观测化

目标：验收脚本能知道 HUD 实际显示了什么。

修改：

- 在 `overlay.rs` 中记录每次 `SetWindowTextW` 的实际文本。
- 对 HUD window 保存：
  - hwnd
  - text hwnd
  - rect
  - alpha
  - visible
  - target_text
  - display_text
- 每个 case 汇总 `hud_stability`：
  - 最大 left/top 位移。
  - 最大 width/height 变化。
  - alpha 回落次数。
  - active 期间不可见样本数。
- 增加 `hud_display_sample` 事件。
- 增加截图函数，只截 HUD rect 和目标窗口 rect。

验证：

```powershell
cargo test -p ainput-desktop hud_microstream
```

完成判定：

- 能区分“worker 有 partial，但 HUD 没显示”和“HUD 显示了但文本不对”。
- final 到来后能看到 HUD flush 到最终文本的确切时间。
- `hud_move`、`hud_size`、`hud_flash` 超阈值时 case 必须失败。

## Phase 2.5: HUD 稳定性修复

目标：流式文字弹出过程中，HUD 不再因 partial 长短变化而抖动或闪烁。

修改：

- 流式/status HUD 使用固定宽度和固定稳定高度。
- partial 更新、microstream 字符推进、final flush 都走同一稳定尺寸路径。
- 非流式极速识别主链路不进入本轮改动。

验证：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Synthetic -InteractiveTask
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Wav -InteractiveTask
```

完成判定：

- synthetic 和 wav 报告中 `hud_move=0/0`、`hud_size=0/0`、`hud_flash=0/0`。
- 长句 `sentence_combo_long` 也必须通过该门禁。

## Phase 3: Dedicated Target Window

目标：不用真实聊天软件也能验证上屏。

修改：

- 在 `apps/ainput-desktop` 增加 acceptance target window。
- 使用 Win32 multiline EDIT 控件。
- 提供方法：
  - create/focus/clear。
  - read_text_by_wm_gettext。
  - read_text_by_uia fallback。
  - screenshot target rect。
- commit 前再次 focus，并记录 `target_focus_before_commit`。
- commit 前如果 target 已被输入法或外部键盘污染，写 `target_precommit_dirty` 后清空。
- 窗口标题包含 run id。

验证：

```powershell
cargo test -p ainput-desktop acceptance_target
cargo test -p ainput-output
```

完成判定：

- Ctrl+V 后能读回完整文本。
- readback 失败时能给出 `target_readback_unavailable`，不误判为通过。
- 测试窗口前台污染不应导致 false fail；污染必须可追踪。

## Phase 4: Desktop Acceptance Broker

目标：解决 SSH 进程不等于真实前台桌面的问题。

修改：

- 在常驻 ainput 主进程中增加 broker。
- 请求入口二选一，第一版建议先用文件队列：
  - `tmp\acceptance-requests\<run_id>.json`
  - `tmp\acceptance-results\<run_id>.json`
- broker 每 300ms 轮询请求目录。
- 只接受本机项目目录内请求。
- 执行时在真实桌面会话里创建 target window、HUD、截图和 trace。

验证：

```powershell
.\dist\<version>\ainput-desktop.exe bootstrap
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Version <version> -BrokerPing
```

完成判定：

- Codex 通过 SSH 运行脚本能触发桌面会话里的 broker。
- broker 能写 result 文件。
- broker 不可用时脚本明确报 `desktop_session_unavailable`。

## Phase 5: WavRealtime Visible Run

目标：让固定 wav 不只跑 core，还实际驱动 HUD 和目标输入框。

修改：

- 抽象 `StreamingInputSource`：
  - production microphone source。
  - `WavRealtimeSource`。
- 抽出 streaming session runner，让生产热键和测试 wav 共用同一条状态链。
- `WavRealtimeSource` 按 chunk_ms 实时 sleep 并喂入样本。
- 每个 partial/final 走 main event、HUD、output commit。

验证：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Manifest .\fixtures\streaming-selftest\manifest.json
```

完成判定：

- report 能同时包含 worker timeline、HUD timeline、target readback。
- 目标输入框文本等于 final text。

## Phase 6: Synthetic Partial Harness

目标：稳定复现 HUD 抖动、尾部改写、final flush，不依赖 ASR 刚好触发。

修改：

- 增加 `fixtures\streaming-hud-e2e\manifest.json`。
- 增加 synthetic case runner。
- 按 manifest 中的 `t_ms/prepared_text/final_text` 驱动 HUD 和 commit。

验证：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Synthetic
```

完成判定：

- 可以稳定测出：
  - 后缀修正不整段乱抖。
  - final flush 不吃最后一个字。
  - HUD target/display 与最终上屏同源。

## Phase 7: User Voice Fixture Recording

目标：一次采集用户真实声音，以后 Codex 自己 replay。

修改：

- 新增 `scripts\record-streaming-fixtures.ps1`。
- 新增 broker request `record_fixture_set`。
- HUD 显示倒计时和待读句子。
- 保存 wav + manifest + metadata。

默认采集句子：

- “喂喂喂，你好你好，显示现在是否正常。”
- “这个显示最后一个字不能漏掉。”
- “HUD 上面显示的东西必须和最终上屏一样。”
- “我连续说一长串话的时候不要乱抖动。”
- “good 这种中英文混合也不能变得很奇怪。”

验证：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\record-streaming-fixtures.ps1 -Set user-sai-baseline
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Manifest .\fixtures\user-voice\user-sai-baseline\manifest.json
```

完成判定：

- 采集只需用户读一次。
- 后续回归不再要求用户现场试。

## Phase 8: Script 和 dist Gate

目标：把新 E2E 验收并入打包前后流程。

修改：

- 新增 `scripts\run-streaming-live-e2e.ps1`。
- `scripts\package-release.ps1` 打包新脚本和 fixtures。
- README 增加“流式真实前台验收”说明。
- `TASKLIST.md` 勾选已完成阶段。

验证：

```powershell
cargo check -p ainput-desktop
cargo test -p ainput-desktop streaming
cargo test -p ainput-output
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-selftest.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Version <version>
```

完成判定：

- 新 dist 包可以自己跑真实前台 E2E。
- 失败时报告能直接指出失败层。
- 后续不能再用“编译通过 + core wav 通过”替代前台验收。

## 实施顺序

1. Phase 1-2：先让真实 HUD 可观测。
2. Phase 3：建立专用目标输入框和 readback。
3. Phase 4：用 broker 解决真实桌面会话问题。
4. Phase 5：让 wav replay 驱动真实 HUD 和上屏。
5. Phase 6：增加 synthetic partial，专测 HUD 抖动和吃字。
6. Phase 7：采集用户声音 fixture。
7. Phase 8：接入 dist 和 README 收口。

## 风险和处理

- 风险：broker 请求文件被旧进程忽略。
  - 处理：脚本先做 `BrokerPing`，不通就明确失败。

- 风险：截图正常但文本 trace 不一致。
  - 处理：以 trace/readback 判失败，截图只做辅助证据。

- 风险：UIA 在某些真实应用里读不到文本。
  - 处理：第一版目标窗口用 Win32 EDIT 控件，真实应用兼容性后续单独扩展。

- 风险：测试代码污染生产路径。
  - 处理：所有 trace/broker case 只在 acceptance request 或 env var 开启时运行。

- 风险：用户声音 fixture 过期。
  - 处理：fixture manifest 记录麦克风设备和创建时间；需要时重新采集一组。
