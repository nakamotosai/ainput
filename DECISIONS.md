# ainput DECISIONS

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

- 按住 `Ctrl+Win` 开始录音
- 松开后触发离线识别
- 识别结果直接贴到当前输入区域
- 自动粘贴失败时降级到剪贴板

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
