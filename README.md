# ainput

`ainput` 是一个 Windows 本地常驻的“语音输入 + 截图 + 按键精灵”工具。

当前正式版本：`1.0.5`

它不做系统级 IME，也不依赖在线模型。当前重点是把三条前台主链路做稳：

1. 按住语音热键开始录音，松开后离线识别并把文本送进当前输入区
2. 按截图热键进入冻结框选态，完成后把图片送进剪贴板
3. 按自动化热键录制和回放真实键盘鼠标操作

当前默认热键：

- 自动化暂停 / 继续：`F7`
- 语音：`Alt+Z`
- 截图：`Alt+X`
- 自动化录制：`F8`
- 自动化保存：`F9`
- 自动化回放：`F10`
- 自动化停止当前录制 / 回放：`Esc`

正式配置文件：

- `config\ainput.toml`

## 当前功能

- 本地离线 ASR
  - 模型：`SenseVoiceSmall`
  - 运行时：`sherpa-onnx Rust API`
  - 默认：`CPU / 4 线程`
- 语音输入主链路
  - 语音热键可配置
  - 自动直贴失败时，可按配置降级到剪贴板
  - 普通输入框与终端输入区使用不同句号策略
- 截图主链路
  - 截图热键走 Windows 原生 `RegisterHotKey`
  - 冻结整屏后框选
  - 结果复制到剪贴板
  - 可选自动保存 PNG 到桌面
- 按键精灵主链路
  - 内置 `10` 个录制槽位
  - 可录制真实键盘和鼠标输入
  - 按原始时间顺序回放
  - 支持 `F7` 暂停 / 继续
  - 回放中若检测到你手动插入键鼠输入，会自动暂停
  - 可在托盘切换槽位和回放轮数
  - 录制文件默认放在 `data\automation\slots\`
- 托盘菜单重构
  - 分为 `语音`、`截图`、`按键精灵`、`术语与学习`、`通用`
  - 三条主能力各自独立收口
- 术语与学习
  - 内置 AI 编程词库：`data\terms\base_terms.json`
  - 用户词库：`data\terms\user_terms.json`
  - 学习状态：`data\terms\learned_terms.json`
  - 从当前剪贴板学习最近一次修正，达到阈值后自动生效
- 语音历史
  - 最近一条：`logs\last_result.txt`
  - 滚动历史：`logs\voice-history.log`
  - 默认只保留最近 `500` 条
- 前后台解耦
  - 语音历史与最近结果落盘走独立维护线程
  - 前台语音识别和截图链路不等待这些后台写文件动作
  - 周期性资源心跳已移除，避免后台维护动作持续打扰主程序

## 当前托盘菜单

- `状态`
  - 显示当前待机、录音、识别、截图、错误等状态
- `语音`
  - 显示当前语音热键
  - 可开关鼠标中键长按录音
  - 可打开语音历史
- `截图`
  - 显示当前截图热键
  - 可开关“截图后自动保存到桌面”
- `按键精灵`
  - 显示当前自动化状态
  - 显示 `F7 / F8 / F9 / F10 / Esc` 控制热键
  - 切换 `10` 个录制槽位
  - 切换 `1` 到 `5` 轮回放
  - 打开槽位名称文件
  - 打开录制目录
- `术语与学习`
  - 从当前剪贴板学习最近一次修正
  - 打开用户术语文件
  - 打开学习状态文件
  - 打开内置 AI 词库
- `通用`
  - 开机自动启动
  - 打开配置文件
  - 打开日志目录
  - 使用说明
- `退出`

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

推荐直接使用安装包：

```text
dist\ainput-setup-1.0.5.exe
```

安装后：

- 程序默认安装到 `%LOCALAPPDATA%\Programs\ainput`
- 可从开始菜单里的 `ainput` 启动
- 可通过系统“已安装的应用”或开始菜单里的卸载入口移除

## 日常使用

### 语音输入

- 按住 `Alt+Z`
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
- 当前默认关闭鼠标中键长按录音
- 如果你想启用鼠标中键，可以在托盘菜单里打开

### 截图

- 按 `Alt+X`
- 进入冻结框选态
- 拖拽选区
- 松开鼠标后，图片写入剪贴板
- 如果托盘里开启了“截图后自动保存到桌面”，会额外保存 PNG 文件

### 按键精灵

- 按 `F8` 开始录制当前槽位
- 完整走一遍你的键盘和鼠标操作
- 按 `F9` 停止录制并覆盖保存当前槽位
- 按 `F10` 回放当前槽位
- 回放过程中按 `F7` 可暂停，再按一次继续
- 回放进行中或已暂停时，只要你手动插入键盘或鼠标操作，都会自动暂停
- 录制或回放过程中按 `Esc` 都只会停止当前流程，不会退出程序
- 槽位名在 `data\automation\slot-names.json`
- 录制事件在 `data\automation\slots\slot-1.json` 到 `slot-10.json`

## 术语与学习怎么用

相关文件：

- `data\terms\base_terms.json`
- `data\terms\user_terms.json`
- `data\terms\learned_terms.json`

内置词库会随安装包一起提供，重点覆盖 AI 编程场景；用户词库和学习状态会在首次运行时自动创建。

### 自动学习最近一次修正

流程：

1. 先让程序输出一段有错误的识别结果
2. 你在输入框里手工改对
3. 复制“改对后的整段文本”
4. 右键托盘，进入 `术语与学习`
5. 点击 `从当前剪贴板学习最近一次修正`

效果：

- 第一次：记录候选
- 第二次同样修正：自动升格为正式映射并开始生效

## 标点策略

当前规则分两类：

- 普通文本编辑区：
  - 文中插入时，末尾 `。` 和 `.` 会被去掉
  - 行尾输出时，缺句号会补 `。`
- 终端 / 控制台 / 类 TTY 输入区：
  - 默认走保守策略
  - 不再因为拿不到 UIA 上下文就强行补句号

## 视觉反馈

录音时会显示底部悬浮条：

- 出现时滑入
- 消失时滑出
- 录音中按麦克风音量变化
- 位置固定在屏幕下方、任务栏上方

## 日志与调试

日志与历史文件：

- `logs\ainput.log`
- `logs\last_result.txt`
- `logs\voice-history.log`

日志里会记录：

- 录音开始/结束
- 静音判断
- ASR 耗时
- 正则化耗时
- 输出耗时
- 总流水线耗时
- 上下文与输出策略

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

只测图片写入剪贴板：

```powershell
.\target\debug\ainput-desktop.exe clipboard-selftest-image
```

只测整屏截图复制：

```powershell
.\target\debug\ainput-desktop.exe capture-fullscreen-selftest
```

## 配置文件

配置文件路径：

- `config\ainput.toml`

主要配置包括：

- `[hotkeys].voice_input`
- `[hotkeys].screen_capture`
- `[hotkeys].mouse_middle_hold_enabled`
- `[voice].prefer_direct_paste`
- `[voice].fallback_to_clipboard`
- `[voice].history_limit`
- `[startup].launch_at_login`
- `[asr].model_dir`
- `[asr].provider`
- `[asr].num_threads`
- `[learning].auto_activate_threshold`
- `[logging].level`

说明：

- 旧版 `config\ainput.config.json` 只作为迁移输入保留，不再是正式配置口径
- `[voice]` 段里额外带有“句尾 emoji 语音触发”的说明注释，当前规则是内置固定映射，不通过 TOML 单独配置
- 按键精灵当前先沿用固定热键 `F7 / F8 / F9 / F10 / Esc`，还没有并入 `ainput.toml`

## 当前已知取舍

- 当前默认还是 `CPU` 推理，不走 GPU
- 语音热键与截图热键已经可配置，但当前仍以编辑 `ainput.toml` 为主
- 截图热键走 Windows 原生 `RegisterHotKey`
- 语音为了保留“按住说话/松开停止”的行为，仍需要低层按键监听配合
- 按键精灵录制与回放当前也依赖低层键盘鼠标 hook
- 开机自动启动通过当前用户的 `Run` 注册表项实现
- 不同应用对直接粘贴的前台体验可能略有差异
- 某些不支持 UI Automation 的输入框，会退到统一的未知上下文策略

## 句尾 Emoji 触发

当前内置了一组“句尾口述 -> emoji token”的固定规则，用于聊天或吐槽场景。

触发条件：

- 只在程序判断当前光标位于末尾时触发
- 只在句尾触发，不在句中替换
- 上下文未知时默认不触发

当前支持：

- `笑死` -> `[破涕为笑]`
- `偷笑` -> `[偷笑]`
- `哭死` -> `[流泪]`
- `震惊` -> `[震惊]`
- `点赞` -> `[强]`
- `抱拳` -> `[抱拳]`
- `狗头` -> `[狗头]`
- `捂脸` -> `[捂脸]`

例子：

- 口述：`这个 bug 太离谱了笑死`
- 行尾输出：`这个 bug 太离谱了[破涕为笑]`

边界：

- 当前映射是写死在程序里的，还不能在 `ainput.toml` 里自定义增删
- 若你在句中说“我都快笑死了但是还没说完”，不会被替换成 emoji

## 打包正式版

构建正式版：

```powershell
cargo build --release -p ainput-desktop
```

正式版不会弹黑色命令行窗口。

当前发布目录结构使用：

- `dist\ainput-setup-1.0.5.exe`
- `dist\ainput-1.0.5\`
- `dist\ainput-1.0.5.zip`

## 项目结构

- `apps\ainput-desktop`
  - 桌面入口、托盘、热键、按键精灵接线、底部提示条、后台维护线程
- `crates\ainput-automation`
  - 键盘鼠标录制、槽位管理、回放执行
- `crates\ainput-audio`
  - 麦克风录音
- `crates\ainput-asr`
  - SenseVoice + sherpa-onnx
- `crates\ainput-rewrite`
  - 轻量正则化
- `crates\ainput-output`
  - 输出、上下文判断、术语学习
- `crates\ainput-shell`
  - 启动、配置、日志
- `data\terms`
  - 内置词库、用户词库、学习状态

## 当前版本定位

它现在重点解决的是：

- 本地离线语音输入
- AI 编程场景下的中英混合口述
- 语音、截图、按键精灵三条主链路的稳定常驻
- 后台维护动作与前台主链路解耦
