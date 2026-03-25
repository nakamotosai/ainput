# ainput

`ainput` 是一个 Windows 本地常驻语音输入工具。

它的目标不是做系统输入法，也不是做在线语音助手，而是把“按住说话”变成“把识别结果直接塞进当前输入框”。

当前版本已经可用，主链路是：

1. 按住快捷键开始录音
2. 松开后停止录音
3. 本地离线识别
4. 结果直接粘贴到当前输入区域
5. 若直接粘贴失败，则降级写入剪贴板

## 当前功能

- 本地离线 ASR
  - 模型：`SenseVoiceSmall`
  - 运行时：`sherpa-onnx Rust API`
  - 当前配置：`CPU / 4 线程`
- 常驻托盘
  - 启动后托盘可见
  - 右键菜单可直接操作
- 双触发方式
  - `Ctrl+Win` 按住说话
  - 鼠标中键长按约 `200ms` 说话
- 鼠标中键冲突处理
  - 短按中键仍保留原来的中键点击功能
  - 只有长按才进入录音
- 录音视觉反馈
  - 按住录音时，在屏幕下方、任务栏上方显示白色底部提示条
  - 带滑入、滑出和音量联动动画
- 静音抑制
  - 录到的内容接近静音时，直接跳过识别
  - 不再输出莫名其妙的垃圾字符
- 轻量正则化
  - 去掉常见句首赘词
  - 去掉连续重复词
  - 保守清理多余空白
- 智能句号
  - 光标右边如果还有内容，自动去掉末尾 `。` 或 `.`
  - 光标右边为空时，保留或补全句末标点
- 术语增强
  - 支持手动维护高频正确词
  - 支持从“最近一次手工修正”里学习映射
  - 同一个错误词纠正两次后自动生效
- 最近结果缓存
  - 最近一次识别结果会写入 `logs\last_result.txt`
- 资源日志
  - 后台定期记录 CPU 和内存心跳，便于排查长期驻留问题
- 自定义图标
  - EXE 图标与托盘图标使用同一套应用图标资源

## 当前托盘菜单

- `状态`
  - 显示当前待机、录音、识别、错误等状态
- `启用鼠标中键长按录音`
  - 可实时开关鼠标中键触发
- `学习最近一次修正`
  - 把最近一次识别文本与当前剪贴板文本做比较
  - 自动学习错误词到正确词的映射
- `手动添加易错词`
  - 打开用户术语文档
- `使用说明`
  - 用记事本打开本 README
- `退出`
  - 关闭常驻程序

## 快速开始

### 方式 1：开发版直接运行

```powershell
cargo build -p ainput-desktop
.\target\debug\ainput-desktop.exe
```

### 方式 2：双击运行最新开发版

```bat
run-latest.bat
```

这个脚本会自动：

- 关闭旧的 `ainput-desktop.exe`
- 重新编译最新代码
- 启动 `target\debug\ainput-desktop.exe`

### 方式 3：正式版运行

正式版包目录里直接双击：

```bat
run-ainput.bat
```

## 日常使用

### 键盘触发

- 按住 `Ctrl+Win`
- 说话
- 松开
- 等待识别结果自动进入当前输入框

### 鼠标触发

- 长按鼠标中键约 `200ms`
- 说话
- 松开
- 等待识别结果自动进入当前输入框

注意：

- 短按鼠标中键仍然保留原生功能
- 如果你不想启用鼠标中键，可以在托盘菜单里关闭

## 术语增强怎么用

用户词表文件：

`data\terms\user_terms.txt`

这个文件是纯文本格式，不是 JSON。

你只需要在上半部分一行写一个正确词，例如：

```txt
skill
emoji
OpenAI
Codex
Cursor
prompt
agent
```

程序会基于这些词做保守纠正。

### 自动学习最近一次修正

流程：

1. 先让程序输出一段有错误的识别结果
2. 你在输入框里手工改对
3. 复制“改对后的整段文本”
4. 右键托盘，点击 `学习最近一次修正`

效果：

- 第一次：记录候选
- 第二次同样修正：自动升格为正式映射并开始生效

例如：

- 第一次 `scale -> skill`
- 第二次 `scale -> skill`
- 之后程序自动把 `scale` 修成 `skill`

## 智能句号规则

当前规则很简单：

- 如果光标右边还有任何字符，末尾 `。` 和 `.` 会被去掉
- 如果光标右边没有内容，末尾句号会保留；没有句号时会补 `。`

这个规则主要用于解决“把语音插入到句子中间时，末尾总被多塞一个句号”的问题。

## 视觉反馈

当前录音提示不是系统通知，而是底部悬浮条：

- 出现时滑入
- 消失时滑出
- 录音中按麦克风音量变化

位置固定在：

- 屏幕下方
- 任务栏上方

## 日志与调试

日志文件：

- `logs\ainput.log`
- `logs\last_result.txt`

日志里会记录：

- 录音开始/结束
- 静音判断
- ASR 耗时
- 正则化耗时
- 输出耗时
- 总流水线耗时
- 周期性资源心跳

### 常用调试命令

只测引导：

```powershell
.\target\debug\ainput-desktop.exe bootstrap
```

只测麦克风识别，不测托盘：

```powershell
.\target\debug\ainput-desktop.exe record-once 3
```

只测 WAV 文件：

```powershell
.\target\debug\ainput-desktop.exe transcribe-wav .\some.wav
```

## 配置文件

配置文件路径：

`config\ainput.config.json`

当前主要配置包括：

- `shortcuts.push_to_talk`
- `shortcuts.mouse_middle_hold_enabled`
- `asr.model_dir`
- `asr.provider`
- `asr.num_threads`
- `output.prefer_direct_paste`
- `output.fallback_to_clipboard`
- `logging.level`

## 当前已知取舍

- 当前默认还是 `CPU` 推理，不走 GPU
- 当前热键方案基于 Windows 全局 hook
- `Ctrl+Win` 已针对 `Win` 粘滞问题做了专门状态机修复
- 不同应用对直接粘贴的前台体验可能略有差异
- 某些不支持 UI Automation 的输入框，智能句号会回退成保守行为

## 打包正式版

构建正式版：

```powershell
cargo build --release -p ainput-desktop
```

正式版不会弹黑色命令行窗口。

当前发布目录结构使用：

- `dist\ainput-1.0.1\`
- `dist\ainput-1.0.1.zip`

## 项目结构

- `apps\ainput-desktop`
  - 桌面入口、托盘、热键、底部提示条
- `crates\ainput-audio`
  - 麦克风录音
- `crates\ainput-asr`
  - SenseVoice + sherpa-onnx
- `crates\ainput-rewrite`
  - 轻量正则化
- `crates\ainput-output`
  - 输出、智能句号、术语学习
- `crates\ainput-shell`
  - 启动、配置、日志
- `data\terms`
  - 术语文档

## 当前版本定位

当前版本已经不是项目骨架，而是一个可日常使用的基础版。

它现在重点解决的是：

- 本地离线语音输入
- 中英混合技术口述的基础可用性
- 术语纠错
- 低打扰的常驻体验

后续增强项，例如更强的语义改写、更复杂的提示词整理、更多上下文智能，都不阻塞当前版本使用。
