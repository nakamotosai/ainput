# Streaming Latency Model Sweep V6 RESULTS

更新时间：2026-04-30

## 有效报告

正式有效报告：

```text
C:\Users\sai\ainput\tmp\streaming-latency-benchmark\20260501-012054-119
```

产物：

- `cases.csv`
- `summary_by_variant.csv`
- `summary.json`
- `SUMMARY.md`

说明：

- 第一轮报告 `20260501-011259-369` 暴露了一个无效 raw：`streaming-raw-1777565436207` 只有约 `640ms`，无 partial，像空白/噪声，不适合测速门禁。
- 脚本已改为默认只选 `>= 200KB` 的 raw，再跑第二轮作为有效结论。

## 有效矩阵结果

| variant | failed | first avg | first p50 | first p95 | proc rtf | proc avg | proc p95 | offline avg | punct avg |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| paraformer_bilingual_asr6_chunk80 | 0/8 | 940 | 640 | 1840 | 0.203 | 2141.8 | 5120 | 465.6 | 4 |
| paraformer_bilingual_asr6_chunk40 | 0/8 | 960 | 660 | 1860 | 0.216 | 2342.9 | 5650 | 477.5 | 4.5 |
| paraformer_bilingual_asr8_chunk60 | 0/8 | 960 | 660 | 1860 | 0.217 | 2349.6 | 5732 | 470.9 | 4.2 |
| paraformer_bilingual_asr2_chunk60 | 0/8 | 960 | 660 | 1860 | 0.218 | 2347 | 5688 | 455.8 | 4.8 |
| paraformer_bilingual_asr4_chunk60 | 0/8 | 960 | 660 | 1860 | 0.220 | 2369.6 | 5602 | 466 | 4.2 |
| paraformer_bilingual_asr6_chunk60 | 0/8 | 960 | 660 | 1860 | 0.220 | 2360.8 | 5635 | 473.4 | 4.2 |
| zipformer_small_bilingual_asr6_chunk60 | 8/8 | 880 | 780 | 1080 | 0.138 | 1356.2 | 2925 | 461 | 0 |

## 归因

结论：

- 不建议换到 `streaming-zipformer-small-bilingual-zh-en`。它处理更快，但 8/8 行为或内容失败，长句和中句识别明显退化。
- 不建议为了速度加 ASR 线程。`asr_num_threads = 2/4/6/8` 的 first partial 基本一样，processing RTF 差距很小，8 线程没有收益。
- `chunk_ms = 80` 在本轮 replay 里略好于 `60/40`，但 first partial 只改善约 `20ms`，收益很小，不足以单独解释用户体感延迟。
- 标点不是瓶颈。`punctuation_elapsed_ms` 基本是 `0-7ms`。
- 长句 release 后的 `offline_final_elapsed_ms` 是明确瓶颈。长句样本 offline final 到 `~1.0s`，会直接拖慢松手后最终上屏。
- raw 真实短样本 first partial 到 `~1.84s`，而 fixture 为 `~640-660ms`，说明首字慢更受真实语音起音、音量、静音段、VAD/partial 触发时机影响，不是 CPU 线程或 GPU 问题。

## 建议

下一轮提速优先级：

1. 不换模型，继续默认 `sherpa-onnx-streaming-paraformer-bilingual-zh-en`。
2. 不上 GPU，当前证据不支持 GPU 是第一瓶颈。
3. 优先修 final repair 策略：
   - offline final 不应在 release 后无预算地跑完整长音频。
   - 超过预算时应快速使用 streaming final / HUD commit fallback。
   - 长句可只对尾部窗口做 final repair，或把 offline final 变成异步验证，不阻塞上屏。
4. 首字慢另开小实验：
   - 排查真实 raw 开头静音和音量。
   - 测有效 standby preroll / first speech detection。
   - 不优先调大线程。
5. `chunk_ms = 80` 可作为候选优化，但必须再跑真实前台 HUD 感知测试；目前不建议直接改默认。
