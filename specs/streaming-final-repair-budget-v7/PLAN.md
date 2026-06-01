# Streaming Final Repair Budget V7 PLAN

更新时间：2026-04-30

## Phase 1: 实现预算分流

- 新增完整 offline final 最大音频长度。
- 新增长句 tail window 长度。
- `transcribe_streaming_offline_final` 根据音频长度选择 full / tail。

## Phase 2: 安全合并

- `select_streaming_final_raw_text` 支持 tail window 结果。
- tail result 只能在与 streaming final 尾部有重叠时合并。
- 无重叠则 fallback 到 streaming final / HUD。

## Phase 3: 验证

```powershell
cargo fmt
cargo check -p ainput-desktop
cargo test -p ainput-desktop offline_final -- --nocapture
cargo test -p ainput-desktop streaming -- --nocapture
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-selftest.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-latency-benchmark.ps1 -Repeats 1 -CaseIds sentence_01,sentence_05,sentence_combo_long -ModelIds paraformer_bilingual -AsrThreads 6 -ChunkMs 60 -IncludeRaw:$false
```

## Phase 4: 打包和启动

- 验证通过后生成新 preview。
- 停止旧 `ainput-desktop.exe`。
- 启动最新 dist exe 到 Windows 交互桌面。
