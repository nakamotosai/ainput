# Streaming Output Foundation V4 TASKLIST

更新时间：2026-04-30

## 状态

- [x] v4 范围已收窄为只修流式输出基础体验。
- [x] 明确非流式 `Alt+Z` 不进入本轮修改。
- [x] 明确流式 `Ctrl` 规则不改，只防回归。
- [x] 明确 clipboard + Ctrl+V 输出主链路不改。
- [x] 明确 AI rewrite 不进入本轮。
- [x] 明确 `preview.31` 是回滚基线，新实现必须打新 preview。
- [x] 明确 GPU 暂不进入本轮。
- [x] 明确 HUD 最终显示和最终上屏必须同源。
- [x] 明确提速优先处理固定等待和 CPU 线程，不先做 GPU。

## Phase 0: 基线

- [x] 确认 `dist\ainput-1.0.0-preview.31\ainput-desktop.exe` 存在。
- [x] 确认 `dist\ainput-1.0.0-preview.31.zip` 存在。
- [x] 记录当前运行 exe 和 PID。
- [x] 后续打包版本号必须大于 `preview.31`。

## Phase 1: 热键边界

- [x] 日志区分 `fast_hotkey=Alt+Z` 和 `streaming_effective_hotkey=Ctrl`。
- [ ] `Ctrl+A/C/V` 门禁通过。
- [ ] injected event 不触发录音。
- [x] startup idle 不自动录音。

## Phase 2: 音频预滚

- [ ] standby ring buffer 只缓存短音频，不 ASR、不上屏。
- [ ] `Ctrl` press 后 preroll 真正进入本轮 session。
- [ ] first partial 目标 `<= 900ms`。
- [ ] idle 期间不产生 raw capture。

## Phase 3: 尾字保护

- [x] final merge 不少于最后 HUD 内容字。
- [x] `了/啊/呢/吧/呀/吗/诶` 尾字保护通过。
- [ ] tail drop 必须有 trace reason。
- [x] raw corpus 不再报 `raw_final_tail_dropped`。

## Phase 4: 异步标点

- [x] punctuation worker 常驻或可快速复用。
- [ ] punctuation request/response 带 revision。
- [ ] 过期标点结果丢弃。
- [x] 停顿不单独产生句末标点。
- [x] 重复/冲突标点清理通过。
- [x] 长句 final 前至少出现一次合理标点。

## Phase 5: HUD

- [x] 黑色半透明胶囊。
- [x] 单行居中，不换行。
- [x] 短文本无白色大面板。
- [x] active 期间不闪烁、不隐藏。
- [x] 中心点/top 稳定。
- [x] 新一句开始前无上一句残留。
- [x] final flush 后再隐藏。

## Phase 6: 松手收尾和上屏

- [x] release 后先 drain tail audio。
- [ ] final decode 完成或超时 fallback 后才 commit。
- [x] 创建 `StreamingCommitEnvelope` 作为本轮唯一 commit 文本来源。
- [x] HUD final flush 在 commit 前发生。
- [x] HUD final flush 文本等于 `resolved_commit_text`。
- [x] clipboard 写入文本等于 `resolved_commit_text`。
- [x] output commit request 文本等于 `resolved_commit_text`。
- [x] HUD final flush 后进入 `CommitLocked`。
- [x] 迟到 ASR/标点/final repair 不能再改本轮文本。
- [x] 一轮 release 只有一次 commit。
- [x] 不粘旧剪贴板内容。
- [x] `target_readback == final_text`。
- [x] `post_hud_flush_mutation_count == 0`。

## Phase 7: 性能

- [ ] 线程数 probe 完成。
- [x] GPU 不进入本轮实现和验收。
- [x] release drain 自适应完成。
- [ ] `release_tail_elapsed_ms <= 650ms`。
- [x] offline final 单独计时。
- [ ] offline final 超时 fallback。
- [ ] `offline_final_elapsed_ms <= 350ms`。
- [x] punctuation 单独计时。
- [ ] `punctuation_elapsed_ms <= 220ms`。
- [x] HUD 长文本动态追赶。
- [x] 性能报告能拆出慢点来源。
- [ ] punctuation 不阻塞 HUD。
- [x] raw capture 不阻塞 commit。

## Phase 8: 打包和启动

- [x] 生成新 preview 目录。
- [x] 生成新 preview zip。
- [x] `preview.31` 未被修改。
- [x] dist 级 startup idle 通过。
- [x] dist 级 live E2E synthetic 通过。
- [x] dist 级 raw corpus 短句/长句抽样通过。
- [x] 新版在用户 Windows 交互桌面启动。
- [x] 报告新版 exe 路径和 PID。
