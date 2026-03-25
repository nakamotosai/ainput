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
