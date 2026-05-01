# Streaming Full Bug Audit V13 SPEC

更新时间：2026-05-01

## 目标

在 `1.0.0-preview.45` 基础上，做一次 AI 语义改写以外的流式输入全量 bug 检查，并把“不跟手”的主观体验拆成可测量指标。

## 范围

- 只检查和修复流式输出基础链路。
- 非流式 `Alt+Z` 不改。
- 流式仍是按住 `Ctrl` 录音、松开上屏；`Ctrl` 必须只监听不拦截。
- 保持剪贴板 + `Ctrl+V` 主链路，不重写上屏机制。
- 不接 AI 语义改写。
- GPU 不纳入本轮启用范围。
- 如需修复，必须打新 preview，不覆盖 `preview.45`。

## 检查面

1. 运行态与打包完整性
   - 只允许当前 preview 作为可测主版本。
   - 包内必须包含 exe、zip、config、models、scripts、fixtures。

2. 热键与系统快捷键
   - 启动空闲不能自触发录音。
   - `Ctrl+A/C/V` 不能被流式单独 `Ctrl` 热键破坏。
   - injected key event 不能误触发录音。

3. 识别与文本质量
   - v12 用户真实样本必须继续 4/4 pass。
   - raw corpus 覆盖短句和长句，不能漏尾字、重复上屏、ghost `I/yeah`。
   - 标点不能重复，不能把固定词组拆开。

4. HUD 与上屏一致性
   - HUD final display == output commit == target readback。
   - HUD 不闪、不抖、不残留上一句。
   - 上屏必须 exactly once。

5. 不跟手测速
   - 首个 partial 到达时间。
   - partial 数量与更新时间。
   - final decode / offline repair / punctuation 时间。
   - 处理实时倍率。

## 分级

- P0：破坏 `Ctrl`、误触发录音、贴旧剪贴板、重复上屏、崩溃、包不可用。
- P1：HUD/上屏不一致、明显漏尾字、ghost `I/yeah`、焦点错位、v12 回归失败。
- P2：不跟手、HUD 轻微抖动、长句延迟高、标点体验差但不破坏可用性。
- P3：文档、日志、打包说明、低风险边缘配置。

## 完成标准

- 新增总控脚本 `scripts\run-streaming-full-audit.ps1`。
- 生成 `tmp\streaming-full-audit\<timestamp>\full-audit-report.json` 与 `SUMMARY.md`。
- `RESULTS.md` 写明所有发现、证据、修复状态。
- 若存在 P0/P1，必须修复并重新跑相关门禁。
- 若存在明确可修的 P2 且不影响用户约束，应本轮修复；否则写入下一轮优化项。
- 最终打新 preview、启动最新版、README 回写、提交推送。
