# ainput DECISIONS

## D-022 多语言 RNNT 不再作为默认在线 ASR

- 日期：2026-05-11
- 状态：accepted

原因：

- `nvidia/parakeet-1_1b-rnnt-multilingual-asr` 的 `multi` 自动语言模式在中文实时 HUD partial 上出现严重语言漂移，无法稳定识别中文。
- 用户中文输入是核心路径之一；中文不可用时，默认在线 ASR 必须回到中文专用模型。

决策：

- live 默认回滚到 `preview.76` 和 `nvidia/parakeet-ctc-0_6b-zh-cn`。
- `preview.77` 视为失败实验包，不继续作为默认运行版本。
- 后续 preview 应把中文、日文/英文拆成显式模式或显式语言配置，禁止再用单一 `multi` auto 模式承载三语默认输入。

---

## D-021 在线 ASR 默认切到 NVIDIA Parakeet 多语言 RNNT

- 日期：2026-05-11
- 状态：accepted

原因：

- 用户主要说日文、中文、英文，中文专用 `nvidia/parakeet-ctc-0_6b-zh-cn` 不适合作为长期默认在线模型。
- NVIDIA Build 页面提供 `nvidia/parakeet-1_1b-rnnt-multilingual-asr`，用户已明确要求替换到这个多语言版本。
- 在线 ASR 已经在 `preview.76` 独立成第三模式，本轮只替换在线模式的远端模型，不应再影响本地 Qwen 流式模式。

决策：

- `preview.77` 默认在线模型为 `nvidia/parakeet-1_1b-rnnt-multilingual-asr`。
- NVIDIA function id 使用 `71203149-d3b7-4460-8231-1be2543a1fca`。
- 在线语言参数使用 `multi`，配置面写入 `[voice.online_streaming].language = "multi"`。
- vps-jp adapter 和 Windows 包内 sidecar 保持同一模型 / function id / language 默认值。

放弃：

- 不删除中文 CTC 在线回滚包 `preview.76`。
- 不改本地 Qwen / Sherpa 配置和显存策略。
- 不把 NVIDIA key 写入 Windows 包、repo、dist 或日志。

---

## D-020 在线 ASR 必须是独立语音模式

- 日期：2026-05-11
- 状态：accepted

原因：

- `preview.74` / `preview.75` 把在线 Parakeet 暂时塞进 `[voice.streaming]`，导致托盘仍只显示“极速 / 流式”，用户无法区分本地 Qwen 流式和在线流式。
- 本地 Qwen 仍需要保留为可回退能力，不能因为在线试验覆盖它的配置、显存策略和启动行为。
- 在线 ASR 的上屏策略应更简单：HUD partial 已经是用户可见真相，松手时不应再被本地 Qwen 的 context echo guard、age gate、AI rewrite 或 final HUD ack 阻塞。

决策：

- 新增独立模式 `online_streaming`，配置为 `[voice.online_streaming]`，默认启动走该模式。
- `[voice.streaming]` 重新代表本地流式模式，默认回到 `qwen3_sidecar`、本地 `127.0.0.1:8765`、WSL auto-start、`gpu_memory_utilization = 0.30` 与 `gpu_enabled = true`。
- 在线模式复用 sidecar HTTP contract，但 worker kind、托盘菜单、生命周期状态和配置面独立。
- 在线松手时，如果 HUD 已有文本，先直接粘贴 HUD snapshot；远端 finish/session cleanup/raw capture 保存后台完成。

放弃：

- 不把在线 ASR 继续伪装成本地流式 backend。
- 不删除本地 Qwen / Sherpa 路径。
- 不在本轮修改 `cliproxyapi` 8317 或 NVIDIA key pool。

---

## D-019 临时引入 NVIDIA Parakeet 在线 ASR adapter

- 日期：2026-05-11
- 状态：temporary

原因：

- 本机 Qwen3-ASR 0.6B 在 Windows GPU 上显存占用过高，且近期真实热键链路不稳定。
- NVIDIA Parakeet CTC zh-CN 在线 API 已通过试用验证效果可用，但它是 Riva gRPC/NVCF，不是 OpenAI 兼容音频转写接口。
- 现有 `vps-jp` `cliproxyapi` 8317 生产配置已经持有 5 个 NVIDIA key；临时 adapter 可读取该 key pool，避免把 key 写入 Windows 包。

决策：

- 新增第三个流式 backend：`nvidia_parakeet_online`。
- AInput 仍走现有 sidecar session HTTP contract，临时 adapter 在 `vps-jp` 上把 HTTP session 转成 NVIDIA Riva gRPC offline recognition。
- `preview.75` 默认使用在线 backend，`sidecar_auto_start = false`，不自动拉起本地 Qwen WSL sidecar；在线 adapter 的 `/chunk` 必须返回实时 partial，不能再只在 `/finish` 出最终文本。
- `preview.74` 默认使用在线 backend，`sidecar_auto_start = false`，不自动拉起本地 Qwen WSL sidecar。

放弃：

- 不修改 `cliproxyapi` 8317 生产服务本体。
- 不在 Rust 主程序里直接实现 Riva gRPC。
- 不把 NVIDIA key 放进 TOML、dist 或 git。

---

## D-001 选用 Rust 作为主语言

- 日期：2026-03-25
- 状态：accepted

原因：

- 项目是 Windows 常驻本地工具
- 目标是轻、快、稳、长期自用
- Rust 更适合热键、状态机、音频、托盘、系统集成

放弃：

- 纯 Python 主程序
- Python 作为最终运行时主壳

---

## D-002 ASR 采用 SenseVoiceSmall

- 日期：2026-03-25
- 状态：accepted

原因：

- 推理延迟低
- 支持中文、粤语、英语、日语、韩语
- 与当前场景高度匹配

来源：

- Hugging Face README_zh
- sherpa-onnx SenseVoice 文档

---

## D-003 ASR 运行时采用 sherpa-onnx

- 日期：2026-03-25
- 状态：accepted

原因：

- 与 SenseVoiceSmall 路线匹配
- 有 Rust API 和 C API 可选
- 更适合 Rust 主程序集成

---

## D-004 最终运行时默认不引入 Python

- 日期：2026-03-25
- 状态：accepted

原因：

- 减少常驻体积和复杂度
- 保持运行时一致性
- 降低打包与部署负担

说明：

- Python 允许作为开发辅助脚本存在
- Python 不进入最终常驻主链路

---

## D-005 产品不是系统级 IME

- 日期：2026-03-25
- 状态：accepted

原因：

- 目标是高效自用工具，而非平台级输入法产品
- TSF/IME 路线复杂度过高
- 当前主流程更适合“热键录音 + 输出注入”

---

## D-006 UI 保持极简

- 日期：2026-03-25
- 状态：accepted

原因：

- 自用工具优先效率，不优先视觉
- 首版尽量把复杂度让给主流程，而不是外观

---

## D-007 首版目标收敛到“按住说话后直接贴入”

- 日期：2026-03-25
- 状态：accepted

原因：

- 当前真正刚需是语音识别输入，不是自动生成提示词
- 先把主链路做通，才能验证这类工具是否值得长期自用
- 术语增强、提示词转换、模板账本都属于后续增强，不应阻塞首版交付

首版验收口径：

- 按住语音热键开始录音
- 松开后触发离线识别
- 识别结果直接贴到当前输入区域
- 自动粘贴失败时按配置降级到剪贴板

---

## D-008 首版 ASR 实现先落 sherpa-onnx Rust API

- 日期：2026-03-25
- 状态：accepted

原因：

- 当前官方 `sherpa-onnx` crate 已可直接完成 SenseVoice 离线识别
- 已在本机验证 `wav -> 文本` 与 `麦克风 -> 文本` 最小闭环
- 先用官方 Rust API 能减少首版 FFI 包装成本

说明：

- C API 继续保留为兜底方案
- 只有在 Rust API 后续暴露稳定性或打包问题时才回退

---

## D-009 首版模型目录约定

- 日期：2026-03-25
- 状态：accepted

约定：

- 配置中的 `asr.model_dir` 默认指向 `models/sense-voice`
- 若该目录下直接存在 `model.int8.onnx` / `tokens.txt`，则直接使用
- 若该目录下存在子目录模型包，则自动发现第一套可用 SenseVoice bundle

原因：

- 便于开发阶段直接解压官方模型包
- 兼容后续你替换不同版本模型时的目录差异

---

## D-010 Windows 安装包先采用 IExpress

- 日期：2026-03-25
- 状态：accepted

原因：

- 当前机器已自带 `IExpress`
- 不需要额外安装 NSIS / Inno Setup / WiX
- 对当前“单用户、自用、快速收口”的发布诉求足够

约定：

- 继续保留 zip 便携包
- 额外产出一个 `setup.exe` 安装包
- 安装目录先固定为当前用户的 `LocalAppData\\Programs\\ainput`

---

## D-011 截图能力采用 Rust + Windows GDI/窗口 API

- 日期：2026-03-26
- 状态：accepted

原因：

- 当前需求是 Windows 本地常驻截图，不需要跨平台抽象
- 单次截图更适合直接走 Windows 原生屏幕采集与顶层窗口交互
- 能与现有低层热键、托盘、主事件循环自然集成

约定：

- 截图热键默认采用可配置方案，当前默认值为 `Alt+X`
- 首版截图结果默认写入剪贴板
- 托盘可选开启“额外自动保存到桌面”
- 自动保存文件格式固定为 PNG

---

## D-012 配置升级到 TOML 并保留 JSON 迁移入口

- 日期：2026-03-26
- 状态：accepted

原因：

- 现有 JSON 配置不适合长期维护，也不利于人工编辑
- 热键、语音、截图、学习、日志已经变成多段结构，TOML 更适合承载
- 仍需兼容历史安装和旧工作目录里的 `ainput.config.json`

约定：

- 正式配置文件改为 `config\ainput.toml`
- 启动时若没有 TOML 但存在旧 JSON，则自动迁移
- 旧 JSON 只作为迁移输入，不再作为正式配置口径

---

## D-013 前台主链路优先，后台维护动作默认不打扰

- 日期：2026-03-26
- 状态：accepted

原因：

- 语音识别和截图是产品主链路，日志、历史、学习状态落盘都只是维护动作
- 后台维护动作不应该阻塞前台输出或截图完成

约定：

- 最近结果和语音历史落盘统一走独立维护线程
- 周期性资源心跳从默认运行改为移除
- 前台链路不等待历史落盘完成再返回结果

---

## D-014 句尾 emoji 触发先落“上下文感知输出规则”，不直接塞进纯文本归一化

- 日期：2026-03-26
- 状态：accepted

原因：

- “笑死”替换为 `[破涕为笑]` 依赖当前光标是否在末尾，不是纯文本清洗
- 现有 `ainput-rewrite::normalize_transcription()` 不知道前台输入框上下文，直接塞进去会导致句中误替换
- 首版应依赖输出层的光标上下文判断，而不是基于应用类型做特判

约定：

- 首版 emoji 触发规则只在 `EditableAtEnd` 命中时生效
- 首版只支持句尾口述 `笑死` -> `[破涕为笑]`
- `EditableWithContentOnRight` 与 `Unknown` 默认不触发
- 规则执行顺序固定为：
  - 文本归一化
  - 输出上下文判断
  - emoji 触发替换
  - 句号策略处理
- 若后续语音触发规则增多，再单独抽成可扩展的 “voice actions” 层
