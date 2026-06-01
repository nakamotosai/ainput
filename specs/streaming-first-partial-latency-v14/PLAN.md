# Streaming First Partial Latency V14 PLAN

更新时间：2026-05-01

## Phase 1: 基线调查

- 读取 v13 latency cases.csv。
- 跑 `zipformer_small_bilingual` 对照测试。
- 判断是否有可直接切换的小模型。

## Phase 2: 指标修正

- 在 replay report 中估算语音起点。
- 输出 `speech_start -> first_partial` 指标。
- latency benchmark 汇总新指标。
- full audit P2 判定改用新指标。

## Phase 3: 回归

- 跑 hotkey/streaming/rewrite 相关测试。
- 跑 latency benchmark。
- 跑 full audit。

## Phase 4: 发布

- 打新 preview。
- 启动最新版。
- README / RESULTS / TASKLIST 回写。
- closeout guard、postflight、memory trigger、commit / push、clean tree。
