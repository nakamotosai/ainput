# Streaming Latency Model Sweep V6 SPEC

更新时间：2026-04-30

## 目标

本轮只做流式输出测速和数据归因，不改变用户正在使用的 `preview.35`，不把主观体感当结论。

要回答的问题：

- 当前慢在 streaming ASR 首字、chunk 参数、CPU 线程、offline final、punctuation，还是固定等待以外的 wall time。
- 当前默认中英双语 paraformer 是否仍是最优默认。
- 小型中英双语 zipformer 是否有足够速度收益，且不明显牺牲内容稳定性。
- 中文单语 zipformer 只作为参考，不作为默认候选。

## 硬边界

- 只测流式输出。
- 非流式 `Alt+Z` 不改、不测成默认变量。
- 流式热键仍是按住 `Ctrl` 录音、松开上屏。
- `clipboard + Ctrl+V` 输出主链路不改。
- AI rewrite 不接入，`voice.streaming.ai_rewrite.enabled = false`。
- GPU 本轮不启用，不改 provider 和依赖。
- 不停止、不替换用户当前运行的 `dist\ainput-1.0.0-preview.35\ainput-desktop.exe`。
- 如果最后需要改默认模型或参数，必须另起新 preview，不覆盖旧 dist。

## 测试变量

默认测速矩阵：

- current bilingual paraformer:
  - `asr_num_threads = 2 / 4 / 6 / 8`
  - `chunk_ms = 40 / 60 / 80`
- small bilingual zipformer:
  - `asr_num_threads = 6`
  - `chunk_ms = 60`
- zh-only zipformer:
  - 仅在显式 `-IncludeZhReference` 时加入
  - 只能用于参考速度，不进入默认候选

固定变量：

- `final_num_threads = 8`
- `punctuation_num_threads = 1`
- `gpu_enabled = false`
- `AI rewrite = false`

## 测试样本

默认覆盖：

- fixtures:
  - `sentence_01`
  - `sentence_05`
  - `sentence_combo_long`
- raw:
  - 从 `preview.35` 的 `logs\streaming-raw-captures` 里选 1 条短样本和 1 条长样本
  - 默认只选 `>= 200KB` 的 raw，避免把 0.6s 左右的空白/噪声短录音当成语音门禁

每个 variant 默认 repeat 2 次，用于减少冷启动和偶发噪声。

## 指标

每个 case 必须记录：

- `first_partial_ms`
- `partial_updates`
- `total_decode_steps`
- `input_duration_ms`
- `processing_wall_elapsed_ms`
- `processing_realtime_factor`
- `final_decode_elapsed_ms`
- `online_final_elapsed_ms`
- `offline_final_elapsed_ms`
- `offline_final_timed_out`
- `punctuation_elapsed_ms`
- `commit_source`
- `behavior_status`
- `content_status`
- `final_text`

汇总时按 variant 输出：

- first partial 平均值和 p50
- processing realtime factor 平均值
- offline final 平均耗时
- punctuation 平均耗时
- partial update 平均次数
- 内容/行为失败数

## 验收

本轮完成不以“跑完命令”为准，必须产出：

- `tmp\streaming-latency-benchmark\<stamp>\cases.csv`
- `tmp\streaming-latency-benchmark\<stamp>\summary_by_variant.csv`
- `tmp\streaming-latency-benchmark\<stamp>\summary.json`
- `tmp\streaming-latency-benchmark\<stamp>\SUMMARY.md`

结论必须说明：

- 第一瓶颈是哪一段。
- 哪个变量有明确收益，哪个变量没有收益。
- 是否值得切换模型或参数。
- 如果要切换，下一步必须生成新 preview 并走既有 raw/live/startup 门禁。

## 失败判定

- benchmark 脚本改动了当前运行的 preview，失败。
- 测试过程修改了用户正式 `config\ainput.toml` 默认值，失败。
- 把中文单语模型推荐为默认，失败。
- 只给平均 wall time、不拆 first partial / final / punctuation，失败。
- 内容明显退化但仍推荐为默认，失败。
