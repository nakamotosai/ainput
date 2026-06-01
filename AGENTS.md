# ainput AGENTS

## 1. 项目定位

- 项目名：`ainput`
- 类型：Windows 本地常驻语音输入工具
- 目标用户：项目作者本人
- 主要用途：按住快捷键说话，松开后把离线识别结果直接贴到当前输入区域

## 2. 当前技术方向

- 主语言：Rust
- ASR 路线：`SenseVoiceSmall` + `sherpa-onnx`
- 运行时目标：避免 Python 进入最终常驻链路
- 优先接口：`sherpa-onnx Rust API`
- 兜底接口：`sherpa-onnx C API`

## 3. 设计原则

- 优先做“轻、快、稳”的常驻工具，不做大而全产品
- 首版优先最小可用链路，不为未来可能需求提前做复杂抽象
- UI 只保留必要交互，不做视觉炫技
- 所有核心逻辑应围绕这条主链路组织：
  - 热键
  - 录音
  - ASR
  - 输出注入
- 后续增强能力可以数据驱动，但首版不以前置术语表、模板、账本为阻塞项

## 4. 明确不做

- 不做系统级 IME/TSF 注册
- 不做在线大模型作为主链路依赖
- 不做账号系统、同步、团队协作
- 不做复杂窗口系统
- 不做“原项目翻译器”兼容层

## 5. 当前推荐模块边界

- `crates/ainput-shell`
  - 托盘
  - 热键
  - 状态机
  - 配置
- `crates/ainput-audio`
  - 录音采集
  - 音频缓冲
- `crates/ainput-asr`
  - sherpa-onnx 接入
  - SenseVoiceSmall 推理
- `crates/ainput-rewrite`
  - 后续术语增强
  - 后续提示词转换
- `crates/ainput-output`
  - 剪贴板
  - 自动粘贴
  - 输出降级
- `crates/ainput-data`
  - 词表
  - 模板
  - 账本数据模型
- `apps/ainput-desktop`
  - 主程序入口

## 6. 工作方式

- 新任务默认先更新 `SPEC.md` / `PLAN.md` / `TASKLIST.md`
- 中等及以上改动必须回写 `OPLOG.md`
- 若技术路线或依赖选择发生变化，必须回写 `DECISIONS.md`
- 每一轮实施后优先勾选 `TASKLIST.md`，而不是只在对话里汇报

## 7. 交付口径

- 先说明该轮达成了什么
- 再说明勾掉了哪些任务
- 再说明剩余阻塞
- 文件清单只在需要核对时展开

## 8. 运行交付规则

- 流式语音输入默认交互规则是：按住 `Ctrl` 开始录音，松开 `Ctrl` 收尾并上屏；不要再把流式热键改回 `Alt+Z`。
- 每次改完并完成必要验证、重新打包后，必须直接打开最新 `dist\ainput-<version>\ainput-desktop.exe`。
- 每次打包都必须生成一个新的版本目录和 zip，禁止覆盖旧 `dist` 包；这样坏版本可以直接回退到上一个包。
- 打开最新版前先停止旧的 `ainput-desktop.exe` 进程，避免用户仍在试旧版本。
- 启动必须进入用户当前 Windows 交互桌面会话；SSH 后台启动不算已打开。
- 收口时必须报告实际启动的 exe 路径和进程 PID。
