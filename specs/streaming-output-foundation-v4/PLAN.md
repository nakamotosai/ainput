# Streaming Output Foundation V4 PLAN

更新时间：2026-04-30

## 执行原则

- 只动流式输出相关路径。
- 不动非流式 `Alt+Z` 主链路。
- 不改 clipboard + Ctrl+V 输出机制。
- 不改变单独 `Ctrl` 作为流式热键的产品规则。
- 不接入 AI rewrite。
- GPU 暂不纳入本轮，不做 provider probe，不改依赖。
- 不覆盖 `preview.31`，每次打包生成新 preview。
- 每个阶段都必须有对应验收，不能只用编译通过收口。

## Phase 0: 冻结基线和可回滚规则

目标：
- 把 `preview.31` 固定为回滚点。
- 确认当前运行版本、dist 目录和 zip 存在。
- 记录当前问题证据，避免下一轮又按主观感觉改。

修改：
- 新建 `specs/streaming-output-foundation-v4/`。
- 写入 `SPEC.md`、`PLAN.md`、`TASKLIST.md`。
- 不改源码。

验证：

```powershell
Test-Path .\dist\ainput-1.0.0-preview.31\ainput-desktop.exe
Test-Path .\dist\ainput-1.0.0-preview.31.zip
git status --short
```

完成判定：
- v4 规格包存在。
- `preview.31` 未被触碰。
- 后续实现以新 preview 产物为目标。

## Phase 1: 热键边界和日志事实

目标：
- 不再混淆 `Alt+Z` 和 `Ctrl`。
- 继续保护系统原生 Ctrl 快捷键。

允许修改：
- `apps/ainput-desktop/src/hotkey.rs`
- `apps/ainput-desktop/src/main.rs`
- `apps/ainput-desktop/src/worker.rs`
- 日志字段和 acceptance trace 字段

禁止修改：
- 不把流式热键改成 `Alt+Z`。
- 不把非流式热键改成 `Ctrl`。
- 不吞掉 Ctrl 原生事件。

最小验证：

```powershell
cargo check -p ainput-desktop
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-startup-idle-acceptance.ps1 -Version <new-preview>
```

前台验证：
- Notepad 里 `Ctrl+A/C/V` 正常。
- 日志无误触发录音。
- 日志清楚写出 `fast_hotkey=Alt+Z` 与 `streaming_effective_hotkey=Ctrl`。

## Phase 2: 音频预滚和首字延迟

目标：
- 让 `preroll_ms` 变成真实有效的按键前短 ring buffer。
- 降低 HUD 首次出字等待。

修改：
- 流式模式增加 standby audio ring buffer。
- ring buffer 不识别、不上屏、不写 raw，只在 `Ctrl` press 后拼入当前 session。
- 增加 first audio、first partial、first HUD 的 trace。

验证：

```powershell
cargo test -p ainput-desktop streaming
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-startup-idle-acceptance.ps1 -Version <new-preview>
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-raw-corpus.ps1 -Version <new-preview> -ShortCount 1 -LongCount 1
```

完成判定：
- startup idle 不自动录音。
- first partial 硬目标 `<= 900ms`，若未达标必须报告瓶颈。
- ring buffer 没有长期落盘。

## Phase 3: Streaming 状态和尾字保护

目标：
- final 不能再比 HUD 少内容字。
- 语气词和尾字不静默丢失。

修改：
- 收敛 `committed_prefix / stable_live / volatile_tail / final_candidate`。
- 加 `tail_particle_guard`。
- final merge 时对 `last_hud_target` 做内容字保护。
- 对每次 tail drop 写 trace reason。

验证：

```powershell
cargo test -p ainput-desktop streaming_state
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-raw-corpus.ps1 -Version <new-preview> -ShortCount 1 -LongCount 1
```

完成判定：
- raw corpus 不再出现 `raw_final_tail_dropped`。
- `了/啊/呢/吧/呀/吗/诶` 类尾字不被 final 静默删除。
- final 和 HUD final flush 一致。

## Phase 4: 异步标点 worker

目标：
- 标点在说话过程中出现，而不是只在松手后出现。
- 标点不靠停顿硬插，不重复。

修改：
- 增加常驻 punctuation worker。
- request/response 带 revision。
- 过期标点结果丢弃。
- pause event 只能触发 tail flush，不能单独触发句末标点。
- 清理重复/冲突标点。

验证：

```powershell
cargo test -p ainput-rewrite
cargo test -p ainput-desktop streaming
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-raw-corpus.ps1 -Version <new-preview> -ShortCount 1 -LongCount 1
```

完成判定：
- 长句 final 前至少出现一次合理标点。
- 不出现 `，，`、`。。`、`，。`、`？！` 等。
- 不再因为短暂停顿出现“让。不？”这类硬截断。

## Phase 5: HUD 胶囊显示和稳定性

目标：
- 彻底替换丑白面板和弹字抖动。
- HUD 始终单行、居中、随字数向两边延展。

修改：
- `apps/ainput-desktop/src/overlay.rs`
- `config/hud-overlay.toml`
- HUD trace / live E2E 判定

要求：
- 黑色半透明胶囊。
- 每个字背后都有面板。
- 短文本不出现大白面板。
- 不换行。
- active 期间中心点稳定。
- final flush 后再隐藏。

验证：

```powershell
cargo test -p ainput-desktop hud_microstream
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Version <new-preview> -Synthetic
```

完成判定：
- `white_panel_sample_count == 0`。
- `multiline_panel_sample_count == 0`。
- `alpha_drop_count == 0`。
- `max_center_x_delta_px <= 3`。
- 新 case 无上一句残留。

## Phase 6: 松手收尾和 exactly-once commit

目标：
- 松手后不截断尾音。
- 不粘旧剪贴板。
- 不重复上屏。
- 解决 HUD 看到的文字和最终上屏文字不一致的问题。

修改：
- release 后进入统一 `Finalizing` 状态。
- drain tail audio -> final decode -> final merge -> create commit envelope -> HUD flush -> commit。
- 新增 `StreamingCommitEnvelope`，字段至少包含：
  - `session_id`
  - `revision`
  - `last_hud_target_text`
  - `final_online_raw_text`
  - `offline_final_raw_text`
  - `final_candidate_text`
  - `resolved_commit_text`
  - `commit_source`
- `resolved_commit_text` 是唯一上屏文本。
- HUD final flush、clipboard write、output commit、target readback 都必须引用同一个 envelope。
- HUD final flush 后进入 `CommitLocked`，迟到的 ASR、punctuation、offline final result 一律丢弃并写 trace。
- commit request 加 session id / revision / exactly-once guard。
- commit 前后保留旧剪贴板哨兵测试，但不改输出主链路。

验证：

```powershell
cargo test -p ainput-output
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Version <new-preview> -Synthetic
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-raw-corpus.ps1 -Version <new-preview> -ShortCount 1 -LongCount 1
```

完成判定：
- `output_commit_request_count == 1`。
- `hud_final_flush_text == resolved_commit_text`。
- `clipboard_write_text == resolved_commit_text`。
- `output_commit_request_text == resolved_commit_text`。
- `target_readback == final_text`。
- `post_hud_flush_mutation_count == 0`。
- 不出现旧剪贴板内容。
- release-to-commit `<= 1200ms`。

## Phase 7: 固定等待和 CPU 线程提速

目标：
- 先量化，再减少固定等待和串行阻塞。
- GPU 暂不处理，本轮只做 CPU/等待/异步化提速。
- 明确到底慢在 release drain、offline final、punctuation、HUD 追赶、还是输出。

修改：
- 增加性能 trace：
  - `hotkey_pressed_at`
  - `mic_started_at`
  - `first_audio_at`
  - `first_decode_step_at`
  - `first_partial_at`
  - `first_hud_target_at`
  - `punctuation_elapsed_ms`
  - `release_tail_elapsed_ms`
  - `offline_final_elapsed_ms`
  - `commit_envelope_created_at`
  - `hud_final_flush_at`
  - `output_elapsed_ms`
  - `target_readback_elapsed_ms`
- release drain 改成自适应：
  - 最小等待 `120-180ms`
  - 静音稳定 `120-180ms`
  - 常规上限 `350-500ms`
  - 硬上限 `650ms`
- final repair 超时 fallback，不阻塞上屏。
- punctuation 异步化后不得阻塞 HUD。
- CPU 线程拆分：
  - streaming ASR：测 `4/6/8`
  - offline final：测 `4/8/12`
  - punctuation：测 `1/2`
- HUD microstream 长文本动态追赶。

验证：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-performance-probe.ps1 -Version <new-preview>
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-raw-corpus.ps1 -Version <new-preview> -ShortCount 1 -LongCount 1
```

完成判定：
- `release_tail_elapsed_ms <= 650ms`，目标 `<= 500ms`。
- `offline_final_elapsed_ms <= 350ms`，目标 `<= 180ms`，超时必须 fallback。
- `punctuation_elapsed_ms <= 220ms`，目标 `<= 120ms`，超时不阻塞 HUD。
- `release_to_commit_ms <= 1200ms`，目标 `<= 900ms`。
- 性能报告必须拆分慢点来源。

## Phase 8: 打包、打开新版本、回写结果

目标：
- 生成新 preview，保留 `preview.31`。
- 通过 dist 级验收。
- 打开最新版给用户试。

执行：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-startup-idle-acceptance.ps1 -Version <new-preview>
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-live-e2e.ps1 -Version <new-preview> -Synthetic
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-raw-corpus.ps1 -Version <new-preview> -ShortCount 1 -LongCount 1
```

打开规则：
- 新版本全部门禁通过后，停止旧 `ainput-desktop.exe`。
- 启动 `dist\<new-preview>\ainput-desktop.exe` 到用户 Windows 交互桌面。
- 报告实际 exe 路径和 PID。

完成判定：
- 新版本目录和 zip 存在。
- `preview.31` 未被修改。
- 新版已启动。
- `TASKLIST.md` 回写通过/未通过项。

## 停止条件

出现以下情况时停止继续扩大修改：
- 连续 2 轮修复没有改善 raw corpus 或 HUD 门禁。
- 5 轮内仍无法同时满足尾字、标点、HUD、exactly-once。
- 性能 probe 显示慢点主要来自本轮禁止修改的输出主链路。
- 任何改动需要修改非流式 `Alt+Z` 或输出主链路。

停止后必须输出：
- 当前通过项。
- 当前阻塞项。
- 最小下一步。
- 保持 `preview.31` 可回滚。
