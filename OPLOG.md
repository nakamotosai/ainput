# ainput OPLOG

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
