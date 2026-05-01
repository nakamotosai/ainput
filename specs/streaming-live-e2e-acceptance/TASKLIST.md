# Streaming Live E2E Acceptance TASKLIST

## Startup Idle No-Auto-Recording Gate

- [x] 增加 `scripts\run-startup-idle-acceptance.ps1`
- [x] source/debug 启动空闲不触发录音、HUD、上屏或 raw capture
- [x] dist 包内启动空闲不触发录音、HUD、上屏或 raw capture
- [x] 流式模式实际热键与配置 `hotkeys.voice_input` 一致，默认 `Alt+Z` 下单独 `Ctrl` 不触发语音
- [x] 语音触发日志包含 keyboard primary / modifier-only / mouse-middle 来源

---

## Phase 0: Spec 和当前缺口冻结

- [x] 写入 `SPEC.md`
- [x] 写入 `PLAN.md`
- [x] 写入 `REVIEW.md`
- [x] 写入本 `TASKLIST.md`
- [x] 在根 `TASKLIST.md` 增加 Round 12

## Phase 1: Acceptance Trace

- [x] 新增 `acceptance.rs` 内部 `TraceWriter`
- [ ] 增加全局 `AINPUT_ACCEPTANCE_TRACE_DIR`
- [x] synthetic worker partial/final 写 trace
- [x] synthetic main/HUD 等价路径写 trace
- [x] output commit 写 trace
- [x] trace 单元测试

## Phase 2: HUD 可观测化

- [x] overlay 提供 HUD target/display/rect/visible 快照
- [x] live E2E 写 `hud_after_partial`
- [ ] overlay 写持续 `hud_display_sample`
- [x] live E2E 写 `hud_final_flush`
- [ ] 增加 HUD rect screenshot helper
- [x] HUD 观测测试接入 live E2E
- [x] live E2E 报告 `hud_stability`
- [x] HUD 抖动/闪烁进入失败类别：`hud_jitter` / `hud_flicker`
- [x] 流式 HUD 使用固定稳定尺寸，避免 partial/final 反复 resize

## Phase 3: Dedicated Target Window

- [x] 新增 acceptance target window
- [x] 支持 focus/clear
- [x] 支持 paste readback
- [ ] 支持 target screenshot
- [x] target readback 测试
- [x] commit 前写 `target_focus_before_commit`
- [x] commit 前清理 pre-commit dirty 并写 trace，隔离测试窗口外部输入污染
- [x] commit 前清空后再次聚焦，并写 `target_focus_after_clear` / `focused_hwnd` / `edit_is_focused`

## Phase 4: Desktop Session Runner

- [ ] 增加 request/result 目录
- [ ] 增加 `BrokerPing`
- [ ] 增加单次运行锁
- [x] `-InteractiveTask` 在真实桌面会话执行 case
- [x] 非交互 SSH 直接执行失败时能通过 report 定位为 output/target 问题

## Phase 5: WavRealtime Visible Run

- [ ] 抽象 `StreamingInputSource`
- [ ] 增加 `WavRealtimeSource`
- [ ] 生产热键和测试 wav 共用 streaming session runner
- [x] wav case 驱动真实 HUD
- [x] wav case 驱动真实 target readback
- [x] wav case 覆盖 HUD 稳定性指标

## Phase 6: Synthetic Partial Harness

- [x] 新增 synthetic manifest 格式
- [x] 新增 HUD correction cases
- [x] 新增 tail drop case
- [x] 新增 final flush case
- [x] synthetic runner 接入 live E2E 脚本

## Phase 7: User Voice Fixture Recording

- [ ] 新增 `record-streaming-fixtures.ps1`
- [ ] broker 支持 `record_fixture_set`
- [ ] HUD 显示录音提示和倒计时
- [ ] 保存 wav + manifest + metadata
- [ ] 用户声音 fixture 可被 live E2E replay

## Phase 8: Script 和 dist Gate

- [x] 新增 `run-streaming-live-e2e.ps1`
- [x] package-release 打包脚本和 fixtures
- [x] README 写入真实前台验收入口
- [x] dist 包跑 live E2E
- [x] dist 包交互式脚本等待 task exit code，避免读半成品 report
- [x] 根 `TASKLIST.md` 勾选完成项

## Phase 9: 用户真实反馈回归门禁

- [x] commit 前旧剪贴板哨兵进入 timeline，目标框收到哨兵失败为 `clipboard_stale_paste`
- [x] 每个 case reset 后记录 `hud_after_case_reset`
- [x] HUD reset 后仍有 target/display 时失败为 `hud_stale_text`
- [x] final HUD 使用真实生产一致的非 persistent 完成态
- [x] commit 后记录 `hud_after_commit_hold`
- [x] commit 后 HUD 不可见或不等于 final text 时失败为 `hud_final_hold_missing` / `hud_text_diverged`
- [x] `probe-streaming-live` 报告原始录音 wav/json 落盘路径
- [x] 原始录音保存支持同毫秒唯一文件名并自动保留最近 20 组
- [x] Windows 源码态 live E2E 重新通过
- [x] Windows dist 包内 live E2E 重新通过
- [x] Windows 真项目 `logs\streaming-raw-captures\` 产生 wav/json 并通过保留上限检查

## Phase 10: 按住停顿补尾字与实时标点

- [x] 本轮不接 AI rewrite，保持 `voice.streaming.ai_rewrite.enabled = false`
- [x] 应用层 endpoint 用作 soft finalize：停顿时只刷新 HUD/稳定前缀，不提前上屏
- [x] soft finalize 在 reset 前补静音并调用 streaming `input_finished()`，让尾字不用等松手
- [x] 实时 preview 使用常驻标点模型，但标点结果减少内容字时拒绝
- [x] 停顿边界使用常驻标点模型，允许 HUD 在松手前出现句末标点
- [x] 新增 `scripts\run-streaming-raw-corpus.ps1`，默认抽短句和长句各 2 条
- [x] 源码态 raw corpus 抽样通过：尾字延迟和标点延迟门禁
- [x] 包内 raw corpus 抽样通过：尾字延迟和标点延迟门禁
- [x] 源码态/包内 live E2E 仍通过

## 完成判定

- [x] `cargo check -p ainput-desktop`
- [x] `cargo test -p ainput-desktop streaming`
- [x] `cargo test -p ainput-output`
- [x] `scripts\run-streaming-selftest.ps1`
- [x] `scripts\run-streaming-live-e2e.ps1 -Version <current> -InteractiveTask`
- [x] report 能明确分出 HUD 异常、上屏异常、目标读回异常
- [x] report 能明确分出 HUD 抖动/闪烁异常
- [x] 源码态 synthetic live E2E：`tmp\streaming-live-e2e\20260430-101731-345`，3/3 pass，HUD move/size/flash 全为 0
- [x] Round 15 源码态 raw corpus：`tmp\streaming-raw-corpus\20260430-121618-781`，4 条 pass，短句+长句覆盖，`final_extra_chars=0`，partial/final 均有标点
- [x] Round 15 源码态 synthetic live E2E：`tmp\streaming-live-e2e\20260430-121740-655`，3/3 pass，HUD move/size/flash 全为 0
- [x] Round 15 源码态 wav live E2E：`tmp\streaming-live-e2e\20260430-121756-340`，6/6 pass，HUD move/size/flash 全为 0
- [x] Round 15 包内 raw corpus fixture：`dist\ainput-1.0.0-preview.24\tmp\streaming-raw-corpus\20260430-123441-839`，4 条 pass，短句+长句覆盖，`final_extra_chars=0`，partial/final 均有标点
- [x] Round 15 包内 synthetic live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-123514-026`，3/3 pass，HUD move/size/flash 全为 0
- [x] Round 15 包内 wav live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-123527-953`，6/6 pass，HUD move/size/flash 全为 0
- [x] 打包脚本已补 raw capture 留样保护：zip 后恢复 `dist\...\logs\streaming-raw-captures\`，避免后续重打包清空本地留样
- [x] 源码态 wav live E2E：`tmp\streaming-live-e2e\20260430-101743-605`，6/6 pass，HUD move/size/flash 全为 0
- [x] 包内 synthetic live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-102339-342`，3/3 pass，HUD move/size/flash 全为 0
- [x] 包内 wav live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-102352-551`，6/6 pass，HUD move/size/flash 全为 0
- [x] 源码态 synthetic live E2E：`tmp\streaming-live-e2e\20260430-110516-575`，3/3 pass，新增 `hud_stale_text` / `hud_after_commit_hold` 门禁通过
- [x] 源码态 wav live E2E：`tmp\streaming-live-e2e\20260430-110528-184`，6/6 pass，HUD move/size/flash 全为 0

## Phase 11: HUD 单行黑色胶囊动态面板

- [x] HUD 默认配置改为黑色半透明背景、白色文字、居中显示
- [x] 流式 HUD 移除固定多行大面板路径，改为按内容宽度单行扩展
- [x] live E2E 稳定性门禁改为检查中心点稳定，宽度变化不再视为抖动
- [x] live E2E 新增 `hud_white_panel`、`hud_multiline_panel`、`hud_short_text_wide_panel` 外观门禁
- [x] package-release 不再保留旧 dist 的 HUD 尺寸、颜色、对齐设置，避免白面板被带回包内
- [x] 源码态 synthetic live E2E：`tmp\streaming-live-e2e\20260430-140234-142`，3/3 pass，`hud_center` 最大 `0/0`，`hud_panel` 全 `0/0/0`
- [x] 源码态 wav live E2E：`tmp\streaming-live-e2e\20260430-140252-617`，6/6 pass，`hud_center` 最大 `1/0`，`hud_panel` 全 `0/0/0`
- [x] 包内 synthetic live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-140634-134`，3/3 pass，`hud_center` 最大 `0/0`，`hud_panel` 全 `0/0/0`
- [x] 包内 wav live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-140652-187`，6/6 pass，`hud_center` 最大 `1/0`，`hud_panel` 全 `0/0/0`
- [x] `dist\ainput-1.0.0-preview.24.zip` 已重建，大小 `456341974` bytes，时间 `2026-04-30 14:06:21`
- [x] 源码态真实麦克风 probe 留样：`logs\streaming-raw-captures\streaming-raw-1777514764552.wav` + `.json`
- [x] 包内 synthetic live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-111750-538`，3/3 pass，新增 `hud_stale_text` / `hud_after_commit_hold` 门禁通过
- [x] 包内 wav live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-111805-306`，6/6 pass，HUD move/size/flash 全为 0
- [x] 包内真实麦克风 probe 留样：`dist\ainput-1.0.0-preview.24\logs\streaming-raw-captures\streaming-raw-1777515538735.wav` + `.json`

## Phase 12: 语义标点与尾字保护

- [x] 本轮不接 AI rewrite，保持 `voice.streaming.ai_rewrite.enabled = false`
- [x] 停顿 endpoint 只 flush 尾音和 reset 识别器，不再把 pause 当句末 finalize
- [x] 停顿边界不再强制追加 `。！？；`，也不把未完成句子整体冻结成 committed prefix
- [x] 只有已经存在明确句末标点的前缀才允许冻结；句号以后的 live tail 继续可改
- [x] 标点输出统一去重，禁止 `，，`、`,,`、`。。`、`？？`、`？！` 等重复或冲突标点
- [x] final 提交零容忍尾字丢失；最后 HUD partial 里的 `了/啊/呢/吧/吗/呀/嘛/哦/噢/诶` 必须保留
- [x] raw corpus 门禁新增 `raw_final_tail_dropped`
- [x] raw corpus 门禁新增 `raw_duplicate_punctuation`
- [x] raw corpus 门禁新增 `raw_punctuation_forced_by_pause`
- [x] Windows 源码态 raw corpus 抽样覆盖短句、长句和语气尾字，通过新增门禁：`tmp\streaming-raw-corpus\20260430-145208-844`
- [x] Windows 源码态 synthetic/wav live E2E 通过：`tmp\streaming-live-e2e\20260430-145346-906` / `tmp\streaming-live-e2e\20260430-145402-148`
- [x] 重新打包到 `dist\ainput-1.0.0-preview.24\`
- [x] dist 包内 raw corpus 和 synthetic/wav live E2E 通过：`dist\ainput-1.0.0-preview.24\tmp\streaming-raw-corpus\20260430-150531-347` / `dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-150531-487` / `dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-150548-099`

## Phase 13: Exactly-once 上屏与自触发保护

- [x] 流式上屏期间屏蔽程序自身发出的 `Ctrl+V` 对语音热键的反向触发
- [x] 非流式上屏期间同样屏蔽自身 `Ctrl+V`，避免共享输出链路复发
- [x] suppression guard 结束后继续保留 `350ms` 输出冷却窗口，覆盖尾随按键事件
- [x] streaming commit 后 drain 排队的 voice hotkey command，防止同一轮输出后立刻启动幽灵下一轮
- [x] DirectPaste / NativeEdit 上屏前清理中文 IME composition，避免残留拼音被提交到目标框
- [x] 源码态 live E2E 自动 build 最新 debug exe，避免旧二进制污染测试
- [x] live E2E 提交后增加 `1500ms` 额外观察窗口，读回必须仍等于 final text
- [x] live E2E 新增 `target_duplicate_commit` / `target_extra_commit_fragment` 失败类别
- [x] live E2E 新增 `output_commit_count_mismatch`，`commit_request_count != 1` 直接失败
- [x] live E2E 执行前停掉旧 `ainput-desktop.exe`，避免旧全局键盘钩子污染 `Ctrl+V` 验收
- [x] Windows 源码态 cargo/test/live E2E 通过
- [x] 重新打包到 `dist\ainput-1.0.0-preview.24\`
- [x] dist 包内 synthetic/wav live E2E 通过
- [x] source synthetic live E2E：`tmp\streaming-live-e2e\20260430-175340-791`，3/3 pass，`bad_commit_count=0`，`bad_readback=0`
- [x] source wav live E2E 连续通过：`tmp\streaming-live-e2e\20260430-175123-400`、`tmp\streaming-live-e2e\20260430-175228-953`，均 6/6 pass
- [x] dist synthetic live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-175630-487`，3/3 pass，`bad_commit_count=0`，`bad_readback=0`
- [x] dist wav live E2E 连续通过：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-175703-183`、`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-175813-905`，均 6/6 pass
- [x] dist wav JSON 复核：两轮均 `bad_commit_count=0`，`bad_readback=0`，`failures=0`
