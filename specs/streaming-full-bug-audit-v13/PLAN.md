# Streaming Full Bug Audit V13 PLAN

更新时间：2026-05-01

## Phase 1: 建立审计门禁

- 新增 `scripts\run-streaming-full-audit.ps1`。
- 新增 spec pack：`SPEC.md`、`PLAN.md`、`TASKLIST.md`、`RESULTS.md`。
- 最小验证：脚本能生成报告并不中途吞掉失败信息。

## Phase 2: 全量审计

- 运行包完整性检查。
- 运行 Rust / hotkey / streaming 单测。
- 运行 startup idle、v12 replay、selftest、raw corpus、synthetic live、wav live。
- 运行 latency benchmark，输出“不跟手”指标。
- 汇总 P0/P1/P2/P3。

## Phase 3: 修复非 AI 问题

- P0/P1 必须修。
- P2 若属于配置或小范围实现问题，本轮修；若需要换模型/GPU/AI rewrite，则只记录，不混进本轮。
- 每个修复必须映射回 `RESULTS.md` 的发现项。

## Phase 4: 打包与回归

- 打新 preview，不覆盖 `preview.45`。
- 用新 preview 复跑相关失败门禁和核心旧门禁。
- 启动最新版到 Windows 交互桌面。

## Phase 5: 收口

- README 写入当前版本、验证、handoff。
- `RESULTS.md` 写入最终状态。
- 运行 readme closeout guard、postflight、memory writeback trigger。
- commit / push / clean tree。
