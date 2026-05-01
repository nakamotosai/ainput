# ainput TASKLIST

说明：
每一轮做完，直接勾选；每一轮未完成项保留到下一轮继续推进。

---

## Round 26：HUD 成为流式最终真相源

- [x] 新增 Spec：`specs/streaming-hud-truth-source-v11/`
- [x] 新增 worker -> UI 的 HUD final commit request / ack 协议
- [x] final 文本必须先完整显示到 HUD，再用 HUD ack text 上屏
- [x] live E2E 验收 `hud_final_ack` 先于 `output_commit_request`
- [x] 打包新 preview：`dist\ainput-1.0.0-preview.43`

完成判定：

- [x] `cargo fmt --check` 通过
- [x] `cargo check -p ainput-desktop` 通过
- [x] `cargo test -p ainput-desktop final_commit -- --nocapture` 通过
- [x] `cargo test -p ainput-desktop streaming -- --nocapture` 通过
- [x] `cargo test -p ainput-desktop hotkey -- --nocapture` 通过
- [x] `cargo test -p ainput-desktop` 通过
- [x] 包内 startup idle 通过
- [x] 包内 synthetic / wav live E2E 通过
- [x] 包内 raw corpus 抽样通过
- [x] 最新版本已启动到用户交互桌面：`preview.43` PID `4412`

---

## Round 25：流式尾部 overlap 修正，避免看起来双重上屏

- [x] 新增 Spec：`specs/streaming-tail-overlap-dedup-v10/`
- [x] 从 `preview.40` 日志确认：`streaming-86` 只有一次 paste，但 commit 文本内部重复
- [x] 根因锁定：HUD 尾巴 `我都已经设置了多跳思考的` 和 offline final tail `设置了多跳思考了。` 只有最后 1 字不同，旧的精确 overlap 没识别，导致硬追加
- [x] 增加 fuzzy tail overlap repair
- [x] 单测覆盖真实失败样本
- [x] 打包新 preview：`dist\ainput-1.0.0-preview.41`

完成判定：

- [x] `cargo fmt --check` 通过
- [x] `cargo check -p ainput-desktop` 通过
- [x] `cargo test -p ainput-desktop streaming -- --nocapture` 通过
- [x] `cargo test -p ainput-desktop hotkey -- --nocapture` 通过
- [x] `cargo test -p ainput-desktop` 通过
- [x] 包内 startup idle 通过
- [x] 包内 synthetic / wav live E2E 通过
- [x] 包内 raw corpus 抽样通过
- [x] 最新版本已启动到用户交互桌面：`preview.41` PID `68884`

---

## Round 24：流式幽灵 `Yeah/Okay` 短英文幻听修复

- [x] 新增 Spec：`specs/streaming-ghost-yeah-v9/`
- [x] 确认 `preview.38` 中 `Yeah 。` / `Okay 。` 来自极短低信号流式 session
- [x] 明确本轮只改流式最终提交前的低置信过滤，不动非流式 `Alt+Z`
- [x] 明确不改流式 `Ctrl` 热键规则，不动 `clipboard + Ctrl+V` 上屏主链路
- [x] 增加持续语音 frame 指标
- [x] 增加短英文填充词幻听过滤
- [x] 单测覆盖 `Yeah.` / `Okay.` 低信号 drop
- [x] 打包新 preview 并启动

完成判定：

- [x] `cargo fmt --check` 通过
- [x] `cargo check -p ainput-desktop` 通过
- [x] `cargo test -p ainput-desktop streaming -- --nocapture` 通过
- [x] `cargo test -p ainput-desktop hotkey -- --nocapture` 通过
- [x] `cargo test -p ainput-shell` / `ainput-output` / `ainput-rewrite` 通过
- [x] 包内 startup idle 通过，报告：`tmp\startup-idle-acceptance\20260501-030903-111`
- [x] 包内 synthetic / wav live E2E 通过，报告：`tmp\streaming-live-e2e\20260501-030936-957` / `tmp\streaming-live-e2e\20260501-030948-711`
- [x] 包内 raw corpus 抽样通过，报告：`tmp\streaming-raw-corpus\20260501-031101-723`
- [x] 最新版本已启动到用户交互桌面，PID `55276`

---

## Round 23：流式 HUD 实时追帧和首字首段测速

- [x] 新增 Spec：`specs/streaming-hud-realtime-latency-v8/`
- [x] 明确本轮只修流式 HUD 实时性，不动非流式 `Alt+Z`、不动流式 `Ctrl`、不动 `clipboard + Ctrl+V`
- [x] 确认 raw 失败样本：短样本 final 比最后 HUD partial 多 2 个内容字
- [x] 增加按住期间尾部 soft flush，不上屏、不强制句末标点
- [x] 增加 timeline / final-vs-HUD 差距指标
- [x] raw corpus 不再出现 `raw_tail_late`
- [x] 打包新 preview 并启动

完成判定：

- [x] `cargo check -p ainput-desktop` 通过
- [x] `cargo test -p ainput-desktop streaming -- --nocapture` 通过
- [x] `cargo test -p ainput-shell` 通过
- [x] `scripts\run-streaming-selftest.ps1` 通过
- [x] raw corpus 短句/长句抽样通过
- [x] 包内 live E2E / startup idle 通过
- [x] 最新版本已启动到用户交互桌面

验证记录：

- [x] Windows 真机 `cargo fmt --check` 通过
- [x] Windows 真机 `cargo check -p ainput-desktop` 通过
- [x] Windows 真机 `cargo test -p ainput-desktop streaming -- --nocapture` 通过，31/31 pass
- [x] Windows 真机 `cargo test -p ainput-shell` 通过，6/6 pass
- [x] Windows 真机 `cargo test -p ainput-output` / `cargo test -p ainput-rewrite` 通过
- [x] Windows 真机 `scripts\run-streaming-selftest.ps1` 通过，6/6 pass
- [x] 源码态 raw corpus 通过，报告：`tmp\streaming-raw-corpus\20260501-021906-777`
- [x] latency smoke 通过，报告：`tmp\streaming-latency-benchmark\20260501-022008-231`，`final_extra_chars_max=0`
- [x] 包内 startup idle 通过，报告：`tmp\startup-idle-acceptance\20260501-022233-239`
- [x] 包内 synthetic live E2E 通过，报告：`tmp\streaming-live-e2e\20260501-022247-075`
- [x] 包内 wav live E2E 通过，报告：`tmp\streaming-live-e2e\20260501-022258-698`
- [x] 包内 raw corpus 通过，报告：`tmp\streaming-raw-corpus\20260501-022325-003`
- [x] `dist\ainput-1.0.0-preview.38` 和 `dist\ainput-1.0.0-preview.38.zip` 已生成
- [x] `dist\ainput-1.0.0-preview.38\ainput-desktop.exe` 已启动到用户交互桌面，PID `60120`

---

## Round 22：流式松手 final repair 预算分流

- [x] 新增 Spec：`specs/streaming-final-repair-budget-v7/`
- [x] 明确本轮只修松手后的 final repair 阻塞，不修 HUD 首字速度
- [x] 长句不再整段同步跑 offline final
- [x] 长句只取尾部窗口做 final repair
- [x] tail repair 只有和 streaming final 尾部重叠时才合并，否则 fallback 到 streaming/HUD 文本
- [x] `cargo fmt`
- [x] `cargo check -p ainput-desktop`
- [x] `cargo test -p ainput-desktop offline_final -- --nocapture`
- [x] `cargo test -p ainput-desktop streaming -- --nocapture`
- [x] `scripts\run-streaming-selftest.ps1`
- [x] latency smoke benchmark
- [x] 打包新 preview 并启动

完成判定：

- [x] `sentence_combo_long` 的 `offline_final_elapsed_ms` 明显低于 v6 整段识别的约 `1s`
- [x] 内容门禁不回退
- [x] 当前旧版本可回滚
- [x] 最新版本已启动到用户交互桌面

验证记录：

- [x] latency smoke：`tmp\streaming-latency-benchmark\20260501-014239-785`，`sentence_combo_long.offline_final_elapsed_ms=164ms`
- [x] Windows 真机 `cargo fmt --check` 通过
- [x] Windows 真机 `cargo check -p ainput-desktop` 通过
- [x] Windows 真机 `cargo test -p ainput-desktop offline_final -- --nocapture` 通过，2/2 pass
- [x] Windows 真机 `cargo test -p ainput-desktop streaming -- --nocapture` 通过，31/31 pass
- [x] Windows 真机 `cargo test -p ainput-output` / `cargo test -p ainput-rewrite` / `cargo test -p ainput-shell` 通过
- [x] Windows 真机 `scripts\run-streaming-selftest.ps1` 通过，6/6 pass
- [x] 包内 startup idle 通过，报告：`tmp\startup-idle-acceptance\20260501-015003-544`
- [x] 包内 synthetic live E2E 通过，报告：`tmp\streaming-live-e2e\20260501-015017-595`，3/3 pass
- [x] 包内 wav live E2E 通过，报告：`tmp\streaming-live-e2e\20260501-015159-399`，3/3 pass
- [x] `dist\ainput-1.0.0-preview.37` 和 `dist\ainput-1.0.0-preview.37.zip` 已生成
- [x] `dist\ainput-1.0.0-preview.37\ainput-desktop.exe` 已启动到用户交互桌面，PID `54460`
- [ ] raw corpus 抽样发现既有 out-of-scope 残留：短样本 final 比最后 HUD partial 多 2 个内容字，归入下一轮 HUD 实时追帧/首字速度问题

---

## Round 21：流式延迟 / 模型 / CPU 参数测速

- [x] 新增测速 Spec：`specs/streaming-latency-model-sweep-v6/`
- [x] 明确本轮只测速流式输出，不改非流式 `Alt+Z`、不改流式 `Ctrl`、不改 `clipboard + Ctrl+V`
- [x] replay JSON 增加 final / online / offline / punctuation / processing wall time 指标
- [x] 新增独立 benchmark 脚本，使用临时 `AINPUT_ROOT`，不影响当前运行的 `preview.35`
- [x] `cargo check -p ainput-desktop`
- [x] 小样本 smoke benchmark
- [x] release build 后跑默认测速矩阵
- [x] 基于数据输出瓶颈归因和模型/参数建议

完成判定：

- [x] 产出 `tmp\streaming-latency-benchmark\20260501-012054-119\cases.csv`
- [x] 产出 `tmp\streaming-latency-benchmark\20260501-012054-119\summary_by_variant.csv`
- [x] 产出 `tmp\streaming-latency-benchmark\20260501-012054-119\summary.json`
- [x] 产出 `tmp\streaming-latency-benchmark\20260501-012054-119\SUMMARY.md`
- [x] 结论明确说明慢在 first partial / processing wall / offline final / punctuation / 其他哪一段

验证记录：

- [x] `cargo check -p ainput-desktop` 通过
- [x] `cargo build -p ainput-desktop --release` 通过
- [x] smoke benchmark：`tmp\streaming-latency-benchmark\20260501-011224-851`
- [x] 第一轮正式 benchmark：`tmp\streaming-latency-benchmark\20260501-011259-369`，发现一个 640ms 无效 raw 样本
- [x] 有效正式 benchmark：`tmp\streaming-latency-benchmark\20260501-012054-119`
- [x] 结论写入：`specs\streaming-latency-model-sweep-v6\RESULTS.md`

---

## Round 20：流式 final merge 重复拼接最高优先级修复

- [x] 新增最高优先级 Spec：`specs/streaming-duplicate-final-merge-v5/`
- [x] 真实日志确认：`final_offline_raw_text=你这些分辨率有问题。` 已正确，但 `candidate_display_text` 被拼成 `你最些分辨率有问你这些分辨率有问题。`
- [x] 修复 rollover prefix merge：完整 final replacement 不得再被当成 segment tail 追加
- [x] 保留真正 segment tail 的拼接能力
- [x] 用单元测试覆盖用户真实失败例
- [x] 用 raw `streaming-raw-1777561249467.wav` 回放验证
- [x] 修复 HUD final 与实际上屏末尾句号不一致，避免验收里 HUD/目标框不同步
- [x] 打包新版本 `dist\ainput-1.0.0-preview.35`
- [x] 修复后继续开始 latency/model 测试；模型候选必须优先中英双语，不能选过大模型

验证记录：

- [x] `cargo test -p ainput-desktop rollover_prefix -- --nocapture`：4/4 pass
- [x] `cargo test -p ainput-desktop final_commit -- --nocapture`：1/1 pass
- [x] `cargo test -p ainput-desktop streaming -- --nocapture`：31/31 pass
- [x] `cargo check -p ainput-desktop` 通过
- [x] `cargo test -p ainput-output` / `cargo test -p ainput-rewrite` / `cargo test -p ainput-shell` 通过
- [x] 源码态 raw 回放：`tmp\streaming-raw-corpus\20260501-003749-790`，用户失败样本 final=`你这些分辨率有问题。`
- [x] 包内 raw 回放：`dist\ainput-1.0.0-preview.35\tmp\streaming-raw-corpus\20260501-004000-827`，2/2 pass
- [x] 包内 synthetic live E2E：`dist\ainput-1.0.0-preview.35\tmp\streaming-live-e2e\20260501-004033-262`，3/3 pass
- [x] 包内 wav live E2E：`dist\ainput-1.0.0-preview.35\tmp\streaming-live-e2e\20260501-004315-592`，6/6 pass
- [x] 包内 startup idle：`dist\ainput-1.0.0-preview.35\tmp\startup-idle-acceptance\20260501-004427-369`，pass

---

## Round 19：启动空闲误触发修复

- [x] 流式模式必须继续使用 `ainput.toml` 里的 `hotkeys.voice_input`，不得把配置热键覆盖成单独 `Ctrl`
- [x] 启动全局热键 hook 时先 reset 热键状态，并设置启动冷却窗口，避免启动瞬间残留键盘状态触发录音
- [x] keyboard primary / modifier-only / mouse-middle 三类语音触发必须写入来源日志，方便下次定位
- [x] 截图热键 release 只有在本轮 screenshot active 时才吞掉，不能吞无来源的普通 `Alt+X` release
- [x] 新增 `scripts\run-startup-idle-acceptance.ps1`，启动后不发送任何热键，扫描新增日志和 raw captures
- [x] startup idle 源码态连续通过
- [x] startup idle dist 包内连续通过
- [x] 回归 exactly-once streaming live E2E，确认不破坏上轮修复
- [x] 重新打包到 `dist\`

完成判定：

- [x] 打开新版后，不按语音热键时不会自动开始录音、不会显示语音 HUD、不会向当前窗口上屏
- [x] 日志中启动空闲期间没有 `start microphone recording` / `streaming microphone armed` / `output delivery timing`
- [x] 默认配置 `Alt+Z` 下，普通 `Ctrl` 操作不会启动流式识别

验证记录：

- [x] Windows 真机 `cargo check -p ainput-desktop` 已通过
- [x] Windows 真机 `cargo test -p ainput-desktop hotkey` 已通过，4/4 pass
- [x] Windows 真机 `cargo test -p ainput-desktop acceptance` 已通过，5/5 pass
- [x] Windows 真机 `cargo test -p ainput-desktop streaming` 已通过，30/30 pass
- [x] Windows 真机 `cargo test -p ainput-output` / `cargo test -p ainput-shell` / `cargo test -p ainput-rewrite` 已通过
- [x] Windows 真机 `scripts\run-streaming-selftest.ps1` 已通过，6/6 cases pass
- [x] 源码态 startup idle 通过，报告：`tmp\startup-idle-acceptance\20260430-195103-794`，2/2 pass，`expected_voice_hotkey=Alt+Z`
- [x] 包内 startup idle 通过，报告：`dist\ainput-1.0.0-preview.24\tmp\startup-idle-acceptance\20260430-195614-253`，3/3 pass，`expected_voice_hotkey=Alt+Z`
- [x] 包内 synthetic live E2E 已通过，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-195810-447`，3/3 pass
- [x] 包内 wav live E2E 已通过，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-195830-871`，6/6 pass
- [x] `dist\ainput-1.0.0-preview.24.zip` 已重建；最终包内脚本可直接运行

---

## Round 18：Exactly-once 上屏与自触发保护

- [x] 程序自身执行 `Ctrl+V` 粘贴期间，语音热键钩子必须忽略这组按键，避免粘贴动作反向触发第二轮语音
- [x] suppression guard 结束后继续保留 `350ms` 输出冷却窗口，覆盖尾随按键事件
- [x] streaming commit 后 drain 排队的 voice hotkey command，防止同一轮输出后立刻启动幽灵下一轮
- [x] DirectPaste / NativeEdit 上屏前清理中文 IME composition，避免 `wan`、`ngl`、`us`、`gxi` 这类残留拼音混进目标框
- [x] 流式 DirectPaste 前稳定等待提升到 `120ms`，减少最终粘贴时目标控件未就绪导致的尾字截断
- [x] 流式最终提交一次 release 只能产生一次 commit；目标框不得出现 `final+final` 或 `final+错误片段`
- [x] live E2E 提交后追加 `1500ms` 观察窗口，重复上屏失败类别必须明确为 `target_duplicate_commit` 或 `target_extra_commit_fragment`
- [x] `commit_request_count != 1` 直接失败为 `output_commit_count_mismatch`
- [x] live E2E 执行前停掉旧 `ainput-desktop.exe` 托盘进程，并复查残留；源码态 E2E 自动先 build 最新 debug exe
- [x] 保持非流式主链效果不变，只给共享输出热键增加保护
- [x] Windows 源码态 cargo/test/live E2E 通过
- [x] 重新打包到 `dist\ainput-1.0.0-preview.24\`
- [x] dist 包内 synthetic/wav live E2E 通过

完成判定：

- [x] 输出期间语音热键 suppression 单测通过，`Ctrl` 作为按住说话热键时不会吃掉程序自己的 `Ctrl+V`
- [x] 每个 live E2E case 的 `commit_request_count == 1`
- [x] 提交后 1500ms 观察窗口内，target readback 仍等于 final text
- [x] 旧托盘进程不会再污染验收脚本

验证记录：

- [x] Windows 真机 `cargo check -p ainput-desktop` 已通过
- [x] Windows 真机 `cargo test -p ainput-desktop hotkey` 已通过，4/4 pass
- [x] Windows 真机 `cargo test -p ainput-desktop acceptance` 已通过，5/5 pass
- [x] Windows 真机 `cargo test -p ainput-desktop streaming` 已通过，30/30 pass
- [x] Windows 真机 `cargo test -p ainput-output` 已通过，9/9 pass
- [x] Windows 真机 `cargo test -p ainput-shell` 已通过，6/6 pass
- [x] Windows 真机 `cargo test -p ainput-rewrite` 已通过，16/16 pass
- [x] Windows 真机 `scripts\run-streaming-selftest.ps1` 已通过，6/6 pass
- [x] source synthetic live E2E 已通过：`tmp\streaming-live-e2e\20260430-175340-791`，3/3 pass，`bad_commit_count=0`，`bad_readback=0`
- [x] source wav live E2E 连续通过：`tmp\streaming-live-e2e\20260430-175123-400`、`tmp\streaming-live-e2e\20260430-175228-953`，均 6/6 pass，`bad_commit_count=0`，`bad_readback=0`
- [x] dist synthetic live E2E 已通过：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-175630-487`，3/3 pass，`bad_commit_count=0`，`bad_readback=0`
- [x] dist wav live E2E 连续通过：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-175703-183`、`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-175813-905`，均 6/6 pass，`bad_commit_count=0`，`bad_readback=0`
- [x] `dist\ainput-1.0.0-preview.24.zip` 已重建，大小 `456367554` bytes，时间 `2026-04-30 17:55:57`
- [x] raw corpus 本轮确认未覆盖：当前 `logs\streaming-raw-captures` 没有足够大的 raw wav；本轮问题属于输出 exactly-once，不依赖 raw ASR 回放

---

## Round 17：语义标点与尾字保护

- [x] 本轮明确不接入 AI rewrite；`voice.streaming.ai_rewrite.enabled = false` 保持不变
- [x] 停顿不再等同于句末；`pause_ms` 只能触发尾音 flush 和识别器 reset，不能强制补 `。！？；`
- [x] 停顿边界不再把未完成句子整体冻结；只有已有明确句末标点的前缀才进入不可改 committed prefix
- [x] 实时标点继续常驻，但句末标点必须来自文本语义，不得来自停顿事件本身
- [x] 标点输出统一去重，禁止 `，，`、`,,`、`。。`、`？？`、`？！`、`，。` 等重复或冲突标点
- [x] final 提交不能比最后 HUD partial 少内容字；`了/啊/呢/吧/吗/呀/嘛/哦/噢/诶` 等尾字零容忍丢失
- [x] raw corpus 验收新增尾字丢失、重复标点、pause 强制句末三类门禁
- [x] Windows 源码态 raw corpus 抽样覆盖短句、长句和语气尾字并通过
- [x] Windows 源码态 synthetic/wav live E2E 通过
- [x] 重新打包到 `dist\ainput-1.0.0-preview.24\`
- [x] dist 包内 raw corpus 与 synthetic/wav live E2E 通过

完成判定：

- [x] `endpoint_rollover` 不得在上一条 partial 无句末标点时新增尾部 `。！？；`
- [x] 任意 partial/final 不得出现重复或冲突标点
- [x] final content chars 不得少于最后一个非空 HUD partial
- [x] 最后 HUD partial 的语气尾字必须出现在 final
- [x] HUD final、最终上屏、目标框读回仍保持同一文本

验证记录：

- [x] Windows 真机 `cargo check -p ainput-desktop` 已通过
- [x] Windows 真机 `cargo test -p ainput-desktop streaming` 已通过，30/30 pass
- [x] Windows 真机 `cargo test -p ainput-desktop acceptance` 已通过，1/1 pass
- [x] Windows 真机 `cargo test -p ainput-output` 已通过，9/9 pass
- [x] Windows 真机 `cargo test -p ainput-shell` 已通过，6/6 pass
- [x] Windows 真机 `cargo test -p ainput-rewrite` 已通过，16/16 pass
- [x] Windows 真机 `scripts\run-streaming-selftest.ps1` 已通过，6/6 pass
- [x] Windows 源码态 `scripts\run-streaming-raw-corpus.ps1` 已通过，报告：`tmp\streaming-raw-corpus\20260430-145208-844`
- [x] Windows 源码态 `scripts\run-streaming-live-e2e.ps1 -Synthetic -InteractiveTask` 已通过，报告：`tmp\streaming-live-e2e\20260430-145346-906`
- [x] Windows 源码态 `scripts\run-streaming-live-e2e.ps1 -Wav -InteractiveTask` 已通过，报告：`tmp\streaming-live-e2e\20260430-145402-148`
- [x] dist 包内 `scripts\run-streaming-raw-corpus.ps1` 已通过，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-raw-corpus\20260430-150531-347`
- [x] dist 包内 `scripts\run-streaming-live-e2e.ps1 -Synthetic -InteractiveTask` 已通过，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-150531-487`
- [x] dist 包内 `scripts\run-streaming-live-e2e.ps1 -Wav -InteractiveTask` 已通过，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-150548-099`
- [x] `dist\ainput-1.0.0-preview.24.zip` 已重建，大小 `456359936` bytes，时间 `2026-04-30 15:05:14`

---

## Round 16：HUD 单行黑色胶囊动态面板

- [x] HUD 默认样式改为黑色半透明背景、白色文字、居中显示
- [x] 流式 HUD 不再使用固定大面板；每次显示按当前文字宽度动态扩展
- [x] HUD 单行显示，不使用自动换行；字数变多时从任务栏上方中心向两边延长
- [x] 新一轮录音 reset 后从空 HUD 和小底板重新开始，不继承上一句宽度
- [x] live E2E 门禁改为检查 `max_center_x_delta_px`，允许宽度动态变化但不允许中心漂移
- [x] live E2E 增加 `hud_white_panel`、`hud_multiline_panel`、`hud_short_text_wide_panel` 三个视觉回归门禁
- [x] 打包脚本不再从旧 dist 保留白色 HUD 的尺寸/颜色/对齐配置，只保留字体和显示保留时间
- [x] Windows 源码态 synthetic live E2E 通过
- [x] Windows 源码态 wav live E2E 通过
- [x] 重新打包到 `dist\ainput-1.0.0-preview.24\`
- [x] dist 包内 synthetic/wav live E2E 通过

完成判定：

- [x] 一个字/短文本时不出现大白面板，短文本过宽会失败为 `hud_short_text_wide_panel`
- [x] HUD 背景不能接近白色，否则失败为 `hud_white_panel`
- [x] HUD 高度必须保持单行胶囊，不允许回到多行大面板，否则失败为 `hud_multiline_panel`
- [x] HUD 中心必须稳定，允许宽度变化但不允许从中心漂走
- [x] HUD active 期间不能闪烁：`alpha_drop_count == 0` 且 `invisible_sample_count == 0`

验证记录：

- [x] Windows 真机 `cargo check -p ainput-desktop` 已通过
- [x] Windows 真机 `cargo test -p ainput-desktop acceptance` 已通过，1/1 pass
- [x] Windows 真机 `cargo test -p ainput-shell` 已通过，6/6 pass
- [x] Windows 真机 `cargo test -p ainput-desktop streaming` 已通过，24/24 pass
- [x] Windows 真机 `cargo test -p ainput-output` 已通过，9/9 pass
- [x] Windows 真机 `scripts\run-streaming-selftest.ps1` 已通过，6/6 pass
- [x] 源码态 synthetic live E2E：`tmp\streaming-live-e2e\20260430-140234-142`，3/3 pass，`hud_center` 最大 `0/0`，`hud_panel` 全 `0/0/0`
- [x] 源码态 wav live E2E：`tmp\streaming-live-e2e\20260430-140252-617`，6/6 pass，`hud_center` 最大 `1/0`，`hud_panel` 全 `0/0/0`
- [x] 包内 synthetic live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-140634-134`，3/3 pass，`hud_center` 最大 `0/0`，`hud_panel` 全 `0/0/0`
- [x] 包内 wav live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-140652-187`，6/6 pass，`hud_center` 最大 `1/0`，`hud_panel` 全 `0/0/0`
- [x] `dist\ainput-1.0.0-preview.24.zip` 已重建，大小 `456341974` bytes，时间 `2026-04-30 14:06:21`

---

## Round 15：按住停顿补尾字与实时标点

- [x] 本轮明确不接入 AI rewrite；`voice.streaming.ai_rewrite.enabled = false` 保持不变
- [x] 按住不松但语音停顿时启用应用层 soft finalize，只刷新 HUD/稳定前缀，不提前上屏
- [x] soft finalize 前补短静音并调用 streaming `input_finished()`，让最后几个字不用等松手才出现
- [x] 停顿边界文本跑常驻标点模型，内容字数变少则拒绝标点结果
- [x] 实时 preview 恢复标点模型，但加内容保护；标点可以出现，尾巴不能被模型裁短
- [x] 新增真实 raw wav 抽样回放脚本 `scripts\run-streaming-raw-corpus.ps1`
- [x] 当前配置启用 `[voice.streaming.endpoint]`，`pause_ms = 720`，`min_segment_ms = 900`，`tail_padding_ms = 480`
- [x] Windows 源码态 raw corpus 抽样覆盖短句和长句，通过尾字和标点门禁
- [x] Windows 前台 E2E 通过
- [x] 重新打包到 `dist\ainput-1.0.0-preview.24\`
- [x] dist 包内 raw corpus 抽样和前台 E2E 通过

完成判定：

- [x] 抽样真实 raw wav 不必全跑 20 条，但必须包含短句和长句
- [x] 对非空语音，最后一个 HUD partial 与 final text 的内容字差距不得超过 `1`
- [x] 对超过 `1200ms` 且 final 带标点的语音，final 前至少一个 partial 必须已经显示标点
- [x] `endpoint_rollover` / soft finalize 只更新 HUD，不提前上屏
- [x] AI rewrite 状态必须保持关闭，不能混入本轮验收

验证记录：

- [x] Windows 真机 `cargo check -p ainput-desktop` 已通过
- [x] Windows 真机 `cargo test -p ainput-desktop streaming` 已通过，24/24 pass
- [x] Windows 真机 `cargo test -p ainput-output` 已通过，9/9 pass
- [x] Windows 真机 `cargo test -p ainput-desktop acceptance` 已通过，1/1 pass
- [x] Windows 真机 `scripts\run-streaming-selftest.ps1` 已通过，6/6 pass
- [x] 源码态 raw corpus 抽样：`tmp\streaming-raw-corpus\20260430-121618-781`，4 条 pass，短句+长句覆盖，`final_extra_chars=0`，partial/final 均有标点
- [x] 源码态 synthetic live E2E：`tmp\streaming-live-e2e\20260430-121740-655`，3/3 pass，HUD move/size/flash 全为 0
- [x] 源码态 wav live E2E：`tmp\streaming-live-e2e\20260430-121756-340`，6/6 pass，HUD move/size/flash 全为 0
- [x] 包内 raw corpus fixture 抽样：`dist\ainput-1.0.0-preview.24\tmp\streaming-raw-corpus\20260430-123441-839`，4 条 pass，短句+长句覆盖，`final_extra_chars=0`，partial/final 均有标点
- [x] 包内 synthetic live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-123514-026`，3/3 pass，HUD move/size/flash 全为 0
- [x] 包内 wav live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-123527-953`，6/6 pass，HUD move/size/flash 全为 0
- [x] `dist\ainput-1.0.0-preview.24.zip` 已重建，大小 `456340280` bytes，时间 `2026-04-30 12:34:14`
- [x] 已修复打包脚本：重打包时会在 zip 之后恢复 `dist\...\logs\streaming-raw-captures\`，避免再次清掉本地近 20 条 raw 留样；本轮旧 dist 中的 20 条样本已被重打包清空，当前只能后续重新积累

---

## Round 14：流式松手提交、HUD 残影与原始录音留样门禁

- [x] live E2E 每个 case 开始后采样 `hud_after_case_reset`，HUD target/display 不为空直接失败为 `hud_stale_text`
- [x] live E2E final HUD 改成真实生产一致的非 persistent 完成态，并追加 `hud_after_commit_hold`
- [x] live E2E 要求提交后短暂停留窗口内 HUD 仍可见且显示 final text，否则失败为 `hud_final_hold_missing` / `hud_text_diverged`
- [x] `probe-streaming-live` 结束时同步写入 `logs\streaming-raw-captures\`，报告返回 wav/json 路径，方便验证留样后门真的落盘
- [x] 原始录音留样文件名增加同毫秒冲突保护，避免连续快速保存时互相覆盖
- [x] Windows 真项目与本轮镜像哈希完全对齐
- [x] Windows 源码态测试、前台 E2E 和原始留样产物检查通过
- [x] 重新打包到 `dist\ainput-1.0.0-preview.24\`
- [x] dist 包内前台 E2E 和原始留样产物检查通过

完成判定：

- [x] 旧剪贴板哨兵不得进入目标输入框，失败类别必须是 `clipboard_stale_paste`
- [x] 新一句开始时 HUD 不得显示上一句 target/display
- [x] 松手后顺序必须是 drain/final decode -> HUD final flush -> commit，上屏前不得暴力截断
- [x] 提交后 HUD 必须短暂停留最终文本，不能松手瞬间消失
- [x] `logs\streaming-raw-captures\` 至少生成一组真实录音 wav/json，且保留上限为最近 20 组

验证记录：

- [x] 源码态 synthetic live E2E：`tmp\streaming-live-e2e\20260430-110516-575`，3/3 pass，HUD move/size/flash 全为 0
- [x] 源码态 wav live E2E：`tmp\streaming-live-e2e\20260430-110528-184`，6/6 pass，HUD move/size/flash 全为 0
- [x] 源码态真实麦克风 probe 留样：`logs\streaming-raw-captures\streaming-raw-1777514764552.wav` + `.json`
- [x] 包内 synthetic live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-111750-538`，3/3 pass，HUD move/size/flash 全为 0
- [x] 包内 wav live E2E：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-111805-306`，6/6 pass，HUD move/size/flash 全为 0
- [x] 包内真实麦克风 probe 留样：`dist\ainput-1.0.0-preview.24\logs\streaming-raw-captures\streaming-raw-1777515538735.wav` + `.json`

---

## Round 13：流式真实热键路径修复

- [x] 松手后先等待热键修饰键真正释放，再开始上屏
- [x] 松手收尾改成语音活动驱动 drain，最长等待 `900ms`，避免暴力截断最后一个字
- [x] 最终解码静音 padding 增加到 `720ms`
- [x] 流式 Ctrl+V fallback 不再提前恢复旧剪贴板，避免把旧剪贴板内容贴上屏
- [x] live E2E 提交前写入旧剪贴板哨兵，若目标框收到哨兵则报 `clipboard_stale_paste`
- [x] `StreamingStarted` 清空 HUD 内部 target/display/message，禁止新一句先显示上一句内容
- [x] 每次流式原始录音异步保存到 `logs\streaming-raw-captures\`，自动只保留最近 20 条 wav + json

完成判定：

- [x] `cargo test -p ainput-output`
- [x] `cargo test -p ainput-desktop acceptance`
- [x] `cargo test -p ainput-desktop streaming`
- [x] `cargo test -p ainput-desktop worker::tests::raw_capture_writer_keeps_only_recent_twenty_wavs`
- [x] 源码态 `run-streaming-selftest.ps1` 通过，6/6 cases pass
- [x] 源码态 `run-streaming-live-e2e.ps1 -Synthetic -InteractiveTask` 通过，3/3 cases pass，报告：`tmp\streaming-live-e2e\20260430-104650-682`
- [x] 源码态 `run-streaming-live-e2e.ps1 -Wav -InteractiveTask` 通过，6/6 cases pass，报告：`tmp\streaming-live-e2e\20260430-104656-723`
- [x] 源码态 live E2E timeline 显示全部提交为 `DirectPaste`，且目标框收到识别文本，不是旧剪贴板哨兵

---

## Round 12：流式真实前台自测闭环

- [x] 建立 `specs/streaming-live-e2e-acceptance/` 规格包
- [x] 写入 E2E `SPEC.md`
- [x] 写入 E2E `PLAN.md`
- [x] 写入 E2E `REVIEW.md`
- [x] 写入 E2E `TASKLIST.md`
- [x] Phase 1：增加 synthetic live E2E Acceptance Trace
- [x] Phase 2：让 HUD 实际显示可被采样（截图后续补）
- [x] Phase 3：增加专用目标输入框和 readback
- [x] Phase 4：增加真实桌面会话执行入口（InteractiveTask）
- [x] Phase 5：让固定 wav 驱动真实 HUD 和上屏
- [x] Phase 6：增加 synthetic partial 压测 HUD 抖动和吃字
- [ ] Phase 7：采集用户真实声音 fixture
- [x] Phase 8：把 live E2E 纳入 dist 验收

完成判定：

- [x] Codex 通过 SSH 发起测试，但由用户桌面会话里的 ainput 执行
- [x] 每轮报告包含 worker partial/final、HUD target/display、output commit、target readback
- [x] HUD final flush 与最终上屏文本一致
- [x] HUD 抖动/闪烁作为硬门禁：位移/尺寸变化超过 3px 失败，alpha 下降或不可见采样失败
- [x] 目标输入框 readback 等于最终提交文本
- [x] 出错时能明确归类到 HUD、上屏或目标读回问题；ASR 类错误留给 wav/live voice 阶段
- [x] `run-streaming-live-e2e.ps1 -Version <current> -InteractiveTask` 通过后，才允许说“synthetic 真实前台验收通过”
- [x] 源码态 `run-streaming-live-e2e.ps1 -Synthetic -InteractiveTask` 通过，3/3 cases pass
- [x] 源码态 `run-streaming-live-e2e.ps1 -Wav -InteractiveTask` 通过，6/6 cases pass
- [x] 包内 `dist\ainput-1.0.0-preview.24\scripts\run-streaming-live-e2e.ps1 -Synthetic -InteractiveTask` 通过，3/3 cases pass
- [x] 包内 `dist\ainput-1.0.0-preview.24\scripts\run-streaming-live-e2e.ps1 -Wav -InteractiveTask` 通过，6/6 cases pass

---

## Round 11：流式实时改写 V3

- [x] 建立 `specs/streaming-realtime-rewrite-v3/` 规格包
- [x] 写入 V3 `SPEC.md`
- [x] 写入 V3 `PLAN.md`
- [x] 写入 V3 `REVIEW.md`
- [x] 写入 V3 `TASKLIST.md`
- [x] Phase 1：重做稳定状态机
- [x] Phase 2：接入短停顿 endpointing
- [x] Phase 3：恢复 HUD 双缓冲逐字追目标
- [x] Phase 4：AI 尾巴改写 revision/stale 化
- [x] Phase 5：扩展输入上下文快照
- [x] Phase 6：抽出输出适配层
- [x] Phase 7：升级 streaming 回归指标
- [x] Phase 8：README / TASKLIST / 旧 specs 收口

完成判定：

- [x] `极速语音识别` 行为保持不变
- [x] 流式模式已区分 committed / stable / volatile / rewrite candidate
- [x] 400-700ms 短停顿能稳定切段
- [x] HUD 不再大块跳字，final 可立即 flush
- [x] AI rewrite 不阻塞热路径，stale response 不可覆盖新文本
- [x] 固定 wav 回归不再只按字符数放行
- [x] Windows 前台 synthetic 目标输入框验收通过

---

## Round 10.2：按键精灵回放防抖与轮数持久化

- [x] 修复按键精灵回放时鼠标轻微抖动就误暂停
- [x] 保留 `1` 到 `5` 轮快捷项并新增自定义回放轮数输入
- [x] 将按键精灵回放轮数写回正式 `ainput.toml`
- [x] 在 Windows 目标机重新编译并打包新的便携版

完成判定：

- [x] 单次轻微鼠标抖动不会再直接触发回放自动暂停
- [x] 明确鼠标接管仍会触发自动暂停
- [x] 自定义回放轮数立即生效，重启后继续沿用
- [x] 产出新的 `dist\ainput-1.0.13\` 与 `dist\ainput-1.0.13.zip`

---

## Round 10.1：语音/录屏稳定性修复

- [x] 修复语音 worker 失联后托盘蓝图标长期不回收
- [x] 托盘菜单增加 `重新启动`
- [x] 修复录屏启动失败 / 导出失败只写日志、不回前台错误态
- [x] 在 Windows 目标机重新编译 `ainput-desktop`

完成判定：

- [x] 语音线程失联时不会再只剩蓝图标卡住
- [x] 录屏失败时托盘会明确回到错误态，允许重试
- [x] 托盘菜单可直接重启当前常驻进程

---

## Round 0：方案冻结与启动资产

- [x] 产品名固定为 `ainput`
- [x] 项目根目录固定为 `C:\Users\sai\ainput`
- [x] 技术路线固定为 `Rust 主程序 + sherpa-onnx Rust API`
- [x] ASR 模型固定为 `SenseVoiceSmall`
- [x] 最终运行时默认不引入 Python
- [x] 建立 `AGENTS.md`
- [x] 建立 `README.md`
- [x] 建立 `SPEC.md`
- [x] 建立 `PLAN.md`
- [x] 建立 `TASKLIST.md`
- [x] 建立 `ARCHITECTURE.md`
- [x] 建立 `DECISIONS.md`
- [x] 建立 `WORKFLOW.md`
- [x] 建立 `OPLOG.md`
- [x] 建立 Rust workspace 骨架文件

完成判定：

- [x] 新会话进入目录后可直接按文档继续

---

## Round 1：Rust workspace 骨架

- [x] 建立 `apps/ainput-desktop`
- [x] 建立 `crates/ainput-shell`
- [x] 建立 `crates/ainput-audio`
- [x] 建立 `crates/ainput-asr`
- [x] 建立 `crates/ainput-rewrite`
- [x] 建立 `crates/ainput-output`
- [x] 建立 `crates/ainput-data`
- [x] 建立基础配置加载
- [x] 建立基础日志初始化
- [x] 跑通 `cargo check`

完成判定：

- [x] workspace 可检查通过

---

## Round 2：ASR 链路选型与最小打通

- [x] 确认使用 sherpa-onnx Rust API 还是 C API 落地
- [x] 固定模型目录约定
- [x] 打通 wav 文件到文本的最小识别
- [x] 打通麦克风录音到文本的最小识别
- [x] 建立 ASR 错误日志
- [x] 记录性能观察

完成判定：

- [x] 本机可得到一条离线识别文本

---

## Round 3：热键、录音、状态机

- [x] 设计按住 `Alt+Z` 说话状态机
- [x] 接入 `Alt+Z` 全局热键
- [x] 接入麦克风录音
- [x] 处理开始/停止录音
- [x] 建立失败恢复策略

完成判定：

- [x] 热键按住说话主流程可用

---

## Round 4：输出注入

- [x] 剪贴板输出
- [x] 自动粘贴输出
- [x] 插入失败降级
- [x] 最近结果缓存

完成判定：

- [ ] 至少一个 IDE 输入框可用
- [ ] 至少一个浏览器输入框可用

---

## Round 5：极简 UI 与托盘

- [x] 托盘入口
- [x] 录音状态反馈
- [ ] 最近结果预览
- [x] 最小设置入口

完成判定：

- [x] UI 足够轻，且不干扰主流程

---

## Round 5.5：截图能力

- [x] 设计 `Alt+X` 截图状态机
- [x] 接入 `Alt+X` 全局热键
- [x] 实现冻结全屏与框选
- [x] 实现图片写入剪贴板
- [x] 实现托盘“截图后自动保存到桌面”开关
- [x] 实现桌面 PNG 自动保存
- [x] 完成最小本地截图回归

完成判定：

- [x] `Alt+X` 可进入截图框选态
- [ ] 截图能进入剪贴板
- [ ] 勾选后可额外保存到桌面

---

## Round 6：后续增强能力

- [ ] 设计术语数据结构
- [ ] 建立内置词表
- [ ] 建立用户词表
- [ ] 实现大小写规范
- [ ] 实现空格规则
- [ ] 完成至少 20 个术语
- [ ] 定义模式系统
- [ ] 实现提示词转换第一版

完成判定：

- [ ] 识别后文本增强能力可单独启用

---

## Round 7：模板与账本

- [ ] `debug_root_cause`
- [ ] `fix_minimal`
- [ ] `review_code`
- [ ] `refactor_safely`
- [ ] `write_tests`
- [ ] `explain_code`
- [ ] `spec_first`
- [ ] `command_only`
- [ ] 完成至少 10 条账本模板

完成判定：

- [ ] 至少 8 个场景模板和 10 条账本模板可用

---

## Round 8：打包与回归

- [x] 打包方案确认
- [x] 安装包方案确认
- [x] 安装脚本
- [x] 卸载脚本
- [x] 生成 setup.exe
- [x] 安装/卸载回归一轮
- [x] 模型部署说明
- [x] 回归样例
- [x] 日常实测一轮

完成判定：

- [x] 可形成长期自用版本

---

## Round 9：稳定性与产品化重构

### 9.1 标点与上下文判断

- [x] 梳理当前句号策略在普通输入框与终端输入区的差异
- [x] 为“未知上下文”定义新的保守输出规则
- [x] 为终端 / 控制台 / 类 TTY 输入区建立单独策略
- [x] 为上下文判断结果增加日志与自检入口

完成判定：

- [x] 普通输入框与终端输入区都具备可解释、稳定的句号行为

### 9.2 术语与学习系统

- [x] 设计新的内置词库 / 用户词库 / 候选池 / 激活映射模型
- [x] 预置 AI 编程高频英文术语资产
- [x] 废弃现有隐式学习主流程
- [x] 增加学习结果可见反馈
- [x] 增加学习状态持久化与迁移策略
- [x] 设计“语音触发动作”规则入口
- [x] 实现句尾“笑死” -> `[破涕为笑]` 的 emoji 触发
- [x] 将 emoji 触发与光标末尾判断联动
- [x] 补齐句尾命中 / 句中不命中 / 终端不命中的最小测试

完成判定：

- [x] 用户可明确知道学习是否成功、哪些词已生效、重装后哪些资产会保留

### 9.3 语音历史

- [x] 新增语音历史文件
- [x] 实现一行一条写入
- [x] 实现最近 500 条滚动裁剪
- [x] 区分 `last_result.txt` 与历史文件职责

完成判定：

- [x] 最近 500 条语音文本可稳定查看

### 9.4 托盘与设置

- [x] 重做托盘信息架构
- [x] 分离语音、截图、通用入口
- [x] 定义哪些设置留在托盘，哪些进入配置文件
- [x] 判断是否需要轻量设置面板

完成判定：

- [x] 托盘结构清晰，设置入口合理，不再混杂

### 9.5 热键系统

- [x] 切换主热键到 Windows 原生热键注册接口
- [x] 支持热键配置落盘
- [x] 支持热键冲突检测与失败反馈
- [x] 把鼠标中键长按从主热键逻辑中剥离

完成判定：

- [x] 热键可改、可诊断、可稳定恢复

### 9.6 系统接口与截图链路

- [x] 重新审计语音与截图所用系统接口
- [x] 优先切换到 Windows 原生稳定接口
- [x] 修复截图剪贴板句柄所有权问题
- [x] 补齐截图复制与保存的真实回归

完成判定：

- [x] 语音与截图两条链路的系统接口选择清晰且稳定

### 9.7 运行时治理

- [x] 删除或接通伪配置项
- [x] 收紧根目录发现逻辑
- [x] 降低空闲动画 tick 资源消耗
- [x] 收口版本号来源
- [x] 跑通 `fmt` / `clippy` / 自检命令

完成判定：

- [x] 工程状态与对外文档一致，不再存在明显漂移

### 9.8 后台维护与前台主链路解耦

- [x] 将语音历史与最近结果持久化移出前台识别输出路径
- [x] 保证截图主链路不依赖后台维护任务完成
- [x] 移除默认后台资源心跳
- [x] 在文档中明确前后台职责边界

完成判定：

- [x] 后台记录日志和维护动作不会阻塞语音识别与截图主链路

### 9.9 单实例接管

- [x] 在桌面常驻入口增加单实例检查
- [x] 启动第二个实例时先结束旧实例
- [x] 确认旧实例退出后再拉起新实例
- [x] 避免调试命令被单实例逻辑误伤

完成判定：

- [x] 任意时刻系统内只保留一个常驻的 `ainput-desktop.exe`

### 9.10 截图遮罩反馈

- [x] 截图进入时增加全屏半透明暗膜
- [x] 选区内保持原始截图亮度
- [x] 选区边框改为 1px 白色边框
- [x] 退出截图时遮罩随窗口立即消失
- [x] 不引入动画效果

完成判定：

- [x] 进入截图态后，用户可立即感知屏幕变暗
- [x] 拖选时只有选区保持原亮度，外部区域持续变暗
