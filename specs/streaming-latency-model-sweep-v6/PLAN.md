# Streaming Latency Model Sweep V6 PLAN

更新时间：2026-04-30

## Phase 1: 观测字段

- 在 `StreamingReplayReport` 里暴露已有 final / punctuation 计时。
- 增加 replay processing wall time 和 realtime factor。
- 不改变生产流式行为。

验证：

```powershell
cargo check -p ainput-desktop
```

## Phase 2: 独立 benchmark 脚本

- 新增 `scripts\run-streaming-latency-benchmark.ps1`。
- 使用 `AINPUT_ROOT` 指向临时 benchmark root。
- 临时 root 只复制 config，并用 junction 指向 repo `models`。
- 每个 variant 只 patch 临时 config。
- 输出 CSV / JSON / Markdown 汇总。

验证：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-latency-benchmark.ps1 -Repeats 1 -CaseIds sentence_01 -ModelIds paraformer_bilingual
```

## Phase 3: 正式测速

- 先 build release，避免 debug exe 扭曲 wall time。
- 默认矩阵跑 current bilingual paraformer 的 CPU/thread/chunk sweep。
- 跑 small bilingual zipformer baseline。
- zh-only 只在需要参考时追加。

验证：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-latency-benchmark.ps1 -BuildRelease
```

## Phase 4: 结论归因

- 读取 `summary_by_variant.csv` 和 `cases.csv`。
- 对比 first partial、processing RTF、offline final、punctuation。
- 只在内容无明显退化时考虑推荐参数或模型。

## Phase 5: 后续动作

- 如果只需要参数调整：更新 spec 后再生成新 preview。
- 如果需要模型切换：先补模型内容回归和中英混合样本，再生成新 preview。
- 本轮测速本身不直接替换用户正在运行的版本。
