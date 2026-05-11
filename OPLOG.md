# ainput OPLOG

## 2026-05-11 打包 1.0.0-preview.78：中文 CTC 在线默认、release 快速响应、数字误改收紧

- 目标：在保留 `极速 / 本地流式 / 在线流式` 三个独立模式的前提下，把在线默认从失败的多语言 RNNT 拉回中文专用 CTC，并修掉松手上屏慢和中文“一”被乱改成 `1` 的问题。
- 修复：vps-jp live adapter 与 Windows 包内 sidecar 默认模型回到 `nvidia/parakeet-ctc-0_6b-zh-cn`、function id `9add5ef7-322e-47e0-ad7a-5653fb8d259b`、`language=zh-CN`。
- 修复：在线 worker 在活跃录音 tick 内最多发送 1 个远端 `/chunk`，避免积压 chunk 的同步 HTTP 调用阻塞 `CtrlUp` 释放命令；释放路径仍完整 drain。
- 修复：vps-jp adapter `PARTIAL_WAIT_SEC` 默认从 `0.18s` 收紧到 `0.06s`，减少没有新 partial 时的等待堆积。
- 修复：speech context boost 只使用 vps-jp `~/.codex/sessions` 用户指令历史统计出的高频英文词，未加入猜测词。
- 修复：中文数字归一化改为保守策略，只处理纯中文数字连读；`一起 / 一边 / 一下 / 一个` 等自然中文词不再被强行改成 `1起 / 1边 / 1下 / 1个`。
- 保持：本地 Qwen/Sherpa 模式不变；`preview.77` 保留为失败实验证据；不修改或重启 `cliproxyapi` 8317；不把 NVIDIA key 写入 repo、dist 或日志。

验证：

- `cargo fmt --all -- --check` 已通过。
- `cargo check -p ainput-desktop` 已通过；只剩既有 dead-code warning。
- `cargo test -p ainput-rewrite` 已通过，18/18 passed，覆盖 `等会儿1起修 -> 等会儿一起修` 与纯中文数字连读转换。
- `cargo test -p ainput-shell` 已通过，6/6 passed。
- Windows 包内 sidecar `python -m py_compile` 已通过；vps-jp live adapter `python3 -m py_compile` 已通过。
- vps-jp `ainput-parakeet-asr.service` 是 user service，已重启并 active；`/health` 返回 `model=nvidia/parakeet-ctc-0_6b-zh-cn`、`language=zh-CN`、`partial_wait_sec=0.06`、`boost_phrases=29`、`key_count=5`。
- Windows 访问 `http://vps-jp.tail4b5213.ts.net:18765/health` 返回同样配置。
- `scripts\package-release.ps1 -Version 1.0.0-preview.78` 已通过，产出 `dist\ainput-1.0.0-preview.78\` 与 `dist\ainput-1.0.0-preview.78.zip`。
- `run-ainput.bat` 与 HKCU Run 自启动均指向 `dist\ainput-1.0.0-preview.78\ainput-desktop.exe`。
- Windows 交互桌面已运行 `dist\ainput-1.0.0-preview.78\ainput-desktop.exe`，`SessionId=1`。
- preview78 启动日志确认 `voice_mode=OnlineStreaming`、backend 为 `NVIDIA Parakeet online ASR`、模型为中文 CTC，并出现 `local model preload skipped`。

## 2026-05-11 hot rollback：preview.77 多语言 RNNT 中文失败，live 回滚 preview.76

- 现象：`nvidia/parakeet-1_1b-rnnt-multilingual-asr` 配合 `language=multi` 在中文实时输入时会不断把 HUD partial 猜成多种语言，最终中文结果严重错误。
- 判断：该模型的自动语言识别不适合作为 AInput 默认实时 ASR，尤其不能替代中文专用 `nvidia/parakeet-ctc-0_6b-zh-cn`。
- 止血：vps-jp `ainput-parakeet-asr.service` 已回滚到 `nvidia/parakeet-ctc-0_6b-zh-cn`、function id `9add5ef7-322e-47e0-ad7a-5653fb8d259b`、`language=zh-CN`。
- 止血：Windows 当前进程、`run-ainput.bat`、HKCU Run 均回滚到 `dist\ainput-1.0.0-preview.76\ainput-desktop.exe`。
- 后续：新版本应把中文在线 CTC 设为默认；多语言 RNNT 只能作为日文/英文实验模式或显式语言模式，不再做默认 auto 模式。

验证：

- Windows live process: `C:\Users\sai\ainput\dist\ainput-1.0.0-preview.76\ainput-desktop.exe`，`SessionId=1`。
- Windows `/health`: `model=nvidia/parakeet-ctc-0_6b-zh-cn`、`language=zh-CN`、`streaming_partials=true`。
- preview76 启动日志确认 `voice_mode=OnlineStreaming`、`model=nvidia/parakeet-ctc-0_6b-zh-cn`、`local model preload skipped`。

## 2026-05-11 打包 1.0.0-preview.77：在线 Parakeet 多语言 RNNT

- 目标：把第三个独立在线流式模式从中文专用 `nvidia/parakeet-ctc-0_6b-zh-cn` 替换为多语言 `nvidia/parakeet-1_1b-rnnt-multilingual-asr`，优先覆盖用户的日文、中文、英文使用场景。
- 修复：vps-jp live adapter 与 Windows 包内 sidecar 默认模型改为 `nvidia/parakeet-1_1b-rnnt-multilingual-asr`。
- 修复：NVIDIA function id 改为 `71203149-d3b7-4460-8231-1be2543a1fca`，`language_code` 改为 `multi`。
- 保持：本地 Qwen/Sherpa 模式不变；`preview.76` 仍是中文 CTC 在线模式回滚包；不修改或重启 `cliproxyapi` 8317；不把 NVIDIA key 写入 repo、dist 或日志。

验证：

- vps-jp `ainput-parakeet-asr.service` 已重启并 active，`/health` 返回新模型、新 function id、`language=multi`、`key_count=5`、`streaming_partials=true`。
- Windows 访问 `http://vps-jp.tail4b5213.ts.net:18765/health` 已返回同样的新模型配置。
- `python3 -m py_compile` 已通过 vps-jp live adapter；Windows 包内 sidecar `python -m py_compile` 已通过。
- `cargo fmt --all -- --check` 已通过。
- `cargo check -p ainput-desktop` 已通过；只剩既有 unused / dead-code warning。
- `cargo test -p ainput-shell` 已通过，6/6 passed。
- `scripts\package-release.ps1 -Version 1.0.0-preview.77` 已通过，产出 `dist\ainput-1.0.0-preview.77\` 与 `dist\ainput-1.0.0-preview.77.zip`。

## 2026-05-11 打包 1.0.0-preview.76：独立在线流式模式

- 目标：把在线 NVIDIA Parakeet ASR 从本地 `streaming` 模式拆出来，成为第三个独立模式。
- 修复：新增 `online_streaming` 模式、`[voice.online_streaming]` 配置和托盘一级菜单项。
- 修复：本地 `streaming` 默认恢复到 Qwen sidecar，不再指向远端 Parakeet adapter。
- 修复：在线 worker 独立上报 `WorkerKind::OnlineStreaming`，不污染本地 Qwen lifecycle。
- 修复：在线松手时 HUD 有文本就直接粘贴 HUD snapshot；远端 `/finish`、session cleanup 和 raw capture 保存后台执行。

验证：

- `cargo fmt --all -- --check` 已通过。
- `cargo check -p ainput-desktop` 已通过。
- Windows `/health` 可访问 `http://vps-jp.tail4b5213.ts.net:18765/health`，adapter 仍为 `nvidia/parakeet-ctc-0_6b-zh-cn` 且 `streaming_partials=true`。
- `scripts\package-release.ps1 -Version 1.0.0-preview.76` 已通过，产出 `dist\ainput-1.0.0-preview.76\` 与 `dist\ainput-1.0.0-preview.76.zip`。
- Windows 交互桌面已运行 `dist\ainput-1.0.0-preview.76\ainput-desktop.exe`，`SessionId=1`。

## 2026-05-11 打包 1.0.0-preview.75：在线 Parakeet HUD 实时 partial

- 根因：`preview.74` 的 `/chunk` endpoint 每次只缓存音频并返回空文本，所有识别都在 `/finish` 才发生；因此 HUD 按住期间为空，松手后才一次性弹出。
- 修复：`nvidia_parakeet_online_sidecar.py` 改成 session 创建时启动 NVIDIA streaming gRPC 后台线程，`/chunk` 把 PCM16 音频送入队列，并返回最新 partial。
- 修复：开启 `interim_results=True`，维护 final segments + interim text，避免最终结果只靠松手后整段识别。
- 保持：不修改 `cliproxyapi` 8317，不把 NVIDIA key 写入 Windows 包或日志；AInput 现有 sidecar HTTP contract 不变。

验证：

- vps-jp adapter 已重启并 active。
- Windows `/health` 返回 `streaming_partials=true`。
- 已知 WAV 按 240ms chunk、120ms 间隔模拟实时输入，`/finish` 前收到 32 次非空 partial；首个 partial 约 466ms 出现。
- 最终文本仍为：`我现在的问题是所有各个口袋词发过来的消息全部留在1个框里， 所以我翻早起来特别痛苦。`
- `scripts\package-release.ps1 -Version 1.0.0-preview.75` 已通过，产出 `dist\ainput-1.0.0-preview.75\` 与 `dist\ainput-1.0.0-preview.75.zip`。
- Windows 交互桌面已运行 `dist\ainput-1.0.0-preview.75\ainput-desktop.exe`，`SessionId=1`。
- 启动日志确认 backend 为 `NVIDIA Parakeet online ASR`，且出现 `local model preload skipped`。
- vps-jp adapter 最终重启清理测试 session 后，`/health` 返回 `sessions=0` 与 `streaming_partials=true`。

## 2026-05-11 打包 1.0.0-preview.74：临时在线 NVIDIA Parakeet ASR

- 背景：本机 Qwen3-ASR 0.6B 虽然模型权重约 1.88GB，但 vLLM / CUDA 运行态会把 Windows GPU 显存顶到约 5GB 以上；用户本轮要求新增在线 ASR 模式并默认绕开本地模型加载。
- 接口事实：NVIDIA Parakeet CTC zh-CN 是 Riva gRPC/NVCF 形态，不是 OpenAI-compatible `/v1/audio/transcriptions`；因此不能只把 AInput 指向 `cliproxyapi` 8317 的 OpenAI HTTP endpoint。
- 方案：新增 `nvidia_parakeet_online` backend；AInput 继续复用现有 sidecar session HTTP contract，`vps-jp` 临时 adapter 负责读取 8317 生产配置中的 5 个 NVIDIA keys 并轮询调用 Parakeet。
- 安全边界：不修改或重启 `cliproxyapi` 8317；不把 NVIDIA key 写入 Windows TOML、dist、git 或日志。
- 默认配置：`voice.mode = "streaming"`，`voice.streaming.backend = "nvidia_parakeet_online"`，`sidecar_url = "http://vps-jp.tail4b5213.ts.net:18765"`，`sidecar_auto_start = false`，`gpu_enabled = false`。

验证：

- `cargo fmt --all -- --check` 已通过。
- `cargo check -p ainput-desktop` 已通过；仍有既有 dead-code warnings。
- `cargo test -p ainput-shell render_config_file_contains_streaming_ai_rewrite_section` 已通过。
- `vps-jp` adapter `/health` 从 Windows 可访问，返回 Parakeet 模型、16k sample rate、5 个 key。
- 已知 WAV 通过在线 adapter 实测转写，11.38s 音频耗时约 2.15s。
- `scripts\package-release.ps1 -Version 1.0.0-preview.74` 已通过，产出 `dist\ainput-1.0.0-preview.74\` 与 `dist\ainput-1.0.0-preview.74.zip`。
- Windows 交互桌面已运行 `dist\ainput-1.0.0-preview.74\ainput-desktop.exe`，`SessionId=1`。
- 启动日志确认 backend 为 `NVIDIA Parakeet online ASR`，且出现 `local model preload skipped`；未出现本地 Qwen model preload。
- 已停止 `.72` 遗留的 WSL Qwen/vLLM 进程；复查 WSL 中无 qwen/vllm sidecar 进程。
- GPU 复查约 `2856 / 11264 MiB`，未见本地 Qwen 6GB 负载。

## 2026-05-11 打包 1.0.0-preview.72：Qwen context echo guard 与项目暂时收口

- 根因确认：Qwen3-ASR sidecar 在坏音频/低信号场景下会把 `[voice.streaming.qwen3].context` 直接作为 partial/final 文本吐出；`.71` 的拦截点太晚，提示词可能先闪到 HUD 或被 fast HUD snapshot 上屏。
- 修复：新增 Qwen context echo guard，使用当前配置 context 和关键 prompt marker 检测回显；在 `apply_qwen_sidecar_partial_update` 写入 `last_display_text`、发送 HUD partial、进入 history/paste 之前直接拦截。
- 修复：release final 路径在 HUD final ack 与 paste 前二次拦截 context echo；fast HUD snapshot 也拒绝提交 prompt-like `last_display_text`。
- 保持：不改 Qwen context，不改标点策略，不恢复 offline final，不启用应用层 AI rewrite；`voice.streaming.ai_rewrite.enabled = false` 保持关闭。
- 版本入口：workspace version 升到 `1.0.0-preview.72`，`run-ainput.bat` 与 HKCU Run 自启动均指向 `dist\ainput-1.0.0-preview.72\ainput-desktop.exe`。
- Windows live 运行进程已切到 `dist\ainput-1.0.0-preview.72\ainput-desktop.exe`，PID `37176`，`SessionId=1`。

验证：

- `cargo fmt` 已执行。
- `cargo test -p ainput-desktop qwen_context_echo -- --nocapture`：3/3 passed。
- `cargo test -p ainput-desktop worker::tests:: -- --nocapture`：72/72 passed。
- `scripts\package-release.ps1 -Version 1.0.0-preview.72` 产出 `dist\ainput-1.0.0-preview.72\` 与 `dist\ainput-1.0.0-preview.72.zip`。
- 包内配置验证：`voice.streaming.ai_rewrite.enabled = false`。
- `.72` 日志确认 Qwen worker started、model ready、warm chunk completed；`/health` 返回 `ok=true`、`model=Qwen/Qwen3-ASR-0.6B`、`idle_unload_ms=3600000`、`effective_enforce_eager=false`。

## 2026-05-10 打包 1.0.0-preview.68：收紧上屏延迟并清理历史构建产物

- live 证据确认：`preview.67` 的 `hud_final_flush_elapsed_ms` 只有约 `16-18ms`，`output_elapsed_ms` 只有约 `72-98ms`；“HUD 已经完整了但贴上去还慢”的主因不在 Ctrl+V，而在松手后的 release drain + final decode。
- 修复：本地流式 `chunk_ms` 从 `240` 收紧到 `120`，让 partial cadence 和松手收尾都更勤。
- 修复：`release_drain_min_ms / release_drain_idle_settle_ms / release_drain_max_ms` 从 `120 / 120 / 300` 收紧到 `80 / 80 / 220`，减少明明说完了还在等尾巴的时间。
- 修复：`STREAMING_PASTE_STABILIZE_DELAY` 从 `35ms` 收紧到 `20ms`，把最后一小段无意义等待也削掉。
- 修复：Qwen preload 不再只做 `start_session -> finish_session`，现在会先送一段真实 warm chunk 再 finish，补齐 chunk endpoint 的冷路径预热。
- 修复：WSL auto-start 的 Qwen sidecar 环境把 `QWEN3_CHUNK_SIZE_SEC` 收紧到 `0.18`，让 sidecar 流式节奏和本地 120ms feed 更接近。
- 新增：`scripts\prune-artifacts.ps1`，用于保留当前版本和回滚版本，同时清理历史 `dist` zip / 目录、`target*` 目录和旧 installer residue。
- live 空间调查结论：
  - `dist`: `84.03GB`
  - `target`: `44.10GB`
  - `target-r109` ~ `target-r112b`: 约 `12.89GB`
  - `models`: `3.62GB`
  - `tmp`: `1.40GB`
  - `dist` 里共有 `94` 个包目录、`93` 个 zip，zip 单独就占 `35.41GB`
- Windows live 运行进程已切到 `dist\ainput-1.0.0-preview.68\ainput-desktop.exe`。
- `dist\ainput-1.0.0-preview.68\logs\ainput.log` 已确认：
  - `ainput Qwen3-ASR sidecar streaming worker loop started ... chunk_ms=120`
  - `starting async Qwen sidecar model preload`
- 已执行 `scripts\prune-artifacts.ps1` 首轮 live 清理：
  - 清掉旧 `dist` 目录、旧 zip、全部 `target*`、WiX 残留
  - 实际回收 `139.03GB`
  - 清理后顶部目录缩到：`models 3.62GB`、`dist 3.22GB`、`tmp 1.40GB`、`labs 0.62GB`
- 首轮清理后还发现两类漏网产物：
  - `dist\ainput-setup-*.exe` 约 `1.35GB`
  - `tmp\ainput-installer-*` 目录约 `1.28GB`
- 因此补强 `scripts\prune-artifacts.ps1`：默认再清旧 `ainput-setup-*.exe`，并清 `tmp\ainput-installer-*` 解包残留，避免下次又积回去。

## 2026-05-10 打包 1.0.0-preview.67：启动预加载、切模预加载与托盘版本号

- 根因确认：之前托盘 loading/ready 只挂在 `fast_worker_ready / streaming_worker_ready`，而 Qwen worker 会在线程刚起时就给 `Ready(Streaming)`，这和“模型真 ready”不是一回事。
- 修复：主线程新增语音模型生命周期 `Cold / Loading / Ready / Failed`，托盘与 HUD 的加载态改为挂钩真实模型 readiness。
- 修复：启动 `ainput` 时会立刻预加载当前选中的语音模型；当前若是流式 Qwen，就直接发起 sidecar/model preload。
- 修复：切换 `极速语音识别 / 流式语音识别` 时复用同一套预加载生命周期，不再只改菜单勾选状态。
- 修复：Qwen worker 新增 `PreloadModel` 命令；只有 sidecar/model 真 ready 后才回推 `Ready(Streaming)`。
- 修复：若热键在 preload 还没结束时就已经按下，worker 会在 preload 完成后直接衔接 session bootstrap，不再把“线程已起”误当成“模型已就绪”。
- 保持 V19 架构不变：没有恢复 offline final、没有 HUD/offline merge、没有 release hidden correction。
- 保持 Qwen 空闲自动卸载：ready 态会按 `sidecar_idle_unload_ms = 300000` 推导 idle deadline，超时后托盘返回未加载态。
- 托盘右键菜单新增当前版本号显示。

验证：

- `cargo fmt --all` 通过。
- `cargo check -p ainput-desktop` 通过。
- 已打包 `dist\ainput-1.0.0-preview.67\` 与 `dist\ainput-1.0.0-preview.67.zip`。
- 已切换 `run-ainput.bat` 到 `preview.67`。
- 已停止旧版并启动到 Windows 交互桌面，当前进程路径为 `dist\ainput-1.0.0-preview.67\ainput-desktop.exe`，PID `33716`。
- `dist\ainput-1.0.0-preview.67\logs\ainput.log` 确认启动即触发：
  - `ainput Qwen3-ASR sidecar streaming worker loop started ... chunk_ms=240`
  - `starting async Qwen sidecar model preload`
  - `Qwen3-ASR sidecar is ready model=Qwen/Qwen3-ASR-0.6B`
- SSH 会话无法通过 `CopyFromScreen` 直接抓到交互桌面 HUD / 托盘截图，本轮可视面以真实进程路径和包内启动日志替代验收。

## 2026-05-10 打包 1.0.0-preview.59：HUD 快速微流式与 Qwen3-ASR 归一化防误伤

- 根因确认：preview.58 已经让 Qwen partial 高频进入 HUD，但 `StreamingPartial` 每次直接整段替换文本，视觉上会像一块一块跳出来，而不是连续吐字。
- 修复：HUD partial 重新启用 char streaming，但改成自适应追赶；每 16ms 按剩余字符量计算步长，最多 8 字一跳，目标约 8 帧追平当前 partial，避免回到 preview.57 的滞后。
- 根因确认：`10000003ASR` 来自 `千万三ASR` / `千问三ASR` 被中文数字归一化逻辑当成数字词处理。
- 修复：先把常见 `Qwen3-ASR` 误听写法归一到 `Qwen3-ASR`，并阻止带中文单位且后接 ASCII 技术词的片段进入中文数字归一化。
- 追加修复：`千万三模型` / `一万三预算` 这类带 `万/亿` 后接裸数字尾巴的口语片段不再进入中文数字归一化，避免被算成 `10000003` / `10003`。
- 保持 V19 架构：没有 offline final、没有 HUD/offline merge、没有 release hidden correction；release 只负责 drain tail、finish sidecar、用最终 HUD 文本提交。

验证：
- `cargo fmt --check` 通过。
- `cargo test -p ainput-rewrite` 通过：17 passed。
- `cargo test -p ainput-desktop` 通过：108 passed。
- 已重打包 `dist\ainput-1.0.0-preview.59\` 与 `dist\ainput-1.0.0-preview.59.zip`。
- 已启动到 Windows 交互桌面，当前进程路径为 `dist\ainput-1.0.0-preview.59\ainput-desktop.exe`。
- 进程 FileVersion / ProductVersion 均为 `1.0.0-preview.59`。
- Qwen sidecar `/health` 返回 `ok=true`、`model=Qwen/Qwen3-ASR-0.6B`。
- preview.59 包内日志确认 Qwen sidecar worker 启动：`chunk_ms=500`。

## 2026-05-10 打包 1.0.0-preview.58：Qwen partial 绕开旧稳定策略

- 根因确认：preview.57 已经取消 HUD 逐字 microstream，但 live 日志仍然每段话只出现 1 条 `Qwen sidecar partial updated`；Qwen raw replay 证明 sidecar 每 500ms 都在返回递增文本。
- 修复：Qwen partial 不再走 `StreamingState::apply_online_partial_with_policy()`，避免旧 sherpa-oriented 稳定策略把后续递增文本过滤掉。
- 新规则：Qwen partial 只做 `normalize_streaming_preview`、标点清理、trim、去空、去重复，然后直接作为 HUD truth 上屏。
- 保持 V19 架构：没有 offline final、没有 HUD/offline merge、没有 release hidden correction；release 只负责 drain tail、finish sidecar、用最终 HUD 文本提交。

## 2026-05-10 打包 1.0.0-preview.57：HUD partial 直接上屏

- 根因确认：Qwen sidecar 早已在 1s 后开始返回 partial，之后约每 500ms 返回递增文本；慢感来自 HUD microstream 每 14ms 只推进 1 个字符。
- `StreamingPartial` 现在直接刷新 HUD 当前文本，不再走逐字追赶动画。
- Qwen partial 更新日志从 debug 提升到 info，并记录 `partial_updates`，方便之后直接判断“模型已返回 / HUD 是否更新”。
- 保持 preview.56 的 final 截断修复和 Qwen 低延迟参数。

验证：
- 用用户刚才长句 raw wav 分块复放，Qwen 第 2 个 chunk / 1000ms 音频即返回文本，后续每个 500ms chunk 都返回递增文本。
- 待打包后用真实 Ctrl 语音确认 HUD 不再等到松手才一次性追上。

## 2026-05-10 打包 1.0.0-preview.56：Qwen sidecar 低延迟参数与 final 截断修复

- 将 Qwen sidecar 的启动环境调到 `QWEN3_CHUNK_SIZE_SEC=0.5`、`QWEN3_UNFIXED_CHUNK_NUM=1`、`QWEN3_UNFIXED_TOKEN_NUM=2`。
- 同步更新 WSL sidecar 脚本默认值：`/home/sai/ainput-qwen3-asr/qwen3_asr_sidecar.py`。
- 修复 preview.55 的最终提交截断：final paste 直接来自 Qwen `finish.text` 的 normalize / cleanup 结果，不再用可能滞后的 HUD state。
- 保持 V19 约束：无 offline final、无 HUD/offline merge、无 release hidden correction，AI rewrite 仍不参与语音提交链路。
- 打包产物：`dist\ainput-1.0.0-preview.56\` 与 `dist\ainput-1.0.0-preview.56.zip`。
- 已启动到 Windows 交互桌面，当前进程路径为 `dist\ainput-1.0.0-preview.56\ainput-desktop.exe`。

验证：
- `cargo fmt --check` 通过。
- `cargo test -p ainput-desktop` 通过，106 passed。
- zip 内确认包含 `ainput-desktop.exe`。
- 运行进程 FileVersion / ProductVersion 均为 `1.0.0-preview.56`。
- Qwen sidecar `/health` 返回 `ok=true`、`model=Qwen/Qwen3-ASR-0.6B`。
- 运行日志确认 worker 为 `chunk_ms=500`，WSL 启动命令包含 `QWEN3_CHUNK_SIZE_SEC=0.5`。

## 2026-05-10 打包 1.0.0-preview.55：切到本机 GPU Qwen3-ASR sidecar

- 默认流式后端切到 `qwen3_sidecar`，通过 WSL2 使用本机 RTX 2080 Ti 跑原版 `Qwen/Qwen3-ASR-0.6B`。
- 保留 `sherpa` 作为显式配置回退：`[voice.streaming].backend = "sherpa"`，同时回退 `chunk_ms = 60`。
- WSL sidecar 环境固定在 `/home/sai/ainput-qwen3-asr`，模型和 venv 不落到 C 盘。
- 自动启动方式改为 spawn 一个前台 WSL `uvicorn qwen3_asr_sidecar:app` 子进程，避免 `nohup ... &` 在 SSH/WSL 场景下不留驻。
- 继续保持 V19 HUD truth：无 offline final、无 HUD/offline 合并、无 release hidden correction；最终上屏等于 HUD ack 文本。
- 打包产物：`dist\ainput-1.0.0-preview.55\` 与 `dist\ainput-1.0.0-preview.55.zip`。

验证：
- `cargo fmt --check` 通过。
- `cargo test -p ainput-desktop` 通过，106 passed。
- Qwen sidecar `/health` 返回 `ok=true`、`model=Qwen/Qwen3-ASR-0.6B`。
- 5 条近期失败 wav 的 HTTP sidecar 回归通过，结果写入 `tmp/qwen3-asr-0.6b-sidecar-http-eval.json`。
- 已停止 `preview.54`，并启动 `dist\ainput-1.0.0-preview.55\ainput-desktop.exe`，PID `95688`。

## 2026-04-16 打包 1.0.13 便携版

- 将 workspace 版本从 `1.0.12` 提升到 `1.0.13`
- 同步 `README.md` 当前正式版本、便携版目录路径和按键精灵回放说明
- 基于当前修复后的代码重新构建 release
- 产出 `dist\ainput-1.0.13\` 与 `dist\ainput-1.0.13.zip`

## 2026-04-16 修复按键精灵回放鼠标误暂停并持久化回放轮数

- 将按键精灵回放中的鼠标移动接管判断改为防抖确认：
  - 鼠标点击和滚轮仍会立即暂停
  - 单次轻微鼠标抖动不再直接误判为人工接管
- 托盘 `按键精灵` 菜单保留 `1` 到 `5` 轮快捷项，并新增 `设置自定义回放轮数...`
- 自定义回放轮数会立即作用到当前运行态，并写回 `config\ainput.toml` 的 `[automation].repeat_count`
- 下次启动会自动恢复上一次使用的回放轮数

## 2026-04-14 打包 1.0.12 便携版

- 将 workspace 版本从 `1.0.11` 提升到 `1.0.12`
- 同步 `README.md` 当前正式版本和便携版目录路径
- 基于当前修复后的代码重新构建 release
- 产出 `dist\ainput-1.0.12\` 与 `dist\ainput-1.0.12.zip`

## 2026-04-14 修复语音蓝图标卡死、补托盘重启、收口录屏失败状态

- 语音热键链路新增 worker 失联兜底：
  - worker 线程异常退出会主动回推错误事件
  - 主线程发现 worker channel 失效时会自动重建，不再只把托盘停在蓝色图标
- 托盘 `通用` 菜单新增 `重新启动`，会直接拉起新进程并退出旧进程，便于现场自救
- 录屏服务补齐失败状态回写：
  - 框选启动失败会明确回到 `Error`
  - 停止导出失败会明确回到 `Error`
  - 不再只在后台日志打印失败、前台状态却一直卡在录屏中

验证：

- `cargo build -p ainput-desktop`
- 本轮构建在 `C:\Users\sai\ainput` 下通过

## 2026-03-26 语音触发 emoji 方案设计

- 将“句尾说笑死 -> [破涕为笑]”纳入当前 `SPEC.md` / `PLAN.md` / `TASKLIST.md`
- 明确首版必须依赖现有输出上下文判断，只在光标位于末尾时触发
- 记录设计决策：该规则先落在上下文感知输出规则，不直接塞进纯文本归一化函数

## 2026-03-26 语音触发 emoji 实施完成

- 在 `ainput-output` 中增加首版 voice action 规则：仅当上下文为 `EditableAtEnd` 时，将句尾 `笑死` 替换为 `[破涕为笑]`
- 保持 `EditableWithContentOnRight`、`Unknown` 默认不触发，避免句中和未知上下文误替换
- 补充句尾命中、句中不命中、终端不命中、命中后不再补句号的单元测试
- 顺手修复 `ainput-output` crate 独立测试缺少 `windows` feature 的问题，使 `cargo test -p ainput-output` 可单独通过

## 2026-03-26 清除终端特例

- 删除语音输出链路里的 `terminal_strip_trailing_period` 与 `terminal_processes` 正式配置项
- 删除输出层按进程名区分 `TerminalLike` 的逻辑，不再把终端作为特例类别
- 统一保留三种上下文：`EditableAtEnd`、`EditableWithContentOnRight`、`Unknown`
- 统一口径改为：未知上下文默认补句号；emoji 规则只在 `EditableAtEnd` 触发

## 2026-03-26 扩充句尾 emoji 映射

- 将句尾语音触发从单条 `笑死 -> [破涕为笑]` 扩成 8 条固定映射
- 本轮新增支持：
  - `偷笑 -> [偷笑]`
  - `哭死 -> [流泪]`
  - `震惊 -> [震惊]`
  - `点赞 -> [强]`
  - `抱拳 -> [抱拳]`
  - `狗头 -> [狗头]`
  - `捂脸 -> [捂脸]`
- 将“emoji 命中后不补句号”改为通用逻辑，避免新映射命中后被追加 `。`

## 2026-03-26 同步配置与 README 文档

- 在 `config\ainput.toml` 的 `[voice]` 段补充句尾 emoji 语音触发说明
- 明确当前规则为内置固定映射，只能查看说明，不能通过 TOML 自定义
- 在 `README.md` 中新增“句尾 Emoji 触发”章节，并同步当前 8 条支持映射与触发边界

## 2026-03-26 修复单个“那个 / 就是”被误删

- 定位到问题不在重复词折叠，而在 `ainput-rewrite` 的开头 filler 清理
- 移除单个 `那个` 与单个 `就是` 的“开头直接删除”规则
- 保留重复口头禅 `那个那个` / `就是就是` 的清理能力
- 补充回归测试，确保单个语义词保留、重复口头禅仍会清除

## 2026-03-26 收紧截图热键触发条件

- 排查到截图进入十字态只有 `WM_HOTKEY -> ScreenshotTriggered -> start_capture_session` 这一条入口
- 原实现收到 `WM_HOTKEY` 后直接启动截图，没有再核对截图主键是否真的仍处于按下状态
- 为截图热键新增物理按键复核：只有修饰键和主键当前都处于按下状态时才真正触发截图
- 这样即使系统侧出现偏脏的 `WM_HOTKEY`，单独按 `Alt` 也不会再误进截图态

## 2026-03-26 为截图热键增加真实主键门禁

- 根据终端前台复现现象，继续收紧截图热键入口
- 现在除了 `WM_HOTKEY` 外，还必须先在低层键盘钩子里观察到截图主键的真实 `keydown`
- 对当前默认 `Alt+X` 而言，只有先看到真实 `X keydown`，再收到系统热键消息，才允许进入截图态
- 这样可以进一步挡住“只按了 Alt，但系统错误投递截图热键消息”的场景

## 2026-03-26 放弃截图的 RegisterHotKey 入口

- 用户实测证明：在 Tabby / 终端前台，单按 `Alt` 仍会误进十字截图态
- 这说明问题不只是门禁不够，而是截图的 `RegisterHotKey` 入口在终端前台本身不可靠
- 因此将截图热键改为只走低层键盘钩子：
  - 必须观察到截图主键的真实 `keydown`
  - 且修饰键条件同时满足
  - 才发送 `ScreenshotTriggered`
- 现在线路与语音热键一致，不再依赖系统投递的 `WM_HOTKEY`

## 2026-03-25

### 初始化

- 固定项目名为 `ainput`
- 固定项目根目录为 `C:\Users\sai\ainput`
- 确认总体路线为：
  - Rust 主程序
  - sherpa-onnx Rust API
  - SenseVoiceSmall
  - Python 不进入最终运行时

### 本次产出

- 建立项目级协作文件
- 重写 Spec / Plan / Tasklist / Architecture
- 建立技术决策记录
- 建立工作流文档
- 建立最小 Rust workspace 骨架
- 建立 `apps/ainput-desktop` 与 6 个基础 crates
- 建立数据目录与内置词表 / 模板样例

### 当前状态

- 仍处于方案冻结阶段
- Rust 项目骨架已存在
- 尚未开始实际功能实现

### 下一轮起点

- 从 Round 1 Rust workspace 骨架开始

### 目标收敛调整

- 将首版目标从“语音识别 + 提示词转换工具”收敛为“语音识别后直接贴入当前输入区域”
- 明确提示词转换、术语增强、模板账本不再作为首版阻塞项
- 将首版验收固定为：
  - 按住 `Ctrl+Win` 录音
  - 松开后离线识别
  - 结果直接粘贴
  - 失败时降级到剪贴板

### 文档同步

- 更新 `AGENTS.md`
- 更新 `README.md`
- 更新 `SPEC.md`
- 更新 `PLAN.md`
- 更新 `TASKLIST.md`
- 更新 `ARCHITECTURE.md`
- 更新 `DECISIONS.md`
- 记录当前 workspace 已通过 `cargo check`

### Round 1 完成

- 在 `ainput-shell` 中建立默认配置模型与运行目录约定
- 启动时可自动生成 `config/ainput.config.json`
- 建立基础 tracing 日志初始化，日志写入 `logs/ainput.log`
- `ainput-desktop` 已改为通过统一 bootstrap 入口启动

验证：

- `cargo check`
- `cargo run -p ainput-desktop`
- 启动后已生成默认配置文件与日志文件

### Round 2 完成

- 确认首版 ASR 直接使用官方 `sherpa-onnx` Rust API
- 在 `ainput-asr` 中接入 SenseVoice 离线识别
- 固定模型目录约定为 `models/sense-voice`
- 新增自动发现模型 bundle 的逻辑，兼容直接目录和子目录模型包
- 主程序新增 `transcribe-wav` 与 `record-once` 调试入口
- 已下载官方 `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17` 模型包

验证：

- `target\\debug\\ainput-desktop.exe transcribe-wav models\\sense-voice\\sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17\\test_wavs\\zh.wav`
- `target\\debug\\ainput-desktop.exe transcribe-wav models\\sense-voice\\sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17\\test_wavs\\en.wav`
- `target\\debug\\ainput-desktop.exe record-once 1`
- `transcribe-wav zh.wav` 实测耗时约 `2.56s`
- 日志已记录 recognizer 创建、录音开始/结束等关键事件

### Round 3 完成

- 用 `device_query` 实现按住 `Ctrl+Win` 的轮询式状态机
- 接入 `cpal` 默认麦克风输入
- 实现“按下开始录音、松开停止录音、随后转写”的常驻主循环
- 识别失败、录音失败、输出失败时改为记录日志并继续运行，而不是直接退出

验证：

- `cargo build -p ainput-desktop`
- 后台启动 `target\\debug\\ainput-desktop.exe` 2 秒，确认常驻主循环可正常启动

### Round 4 完成

- 用 `arboard` 实现剪贴板写入
- 用 `enigo` 实现 `Ctrl+V` 自动粘贴
- 实现自动粘贴失败时降级为仅写剪贴板
- 增加 `logs\\last_result.txt` 作为最近结果缓存

验证：

- `logs\\last_result.txt` 已生成并写入最新转写结果
- `logs\\ainput.log` 已记录启动、识别、录音等关键链路日志

### 可见性修正

- 修复从 `target\\debug\\ainput-desktop.exe` 直接启动时的根目录识别问题
- 现在优先按可执行文件祖先目录回溯到项目根目录，而不是误落到 `target\\debug`
- 接入系统托盘图标
- 去除 Windows 原生通知
- 新增录音中的底部悬浮条提示，位置固定在屏幕下方、任务栏上方
- 增加托盘菜单：
  - 使用说明
  - 退出

验证：

- 从 `target\\debug` 目录直接执行 `..\\debug\\ainput-desktop.exe bootstrap`，现在会回到项目根配置路径
- 默认启动后进程可持续运行，不再一闪而过

### 1.0 基础版打包完成

- 将热键监听从轮询改为 Windows 原生全局键盘 hook
- 将底部悬浮条动画改为主线程按帧驱动，避免后台线程直接操作窗口
- release 版启用 `windows_subsystem = "windows"`，正式版不再弹黑色命令行窗口
- 生成 `dist\\ainput-1.0.0-base` 独立运行目录
- 生成 `dist\\ainput-1.0.0-base.zip` 归档包

验证：

- `cargo build --release -p ainput-desktop`
- 从 `dist\\ainput-1.0.0-base\\run-ainput.bat` 启动，确认 release 包可正常拉起
- 进程路径已确认落在 `dist\\ainput-1.0.0-base\\ainput-desktop.exe`

### 输出细节打磨

- 在 `ainput-output` 中新增基于 Windows UI Automation 的光标右侧内容判断
- 输出前会先检查当前焦点输入区域的插入点是否已在文档结尾
- 若光标右侧仍有内容，则移除识别结果末尾的中文句号 `。`
- 若光标右侧没有内容，则保持已有句末标点；若完全没有句末标点，则补一个 `。`
- 若当前输入框不支持读取光标文本范围，则保持原始识别结果，不强行改写

验证：

- `cargo check -p ainput-output`
- `cargo test -p ainput-output`
- `cargo check -p ainput-desktop`
- `cargo build -p ainput-desktop`

### 术语增强与自动学习

- 托盘菜单新增：
  - `学习最近一次修正`
  - `手动添加易错词`
- `手动添加易错词` 会直接打开同一份纯文本用户术语文档 `data\\terms\\user_terms.txt`
- 用户现在只需要在文档上半部分一行填写一个希望强化的正确词，不必手动列出所有误识别形式
- 输出前会对英文技术词做保守的 glossary 模糊纠正
- 用户复制修正后的整段文本后，点击 `学习最近一次修正`，程序会将：
  - 最近一次原始识别结果
  - 当前剪贴板中的修正结果
  做单词级对比
- 程序自动学习到的误识别映射会写回同一份 `user_terms.txt` 文档下半部分
- 若识别到同一个误识别词被修正为同一个标准词两次，则自动写入同一份术语文档并开始生效

验证：

- `cargo test -p ainput-output`
- `cargo check -p ainput-desktop`

### 轻量正则化默认开启

- 在 `ainput-rewrite` 中实现轻量正则化
- 默认在识别输出后自动执行：
  - 去句首赘词
  - 去高频连续重复词
  - 中英混排空格规范
  - 保守保持原意，不做大幅改写
- 当前正则化已并入默认主链路，不需要额外开关

验证：

- `cargo test -p ainput-rewrite`
- `cargo check -p ainput-desktop`

### 输出整句粘贴延迟优化

- 移除主链路里识别完成后、输出前的固定 `120ms` 人为等待
- 将直接粘贴的按键发送方式从 `Ctrl + Unicode('v')` 调整为更标准的 `Ctrl + V`
- 当前输出链路仍然是“整句识别完成后一次性粘贴”，并没有实现逐词流式插入
- 若用户仍感觉像“分批弹出”，更可能是目标应用自身对粘贴内容的渲染表现，而不是 `ainput` 在分段发送文本

验证：

- `cargo check -p ainput-output`
- `cargo check -p ainput-desktop`

### 分阶段耗时日志

- 在主识别链路中新增阶段耗时日志：
  - 音频时长
  - ASR 耗时
  - 正则化耗时
  - 输出耗时
  - 整体耗时
  - 实时倍率（总耗时 / 音频时长）
- 在输出层新增更细粒度日志：
  - 术语纠正耗时
  - 标点与光标上下文处理耗时
  - 写剪贴板耗时
  - 发送 `Ctrl + V` 耗时
  - 粘贴稳定等待耗时

验证：

- `cargo check -p ainput-output`
- `cargo check -p ainput-desktop`

### 去掉粘贴稳定等待

- 删除直接粘贴阶段原先保留的固定 `80ms` 等待
- 保留分阶段日志，用于继续观察在不同输入框中是否出现漏粘贴或偶发失败
- 当前直接粘贴路径改为“写剪贴板后立即发送 `Ctrl + V`”，不再人为等待

验证：

- `cargo check -p ainput-output`
- `cargo build -p ainput-desktop`

### CPU 线程数基准调整

- 使用固定基准语音样例，对当前 `debug` 版做同机对比测试
- `num_threads = 1` 时，3 次测量约为：
  - `2700.7ms`
  - `2532.1ms`
  - `2530.0ms`
- `num_threads = 4` 时，3 次测量约为：
  - `2285.8ms`
  - `2298.8ms`
  - `2326.6ms`
- `num_threads = 8` 时，3 次测量约为：
  - `2898.0ms`
  - `2891.4ms`
  - `2859.5ms`
- 补充测试：
  - `num_threads = 2`：
    - `3463.9ms`
    - `3280.6ms`
    - `3223.0ms`
  - `num_threads = 3`：
    - `3295.0ms`
    - `3372.6ms`
    - `3151.6ms`
  - `num_threads = 5`：
    - `2662.9ms`
    - `2673.6ms`
    - `2979.0ms`
  - `num_threads = 6`：
    - `2932.3ms`
    - `2950.2ms`
    - `2846.9ms`
- 结论：当前机器与模型组合下，`4` 线程明显优于 `1` 线程，且 `8` 线程出现明显回退，因此默认配置调整为 `4`
- 补充结论：线程数不要求是偶数，但这台机器上 `2/3/5/6` 都没有优于 `4`

验证：

- 基准样例：`tmp\\benchmark.wav`
- 测试命令：`target\\debug\\ainput-desktop.exe transcribe-wav tmp\\benchmark.wav`

### 后台资源心跳监控

- 新增后台资源心跳线程
- 程序启动后会定期把当前进程的资源状态写入日志，便于观察长期驻留时是否出现异常增长
- 当前心跳日志包含：
  - CPU 使用率
  - 工作集内存
  - 虚拟内存
  - 运行时长
- 当前实现只做监控，不主动做“自动清理内存”或“自动重建识别器”

验证：

- `cargo check -p ainput-desktop`

### 静音误识别抑制

- 在识别前新增静音能量分析：
  - 峰值幅度
  - RMS
  - 活跃采样占比
- 若录音整体接近静音，则直接跳过 ASR，不再让模型对静音“猜词”
- 在极低能量前提下，再对特别短的可疑结果做一次兜底拦截，避免类似 `ユ.` 这类静音幻觉文本被输出
- 静音被拦截时，程序直接回到待机状态，不输出任何文本

验证：

- `cargo check -p ainput-desktop`

### 自定义应用图标

- 将根目录 `logo.png` 转换为适合图标使用的透明背景多尺寸资源
- 生成图标文件：
  - `assets\\app-icon.ico`
  - `assets\\app-icon-256.png`
- 托盘图标改为优先加载新的 `app-icon.ico`
- `ainput-desktop` 新增 Windows 资源编译步骤，生成的 EXE 会内嵌同一套图标资源
- 若运行时找不到图标文件，托盘仍会回退到旧的占位图标，避免启动失败
- 根据实际可见性问题再次调整图标：
  - 将主体由黑色改为白色
  - 进一步压缩透明留白，让图标在任务栏中更显眼

验证：

- `cargo check -p ainput-desktop`
- `cargo build -p ainput-desktop`

### 鼠标中键长按录音

- 新增鼠标中键长按录音方案：
  - 短按中键仍保留原有中键点击功能
  - 长按中键 `200ms` 后才进入录音
  - 松开中键后停止录音并识别
- 为避免与中键原生行为冲突，短按时会补发原始中键点击；只有进入录音态后才由程序接管本次中键行为
- 托盘右键菜单新增“启用鼠标中键长按录音”开关
- 菜单开关会实时生效，并写回 `config\\ainput.config.json`
- `Ctrl+Win` 主快捷键仍保持默认开启；在 `Ctrl+Win` 组合中，程序会优先吞掉关键的 `Win` 键事件，尽量避免被系统或其他软件抢走

验证：

- `cargo check -p ainput-desktop`

### 托盘“使用说明”菜单修复

- 修复托盘右键菜单中“使用说明”点击后只有状态变化、没有实际动作的问题
- 当前点击“使用说明”会直接用记事本打开项目根目录下的 `README.md`

验证：

- `cargo build -p ainput-desktop`

### Ctrl+Win 粘滞问题修复

- 修复 `Ctrl+Win` 组合偶发导致系统误以为 `Win` 键仍处于按下状态的问题
- 根因是：当用户先按 `Win`、后按 `Ctrl` 时，系统可能先看到了 `Win down`，但后续 `Win up` 被程序接管，导致 Windows 自身状态残留
- 当前修复方式：
  - 若检测到 `Win down` 已被系统接收、随后又进入了程序自己的 `Ctrl+Win` 录音组合
  - 程序会主动补发一次 `Win key up`
  - 并吞掉后续对应的物理 `Win up`，避免重复干扰
- 这样可以把系统侧“Win 键卡住 / 字母都变成 Win 组合键 / 松开瞬间弹菜单”的风险收掉

验证：

- `cargo check -p ainput-desktop`

### Ctrl+Win 粘滞问题二次收口

- 首轮修复仍有遗漏：当用户先按 `Win`、后按 `Ctrl` 进入录音，再先松开 `Ctrl` 时，旧状态机会把仍按住的 `Win` 重新标记成“待处理单键”
- 这会导致后续 `Win up` 被错误地还原成单独 `Win` 行为，从而再次触发开始菜单或留下系统级 `Win` 粘滞感
- 当前改成更严格的状态机：
  - 只要 `Ctrl+Win` 组合已经成立，后续剩余的 `Win` 只允许被吞掉，不再回退成单独 `Win`
  - 新增 `WIN_SUPPRESS_UNTIL_UP` 标记，专门处理“组合键结束后只剩下 Win 还按着”的分支
  - `Win` 只有在从未形成组合键时，才允许回放成单独 `Win` 的正常系统行为
- 这样可以从根上收掉“先按到 Win 就把整个系统带偏”的问题

验证：

- `cargo check -p ainput-desktop`
- `cargo build -p ainput-desktop`

### 代码整理与 README 回写

- 将桌面端最重的“录音 -> 静音过滤 -> 识别 -> 正则化 -> 输出”流水线从 `main.rs` 拆到独立 `worker.rs`
- 主入口文件从约 `797` 行收缩到约 `527` 行，行为不变，但后续维护和排障更清晰
- 删除桌面端未使用的 `ainput-data` 依赖
- `.gitignore` 新增：
  - `tmp/`
  - `data/terms/user_terms.txt`
- `README.md` 按当前真实状态重写，补齐以下内容：
  - 当前已实现功能
  - 两种触发方式
  - 托盘菜单
  - 用户术语文档
  - 自动学习机制
  - 智能句号
  - 日志与调试命令
  - 配置项
  - 正式版构建与目录结构

验证：

- `cargo check -p ainput-desktop`
- `cargo test -p ainput-output -p ainput-rewrite`

### 托盘菜单默认值与开机启动

- 将“启用鼠标中键长按录音”的默认值从开启调整为关闭
- 配置文件新增：
  - `startup.launch_at_login`
- 托盘右键菜单新增“开机自动启动”开关，默认开启
- 开机自动启动通过当前用户 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 注册表项实现
- 启动时会按当前配置自动对齐注册表状态
- `README.md`、默认配置文件、打包脚本说明一并回写，避免默认行为与文档不一致

验证：

- `cargo check -p ainput-desktop`
- `cargo build --release -p ainput-desktop`
- `powershell -ExecutionPolicy Bypass -File .\scripts\package-release.ps1 -Version 1.0.2`

### 安装包交付

- 新增安装脚本：`scripts\install-ainput.ps1`
- 新增卸载脚本：`scripts\uninstall-ainput.ps1`
- 新增安装包构建脚本：`scripts\build-installer.ps1`
- 当前安装包方案：
  - 使用系统自带 `IExpress`
  - 继续保留便携版 zip
  - 额外生成单文件安装包 `dist\ainput-setup-1.0.2.exe`
- 安装行为固定为：
  - 安装到 `%LOCALAPPDATA%\Programs\ainput`
  - 创建开始菜单入口
  - 写入卸载注册信息
  - 默认启动程序，由程序自身按配置同步开机自启
- 卸载行为固定为：
  - 停止已安装实例
  - 清理开机自启
  - 清理开始菜单入口
  - 清理卸载注册信息
  - 删除安装目录
- `README.md` 已回写为“安装包优先”的使用口径

验证：

- `cargo build --release -p ainput-desktop`
- `powershell -ExecutionPolicy Bypass -File .\scripts\build-installer.ps1 -Version 1.0.2`
- `powershell -ExecutionPolicy Bypass -File .\scripts\install-ainput.ps1 -PayloadZip .\dist\ainput-1.0.2.zip`
- `powershell -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\ainput\scripts\uninstall-ainput.ps1" -InstallDir "$env:LOCALAPPDATA\Programs\ainput"`
# 2026-03-26

- 启动截图能力开发，补充 `SPEC.md` / `PLAN.md` / `TASKLIST.md` / `DECISIONS.md`
- 确认截图走 Rust + Windows API 路线，并把桌面自动保存定义为截图结果的可选第二出口
- 完成 `Win+Alt` 截图热键、冻结框选窗口、图片剪贴板输出、桌面自动保存开关与 PNG 落盘
- 通过 `cargo check -p ainput-desktop` 与 `cargo build -p ainput-desktop`，并已自动拉起本地测试版 `target/debug/ainput-desktop.exe`

## 2026-03-26 体检与重构规划

- 完成一轮面向当前 worktree 的全库体检
- 当前状态：
  - `cargo check` 通过
  - `cargo test` 通过，但覆盖主要集中在文本处理
  - `cargo clippy --all-targets --all-features -- -D warnings` 未通过
  - `cargo fmt --check` 未通过
- 新确认的高优先级问题：
  - 句号策略在终端类输入区失效
  - 自动学习机制不可见、不可迁移、不可确认
  - 缺少滚动语音历史
  - 托盘菜单信息结构混乱
  - 热键不可配置，且主逻辑仍依赖复杂 Win 键状态机
  - 语音 / 截图链路的系统接口选择未完全收口
  - 截图剪贴板存在位图句柄所有权风险
  - 存在伪配置项、根目录发现过宽、空闲高频 tick、版本号分散硬编码
- 已同步更新：
  - `SPEC.md`
  - `PLAN.md`
  - `TASKLIST.md`
- 正式启动 Round 9“稳定性与产品化重构”，后续按专项方案推进，而不是继续做零散补丁

## 2026-03-26 Round 9 第一轮重构完成

- 配置正式升级为 `config\ainput.toml`
- 启动时支持从旧 `ainput.config.json` 自动迁移到新 TOML
- 语音默认热键恢复为 `Ctrl+Win`
- 截图默认热键恢复为 `Alt+Win`
- 语音与截图共用统一热键配置模型
- 截图热键改为 Windows 原生 `RegisterHotKey`
- 语音链路保留按住/松开语义，并增加可配置热键字符串解析
- 终端 / 控制台 / 类 TTY 输入区增加单独标点策略，默认不乱补句号
- `ainput-output` 改为长期持有术语资产，不再每次输出时重新装载
- 术语与学习系统改为结构化文件：
  - `data\terms\base_terms.json`
  - `data\terms\user_terms.json`
  - `data\terms\learned_terms.json`
- 内置 AI 编程词库显著扩充，覆盖常见模型、工具、工程术语
- 托盘菜单重做为：
  - `语音`
  - `截图`
  - `术语与学习`
  - `通用`
- 新增 `logs\voice-history.log`，滚动保留最近 500 条语音结果
- `last_result.txt` 与语音历史写入改为独立维护线程处理
- 截图剪贴板所有权 bug 已修复，避免部分成功路径下二次释放位图句柄
- 运行根发现逻辑改为严格模式，不再静默退回错误 cwd
- overlay 空闲 tick 从 `7ms` 降到 `33ms`
- 移除默认后台资源心跳，避免后台维护动作干扰前台主链路
- 新增单实例接管机制：启动第二个桌面实例时，会先结束旧实例，再由新实例接管

## 2026-03-26 终端与 Win 组合专项收口

- 终端类进程默认识别新增：
  - `Tabby.exe`
  - `tabby-agent.exe`
- 当前终端句号策略明确收口为：
  - Windows PowerShell / pwsh / Tabby 统一按终端类保守策略处理
  - 不再试图把这几类终端误当成普通可编辑文本框
- 热键层把 `Ctrl+Win` 和 `Alt+Win` 从通用逻辑中拆开，改为独立的 Win 组合管理：
  - `Ctrl+Win` 单独负责按住说话
  - `Alt+Win` 单独负责截图触发
  - 组合激活后会吞掉相关 `Win` 组合事件，降低误弹开始菜单的概率
- 内置词库新增常见工程词：
  - `OpenClaw`
  - `gateway`
  - `session`
  - `watchdog`
  - `memory`
  - `workspace`
  - `Gmail`
  - `Calendar`
  - `VPS`
  - `Cloudflare`
  - `ASR`
  - 以及用户指定的其余基础词

验证：

- `cargo build -p ainput-desktop`
- `cargo test`
- `cargo fmt --check`

验证：

- `cargo build -p ainput-desktop`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo fmt --check`
- `target\debug\ainput-desktop.exe bootstrap`
- `target\debug\ainput-desktop.exe clipboard-selftest-image`
- `target\debug\ainput-desktop.exe capture-fullscreen-selftest`

## 2026-03-26 1.0.3 打包与中文路径兼容

结果：

- 工作区版本号统一升级到 `1.0.3`
- 发布脚本、安装脚本、README 打包文案同步切到 `1.0.3`
- 修复中文安装路径下的 ASR 运行时兼容问题：
  - `sherpa-onnx` 直接读取中文模型路径不稳定
  - 现在检测到非 ASCII 模型目录时，会把 `model.onnx` / `tokens.txt` 缓存到 `%LOCALAPPDATA%\\ainput\\asr-cache\\...`
  - 后续直接从 ASCII 安全缓存目录加载，避免因安装目录含中文而无法启动或无法识别
- `transcribe-wav` 不再依赖 `sherpa-onnx` 的文件路径读取，改为本地读取 WAV 再喂样本，避免中文路径下的 WAV 打开失败
- 开机启动注册表命令路径改成更稳妥的 Unicode 字符串引用方式

验证：

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`
- `cargo build --release -p ainput-desktop`
- `powershell -ExecutionPolicy Bypass -File .\scripts\package-release.ps1 -Version 1.0.3`
- 将 `dist\ainput-1.0.3.zip` 解压到 `C:\Users\sai\AppData\Local\Temp\中文路径-ainput-107`
- 在中文路径内执行：
  - `.\ainput-desktop.exe transcribe-wav .\models\sense-voice\sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17\test_wavs\en.wav`
  - 成功生成 `logs\last_result.txt`
- 在中文路径内直接启动 `ainput-desktop.exe`，进程保持常驻

## 2026-03-26 默认热键改为 Alt+Z / Alt+X

结果：

- 默认语音热键改为 `Alt+Z`
- 默认截图热键改为 `Alt+X`
- 默认运行口径不再占用 `Win` 组合作为主热键，系统 `Win` 键恢复原生默认行为
- 打包说明、配置模板、旧配置迁移默认值、README 与计划文档同步更新
- 删除原先为 `Ctrl+Win / Alt+Win` 保留的专项兼容层，不再在主链路里保留无效的 `Win` 组合吞键状态机

验证：

- `cargo build -p ainput-desktop`
- `target\debug\ainput-desktop.exe bootstrap`
- `powershell -ExecutionPolicy Bypass -File .\scripts\package-release.ps1 -Version 1.0.3`

## 2026-03-26 截图模式增加暗膜反馈

结果：

- 截图窗口进入时会先显示整屏冻结画面，并覆盖一层半透明黑色暗膜
- 拖选区域会回亮到原始截图亮度，形成“内亮外暗”的视觉反馈
- 选区边框改为 1px 白色描边，鼠标仍保持十字
- 取消或完成截图时，遮罩会随截图窗口立即消失
- 本轮未加入任何淡入淡出动画

实现说明：

- 直接在现有截图全屏窗口里完成绘制，没有新增第二层遮罩窗口
- 预先生成一份变暗后的截图位图，绘制时先铺满暗图，再把选区原图贴回去
- 删除旧的焦点框反相重绘方式，改成标准整窗重绘，避免视觉闪烁

验证：

- `cargo fmt`
- `cargo test -p ainput-desktop --no-run`

补充修正：

- 首版暗膜实现会在进入截图前额外生成一整张变暗位图，导致截图起手延迟明显
- 现已改为绘制时直接对原始截图叠加黑色透明层，不再预先处理整屏像素
- 目标是把截图进入速度恢复到接近旧版，同时保留暗膜反馈
- 后续又进一步改成“进程内复用同一个截图窗口”，不再每次截图都创建/销毁全屏窗口
- 这一步是为了继续压低任务栏右下角通知区域在截图开关瞬间的 Shell 抖动
## 2026-05-09 preview.52 streaming final inserted-tail overlap fix
Result:
- Fixed preview.51 duplicated final commit text from the user case.
- Root cause: offline final tail corrected a displayed suffix but previous merge treated it as append-only.
- Packaged preview.52 and launched it in the Windows interactive session, PID 84284.
Verification:
- cargo test -p ainput-desktop passed: 103/103.
- Exact raw capture replayed on preview.52 with duplicate_case=false.
- Packaged raw corpus passed: 4/4.
- Startup idle acceptance passed for preview.52.

## 2026-05-09 preview.53 HUD truth single-chain v19
Result:
- Replaced the old streaming release path with the V19 HUD truth single-chain design.
- Removed release-time offline final repair from the default streaming commit path.
- Final paste now uses the drained HUD truth snapshot exactly once.
- AI rewrite was moved out of the active streaming commit scope and left for a future spec.
Verification:
- cargo test -p ainput-desktop passed: 103/103 at the V19 checkpoint.
- User preview.51 failure raw replayed without inserted duplicate tail.
- Packaged preview.53 was built and launched in the Windows interactive session.
## 2026-05-09 preview.54 punctuation and tray version closeout
Result:
- Removed default hard-coded streaming punctuation insertion for connector words and semantic cue words.
- Sanitized unanchored generated question/exclamation marks so `这个怎么回事啊` does not become `这个怎么？回事啊？`.
- Added the current package version to the tray tooltip and right-click menu: `当前版本：1.0.0-preview.54`.
- Updated README handoff and packaged preview.54 as the current stop point.
Verification:
- cargo fmt --check passed.
- cargo test -p ainput-rewrite passed: 16/16.
- cargo test -p ainput-desktop streaming_punctuation passed: 4/4.
- cargo test -p ainput-desktop passed: 106/106.
- scripts\readme_closeout_guard.py passed.
- Packaged exe file version is 1.0.0-preview.54 and the Windows interactive process is running from preview.54.

## 2026-05-10 preview.69 Qwen streaming normalization and fast commit

Result:
- Bumped package to `1.0.0-preview.69`.
- Added `[voice.streaming.qwen3]` config for Qwen context and streaming/vLLM parameters.
- Set Qwen defaults to `chunk_size_sec=0.18`, `unfixed_chunk_num=4`, `unfixed_token_num=5`, `max_new_tokens=64`, `enforce_eager=false`.
- Changed Qwen sidecar idle unload to `3600000ms`.
- Updated Qwen context for Chinese/English mixed realtime dictation, formal normalization, no forced punctuation on short pauses, and no repeated recognized content.
- Changed AI rewrite prompt into formal normalization: oral/rough ASR tail can be rewritten into correct, formal, natural language without changing intent.
- Kept Qwen forced terminal punctuation call as a commented restore point instead of deleting it.
- Kept non-streaming / fast SenseVoice punctuation path independent from Qwen streaming.
- Changed WSL auto-start to detach through `powershell.exe Start-Process wsl.exe`, while WSL runs `.venv/bin/python -m uvicorn qwen3_asr_sidecar:app`.

Verification:
- `cargo fmt --all` passed.
- `cargo test -p ainput-shell` passed: 6/6.
- `cargo test -p ainput-desktop` passed: 113/113.
- `python -m py_compile C:\Users\sai\ainput\tmp\qwen3_asr_sidecar.py` passed.
- `scripts\package-release.ps1 -Version 1.0.0-preview.69` produced `dist\ainput-1.0.0-preview.69` and zip.
- Windows interactive scheduled-task launch kept `ainput-desktop.exe` running from preview.69.
- WSL `pgrep -af uvicorn` showed `/home/sai/ainput-qwen3-asr/.venv/bin/python -m uvicorn qwen3_asr_sidecar:app --host 127.0.0.1 --port 8765`.
- `/health` returned `idle_unload_ms=3600000`, `requested_enforce_eager=false`, `effective_enforce_eager=false`, `enforce_eager_fallback=true`.
