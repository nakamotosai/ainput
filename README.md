# ainput

`ainput` 是一个 Windows 本地常驻的“语音输入 + 截图 + 录屏 + 按键精灵”工具。

当前实测候选版本：`1.0.0-preview.80`；中文体感冻结基线仍是 `1.0.0-preview.78`（`1.0.0-preview.77` 多语言 RNNT 因中文实时识别严重漂移，已从 live 默认回滚）

本 README 是本项目唯一当前进度标准。

## 当前实测候选：1.0.0-preview.80

- 发布时间：2026-05-11。
- 目标：不换模型、不换部署、不使用 `multi` 模型，在现有 `nvidia/parakeet-ctc-0_6b-zh-cn` + `language=zh-CN` 链路上保守改善中英混说里的短英文岛。
- 当前入口：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.80\ainput-desktop.exe`。
- 当前包：`dist\ainput-1.0.0-preview.80\` 与 `dist\ainput-1.0.0-preview.80.zip`。
- 当前在线模型：`nvidia/parakeet-ctc-0_6b-zh-cn`，function id `9add5ef7-322e-47e0-ad7a-5653fb8d259b`，`language=zh-CN`。
- 当前 sidecar：`boost_enabled=true`，`boost=4.0`，词表来自 `sidecars\parakeet_code_switch_terms.json`，只放 Riva 可编码的低风险英文术语。
- 当前应用侧修复：只对已观察到的 `multi` 丢失、`猫底/某体` 误听、`扣代斯` 等 Codex 误听和常见英文产品词大小写做保守修复；普通中文负例必须保持原样。
- 回滚点：如果用户实测中文体感变差，优先回到 `1.0.0-preview.78`；如果只怀疑 speech context boost，可先在 sidecar 环境禁用 `PARAKEET_ENABLE_SPEECH_CONTEXTS`。

## 冻结基线：1.0.0-preview.78

- 冻结时间：2026-05-11。
- 冻结原因：用户在真实输入场景实测确认“效果很好，识别速度非常快，上屏速度也非常快”，本版本作为后续修改的基准版本。
- 基准入口：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.78\ainput-desktop.exe`。
- 基准包：`dist\ainput-1.0.0-preview.78\` 与 `dist\ainput-1.0.0-preview.78.zip`。
- 基准默认模式：`在线流式识别`，启动不自动加载本地 Qwen。
- 基准在线模型：`nvidia/parakeet-ctc-0_6b-zh-cn`，`language=zh-CN`，`partial_wait_sec=0.06`，`boost_enabled=false`。
- 基准上屏链路：按住 `Ctrl` 时 HUD 实时显示在线 partial；松开后优先粘贴 HUD snapshot，远端 finish / cleanup 后台完成。
- 基准回滚点：`1.0.0-preview.72` 保留为本地 Qwen 回滚版本；`1.0.0-preview.77` 只保留为多语言 RNNT 中文失败实验证据。

## 当前预览重点

- 这条版本线从 `v1.0` 预览重新开始，不再沿用旧的 `1.0.14-preview.x` HUD 补丁序列。
- `极速语音识别` 继续保留原有 `SenseVoice` 离线整段识别链路。
- `在线流式识别` 是第三个独立模式：默认走 `vps-jp` NVIDIA Parakeet adapter，不加载本地 Qwen；松开 `Ctrl` 时如果 HUD 已有文本，先立刻粘贴 HUD snapshot，再后台 finish/cleanup。
- `本地流式识别` 恢复为本机 Qwen/Sherpa 配置面，`qwen3_sidecar` 不再被在线 Parakeet backend 覆盖。
- `流式语音识别` 当前主线是 V19 单链路：`CtrlDown -> 空白 HUD -> streaming ASR -> HUD truth -> CtrlUp 停麦 -> drain -> 粘贴 HUD 文本一次 -> 关闭 HUD`。
- `1.0.0-preview.80` 在不换模型/部署的前提下修复中英混说短英文岛：保持 zh-CN CTC，启用低强度安全 speech context，并加应用侧精确后处理保护 `multi / Codex / OpenAI / API / CLI / GitHub` 等术语。
- `1.0.0-preview.78` 默认回到 `nvidia/parakeet-ctc-0_6b-zh-cn`，在线 adapter `partial_wait_sec` 收紧到 `0.06s`，并只使用 vps-jp Codex 用户指令历史里的高频英文词做 speech context boost。
- `1.0.0-preview.78` 在线 worker 每个 tick 最多发送一个远端 `/chunk`，避免积压的同步 HTTP chunk 阻塞松手命令；松手时仍完整 drain 队列。
- `1.0.0-preview.78` 中文数字归一化改为保守策略，只处理纯中文数字连读；`一起 / 一边 / 一下 / 一个` 等自然中文词不再被上屏改成 `1起 / 1边 / 1下 / 1个`。
- `1.0.0-preview.77` 把在线 Parakeet 默认模型切到 `nvidia/parakeet-1_1b-rnnt-multilingual-asr`，`language = "multi"`，用于日文 / 中文 / 英文混合输入验证。
- `1.0.0-preview.77` 验证失败：`multi` 自动语言在中文实时 partial 上会乱跳多语言，中文不可作为默认；live 已回滚到 `1.0.0-preview.76` 中文专用 CTC。
- `1.0.0-preview.76` 把在线 Parakeet 从本地流式配置里拆出来，托盘一级菜单显示 `极速语音识别 / 本地流式识别 / 在线流式识别` 三个模式。
- `1.0.0-preview.75` 修正在线 Parakeet adapter：按住 `Ctrl` 时 `/chunk` 会返回实时 partial，HUD 不再等松手后才一次性出字。
- `1.0.0-preview.74` 临时把默认流式主模型切到在线 `NVIDIA Parakeet CTC 0.6B zh-CN` adapter；启动不再自动加载本机 Qwen GPU 模型。
- `1.0.0-preview.72` 保留为本机 Qwen 回滚点；`qwen3_sidecar` 与 `sherpa` 仍保留为配置回退。
- 流式模式的官方标点模型固定为 `sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12-int8`。
- 流式模式不再在松手后跑 offline final，不再做 HUD 文本和 offline 尾巴合并，也不再做 release-time final correction。
- HUD 文本是最终真相源；最终上屏文本必须等于 drain 完成后的 HUD truth snapshot。
- `preview.72` 新增 Qwen context echo guard：如果 sidecar 把配置里的 context/prompt 当识别文本吐出来，必须在进入 HUD truth 之前拦截，不能先闪到 HUD 再清掉。
- `preview.72` 保持应用层 `voice.streaming.ai_rewrite.enabled = false`；当前只使用 Qwen ASR 自身的文本整理能力，不再叠加 8317 AI rewrite。
- `preview.68` 在 `preview.67` 的 preload 基础上，额外跑一小段真实 warm chunk，首句冷启动会更接近后续句子。
- `preview.68` 进一步把本地 `chunk_ms` 收紧到 `120ms`，并把 release drain 等待收短，目标是缩短“松手后 HUD 已经好了但上屏还要等”的体感。
- `preview.67` 的托盘 loading/ready/error 状态继续挂钩真实模型 readiness，而不是只看 worker 线程是否已启动。
- Qwen 空闲自动卸载当前为 `3600000ms`；默认空闲 `1` 小时后释放显存，下次使用或下次预加载时再重新拉起。
- 托盘右键菜单现在会直接显示当前版本号。
- AI rewrite 不属于 V19：本版流式主链路会绕开/隔离已有改写代码，改写能力移到 Roadmap / Future Work。
- 新增 `scripts\prune-artifacts.ps1`，可以清掉历史 `dist` / `target*` 构建垃圾，同时保留当前版本和一个回滚版本。
- 当前发包目录已经更新到 `dist\ainput-1.0.0-preview.80\` 与 `dist\ainput-1.0.0-preview.80.zip`。

它不做系统级 IME。当前默认热路径由本地 ASR/HUD 单链路负责；AI rewrite 暂不参与 V19 语音提交链路。当前重点是把四条前台主链路做稳：

1. 按住语音热键开始录音，在线流式模式松开后优先提交 HUD snapshot，再后台 finish/cleanup
1. 语音支持 `极速语音识别 / 本地流式识别 / 在线流式识别` 三模式，直接从托盘一级菜单切换
2. 按截图热键进入冻结框选态，完成后把图片送进剪贴板
3. 按录屏热键框选区域，实时录下视频和系统音频并导出到桌面
4. 按自动化热键录制和回放真实键盘鼠标操作
5. 按键精灵录制会持久保存在用户目录，升级版本不会再跟着 `dist\ainput-x.y.z` 目录一起丢失

当前默认热键：

- 自动化暂停 / 继续：`F7`
- 极速语音：`Alt+Z`
- 流式按住说话：`Ctrl`
- 截图：`Alt+X`
- 录屏开始框选 / 开始录制：`F1`
- 录屏停止并导出：`F2`
- 自动化录制：`F8`
- 自动化保存：`F9`
- 自动化回放：`F10`
- 自动化停止当前录制 / 回放：`Esc`

正式配置文件：

- `config\ainput.toml`
- `config\hud-overlay.toml`

## 当前功能

- 本地离线 ASR
  - 模型：`SenseVoiceSmall`
  - 运行时：`sherpa-onnx Rust API`
  - 默认：`CPU / 4 线程`
- 语音输入主链路
  - 托盘一级菜单直接切换 `极速语音识别 / 本地流式识别 / 在线流式识别`
  - `极速语音识别` 保留原有 `SenseVoice` 离线整段识别
  - `在线流式识别` 默认使用 `vps-jp` 上的 NVIDIA Parakeet CTC zh-CN adapter；本地 Qwen / Sherpa 只在切到 `本地流式识别` 后进入对应配置面
  - 流式模式按住热键时显示 HUD 面板
  - 流式模式按住时持续显示流式文字，默认只走本地识别 + 本地轻整理
  - 流式模式会持续喂入在线音频块，HUD truth state 是最终提交的唯一文本来源
  - 应用层短停顿 endpointing 仍保留为配置项，但默认关闭；流式默认在一次按住说话内保持同一条滚动状态
  - Qwen sidecar 当前本地流式块时长为 `120ms`，WSL sidecar auto-start 环境会用 `chunk_size_sec=0.18` / `unfixed_chunk_num=4` / `unfixed_token_num=5`
  - `preview.56` 的最终提交直接来自 Qwen final text 清理结果，不再用可能滞后的 HUD state 截断最终上屏文本
  - `preview.57` 的 HUD partial 直接显示当前识别文本，不再使用逐字 microstream 追赶，避免模型已返回但 HUD 迟迟不上屏
  - `preview.58` 的 Qwen partial 绕开旧 sherpa 稳定策略；Qwen 每次返回的递增文本只做规范化/标点清理/去重后立即推到 HUD
  - `preview.59` 的 HUD partial 使用自适应快速微流式显示：短尾逐字流动，长尾每帧最多追 8 字，避免整段跳变也避免重新积压
  - `preview.59` 修复 `Qwen3-ASR` 被误听成 `千万三ASR` / `千问三ASR` 后又被中文数字归一化成 `10000003ASR` 的问题
  - `preview.67` 启动或切换语音模式时会先进入对应模型的加载态；Qwen worker 只有在 sidecar/model 真 ready 后才会上报 ready
  - 当前 Qwen ready 态会按 `sidecar_idle_unload_ms = 3600000` 本地推导空闲卸载 deadline，托盘会在下次 idle 超时后回到“未加载”
  - 流式模式的标点主链来自官方 `ct-transformer` 标点模型；模型缺失时只降级为无标点，不再让整个流式功能启动失败
  - V19 流式提交链路禁用 AI rewrite 写入；已有改写代码不能改 HUD truth 或最终上屏文本
  - HUD 默认停靠在屏幕正下方、任务栏上方
  - 可从托盘右键菜单直接打开 `HUD 参数文档`
  - `config\hud-overlay.toml` 保存后会自动热加载
  - 松开热键后只停止继续收麦克风；HUD 会继续等待队列音频和 ASR 输出 drain，drain 完成后粘贴 HUD 文本一次并关闭
  - 流式模式会异步保存每次按住说话的原始录音到 `logs\streaming-raw-captures\`，自动只保留最近 `20` 组 wav + json
  - 语音热键可配置
  - 自动直贴失败时，可按配置降级到剪贴板
  - 普通输入框与终端输入区使用不同句号策略
- 截图主链路
  - 截图热键走 Windows 原生 `RegisterHotKey`
  - 冻结整屏后框选
  - 结果复制到剪贴板
  - 可选自动保存 PNG 到桌面
- 录屏主链路
  - `F1` 进入框选，框选完成后立即开始实时录屏
  - `F2` 停止录屏并导出 MP4 到桌面
  - 支持系统音频内录
  - 支持鼠标录制开关
  - 支持水印开关、文本、位置、移动闪现、随机游走
  - 支持 `30 / 60 / 90 / 144 FPS`
  - 高帧率录屏会强制按目标帧率输出，并在封装后校验输出视频的实际帧率
  - 支持 `低 / 中 / 高` 三档画质
- 按键精灵主链路
  - 内置 `10` 个录制槽位
  - 可录制真实键盘和鼠标输入
  - 按原始时间顺序回放
  - `F7` 只暂停 / 继续回放，不会打断录制
  - 回放中若检测到你手动插入键盘、鼠标点击、滚轮或明确鼠标移动，会自动暂停
  - 鼠标移动暂停带防抖，不会再因为单次轻微抖动就立刻误暂停
  - 槽位名称文件改完后会自动刷新到托盘菜单，无需重启
  - 录制时会显示底部动态提示条；回放时会显示从左向右推进的总进度条
  - 可在托盘切换槽位、`1` 到 `5` 轮预设，或输入自定义回放轮数
  - 自定义回放轮数会立即生效，并在重启后继续沿用
  - 录制文件默认放在 `data\automation\slots\`
- 托盘菜单重构
  - 分为 `语音`、`截图`、`录屏`、`按键精灵`、`术语与学习`、`通用`
  - 四条主能力各自独立收口
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
- 异常恢复与重置
  - 语音线程异常退出后，会立刻回到错误态，不再把托盘蓝色图标长时间卡住
  - 录屏启动失败或导出失败后，会回到明确错误态，而不是继续卡在录屏状态
  - 托盘 `通用` 菜单内置 `重新启动`，可直接重拉整个常驻进程

## 当前托盘菜单

- `状态`
  - 显示当前待机、录音、识别、截图、错误等状态
- `当前版本`
  - 托盘一级菜单直接显示当前 `preview` 版本号
- `极速语音识别`
  - 一级菜单直接切到离线整段识别模式
- `本地流式识别`
  - 一级菜单直接切到本机 Qwen/Sherpa HUD 流式提交模式
- `在线流式识别`
  - 一级菜单直接切到 vps-jp NVIDIA Parakeet 在线 HUD 流式提交模式
- `打开 HUD 参数文档`
  - 一级菜单直接打开 `config\hud-overlay.toml`
  - 可调整 HUD 字号、字体、颜色、宽度、圆角、位置、停留时间等参数
  - 保存后立即热加载，无需重启
- `语音`
  - 显示当前语音热键
  - 可开关鼠标中键长按录音
  - 可打开语音历史
- `截图`
  - 显示当前截图热键
  - 可开关“截图后自动保存到桌面”
- `录屏`
  - 显示当前录屏状态
  - 显示 `F1 / F2` 控制热键
  - 可开关系统音频、鼠标录制、水印
  - 可设置水印文本
  - 可切换水印位置、帧率、画质
- `按键精灵`
  - 显示当前自动化状态
  - 显示 `F7 / F8 / F9 / F10 / Esc` 控制热键
  - 切换 `10` 个录制槽位
  - 切换 `1` 到 `5` 轮回放，或输入自定义轮数
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
  - 重新启动
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

### 方式 3：便携正式版运行

正式交付只推荐便携版：

```text
dist\ainput-1.0.0-preview.78\
dist\ainput-1.0.0-preview.78.zip
```

说明：

- 直接运行目录里的 `ainput-desktop.exe` 或 `run-ainput.bat`
- 后续版本发布默认只验证便携版目录和 zip
- 安装包流程已废弃，不再作为收口标准

### 方式 4：跑流式长句回归

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\streaming-regression.ps1
```

说明：

- 这条脚本会直接调用 `target\release\ainput-desktop.exe`
- 默认会生成一条拼接长句样本，并跑固定 wav 阈值检查
- 这条旧回归主要检查“可见字符数是否明显大于老 bug 的 2 到 3 个字”
- 结果会同时写入 `tmp\streaming-regression-latest.txt`

V3 实时改写自测：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-selftest.ps1
```

说明：

- 这条脚本会重新生成 `fixtures\streaming-selftest\manifest.json`
- 每条样本同时检查首字延迟、最终延迟、rollback 次数、最大 rollback 字数和 keywords 命中
- `expected` 不完全一致时，必须满足 keywords gate；不再允许“乱码但字符数够”的假通过

## 日常使用

### 语音输入

默认模式看托盘一级菜单当前勾选项。

#### 极速语音识别

- 按住 `Alt+Z`
- 说话
- 松开
- 等待整段离线识别结果自动进入当前输入框

#### 在线流式识别

- 在托盘一级菜单切到 `在线流式识别`
- 按住 `Ctrl`
- 说话时屏幕正下方 HUD 只显示当前识别文字，最新尾巴允许被实时修正
- 如果你正在编辑 `config\hud-overlay.toml`，保存后 HUD 会立刻刷新
- 松开后程序优先提交 HUD snapshot，远端 finish 和清理在后台完成
- 如果直贴失败，会按配置退回到剪贴板
- 如果想调字号、颜色、宽度或位置，直接在托盘右键点 `打开 HUD 参数文档`

#### 本地流式识别

- 在托盘一级菜单切到 `本地流式识别`
- 按住 `Ctrl`
- 本地 Qwen/Sherpa 会按配置加载模型；这一路仍保留本机显存与 idle unload 策略
- 松开后程序按本地流式配置 drain，并把 HUD truth 文本提交到输入框

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

### 录屏

- 按 `F1`
- 进入框选态
- 拖拽并松开鼠标后立即开始录屏
- 录制过程中可正常继续操作电脑
- 按 `F2` 停止录屏
- 成品 MP4 默认输出到桌面

录屏配置都在托盘 `录屏` 菜单里：

- 是否录制系统音频
- 是否录制鼠标移动
- 是否启用水印
- 水印文本
- 水印位置：左上 / 右上 / 左下 / 右下 / 移动闪现 / 随机游走
- 帧率：`30 / 60 / 90`
- 画质：低 / 中 / 高

### 按键精灵

- 按 `F8` 开始录制当前槽位
- 会出现半透明状态提示，托盘图标会切到录制态
- 完整走一遍你的键盘和鼠标操作
- 按 `F9` 停止录制并覆盖保存当前槽位
- 按 `F10` 回放当前槽位
- 回放过程中状态提示会持续更新；按 `F7` 可暂停，再按一次继续
- 回放进行中或已暂停时，只要你手动插入键盘、鼠标点击、滚轮或明显鼠标移动，都会自动暂停
- 单次轻微鼠标抖动不会再直接触发暂停；需要达到防抖阈值才会判定为你在接管鼠标
- 托盘里可以直接切 `1` 到 `5` 轮，或通过“设置自定义回放轮数...”输入任意 `1` 到 `999` 轮
- 回放轮数改完会立即生效，并且会写回 `config\ainput.toml`
- 编辑 `data\automation\slot-names.json` 后，槽位名会自动刷新到托盘菜单
- 录制或回放过程中按 `Esc` 都只会停止当前流程，不会退出程序
- 槽位名在 `data\automation\slot-names.json`
- 录制事件在 `data\automation\slots\slot-1.json` 到 `slot-10.json`

## 术语与学习怎么用

相关文件：

- `data\terms\base_terms.json`
- `data\terms\user_terms.json`
- `data\terms\learned_terms.json`

内置词库会随便携正式版一起提供，重点覆盖 AI 编程场景；用户词库和学习状态会在首次运行时自动创建。

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
- `logs\streaming-raw-captures\streaming-raw-*.wav`
- `logs\streaming-raw-captures\streaming-raw-*.json`

日志里会记录：

- 录音开始/结束
- 静音判断
- ASR 耗时
- 正则化耗时
- 输出耗时
- 总流水线耗时
- 上下文与输出策略
- 流式原始录音留样路径和自动裁剪结果

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

只测 streaming 模型 WAV 文件：

```powershell
.\target\debug\ainput-desktop.exe transcribe-streaming-wav .\some.wav
```

只测一轮真麦克风流式识别并输出结构化证据：

```powershell
.\target\debug\ainput-desktop.exe probe-streaming-live 8
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
- `[automation].repeat_count`
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
- 按键精灵回放轮数已经并入 `ainput.toml` 的 `[automation].repeat_count`，托盘里修改后会立即写回

## 当前状态

- 当前可直接实测的便携版是 `dist\ainput-1.0.0-preview.80\`
- 默认启动模式是 `在线流式识别`
- 当前源码在线流式主链是：`NVIDIA Parakeet CTC zh-CN online adapter + ASR-facing mono/16k + HUD truth state + responsive chunk feed + immediate HUD snapshot paste + background finish`
- 默认热路径由本地 ASR/HUD 单链路负责；V19 不跑 release-time offline final，不做 HUD/offline 尾巴合并，不在松手后二次修正 HUD 文本
- AI rewrite 已从当前版本范围移出；V19 只保证已有改写代码不会改 HUD truth 或最终上屏文本
- `preview.80` 是基于 `preview.78` 的保守中英混说修复候选；`preview.78` 是中文体感冻结基线和快速回滚点；`preview.77` 是失败的多语言 RNNT 实验包；`preview.76` 是中文 CTC 在线模式回滚点；`preview.72` 仍是本机 Qwen 回滚点
- `preview.71` 会泄漏 Qwen context 到 HUD，不建议作为回滚点；需要回滚时优先回到最后一个已验证可接受的旧包并重新验证真实麦克风链路
- 收口门禁脚本是 `scripts\readme_closeout_guard.py`

## Roadmap / Future Work

- AI rewrite 暂停进入 V19，后续另开独立 spec 再做。
- 后续改写只能选择明确产品模式：手动命令改写、用户明确接受的 post-drain cleanup、或 WebSocket/WebRTC 类双向实时改写。
- 普通 server-output SSE / 一次性 HTTP 请求不能伪装成“一个请求持续吃后续 HUD 文本”；如果要持续接收 HUD 文本，就必须使用真正的双向会话协议。
- 未来任何 AI rewrite 都不能绕过 HUD truth 规则：上屏文本必须来自用户可见的最终 HUD 文本。

## 本轮收口验证

2026-05-11 preview.80 Parakeet safe code-switch:

- 修复：保持 `nvidia/parakeet-ctc-0_6b-zh-cn`、function id `9add5ef7-322e-47e0-ad7a-5653fb8d259b`、`language=zh-CN` 不变。
- 修复：sidecar 默认启用低强度 speech context，`boost=4.0`，词表来自 `sidecars\parakeet_code_switch_terms.json`，只包含当前 Riva 可编码短语。
- 修复：应用侧只精确修复已观察到的 `multi` 丢失、`猫底/某体` 误听、`扣代斯` 等 Codex 误听和英文产品词大小写。
- 保护：新增中文负例，`猫底下有一根线。`、`某体文章写得很好。` 不会被误改。
- 验证：`cargo test -p ainput-rewrite` 20/20 passed；`cargo test -p ainput-desktop online_parakeet -- --nocapture` 1/1 passed；`cargo check -p ainput-desktop` passed。
- 验证：vps-jp `/health` 返回 `model=nvidia/parakeet-ctc-0_6b-zh-cn`、`language=zh-CN`、`boost_enabled=true`、`boost=4.0`、`boost_phrases=22`。
- 验证：debug exe 与 dist exe 都通过 `scripts\run-online-code-switch-replay.ps1`，覆盖 preview.79 原始 WAV 和新增文本回归。
- 验证：`dist\ainput-1.0.0-preview.80\` 与 zip 已生成，包内包含 sidecar、sidecar 词表、data 词表和回放脚本。
- 验证：Windows 交互桌面已运行 `dist\ainput-1.0.0-preview.80\ainput-desktop.exe`，`SessionId=1`；`run-ainput.bat` 与 HKCU Run 均指向 preview.80。
- 未覆盖：真人麦克风中文体感和自由混说仍需要用户实测；如果中文变差，快速回滚到 preview.78。

2026-05-11 preview.78 online Parakeet zh-CN fast release:

- 修复：在线默认回到 `nvidia/parakeet-ctc-0_6b-zh-cn`，不再使用失败的 `multi` RNNT 默认。
- 修复：在线 worker 每个 tick 最多发送 1 个远端 `/chunk`，避免 chunk 队列同步 HTTP 阻塞松手命令。
- 修复：vps-jp adapter `PARTIAL_WAIT_SEC` 默认收紧到 `0.06s`，减少 `/chunk` 等待堆积。
- 修复：speech context boost 只使用 vps-jp Codex 用户指令历史中统计到的高频英文词。
- 修复：中文数字归一化只处理纯中文数字连读；自然中文 `一起 / 一边 / 一下 / 一个` 不再被强行改成阿拉伯数字。
- 验证：见本轮 OPLOG，必须包含 Rust 测试、sidecar py_compile、Windows 打包、vps-jp `/health` 和 live 启动入口回读。

2026-05-11 preview.76 independent online streaming mode:

- 修复：新增独立 `online_streaming` 语音模式和 `[voice.online_streaming]` 配置，默认启动不再占用本地 Qwen 流式配置。
- 修复：托盘一级菜单拆成 `极速语音识别 / 本地流式识别 / 在线流式识别`。
- 修复：本地 `[voice.streaming]` 恢复为 `qwen3_sidecar`、`http://127.0.0.1:8765`、`sidecar_auto_start = true`、`gpu_memory_utilization = 0.30`、`gpu_enabled = true`。
- 修复：在线释放路径在 HUD 已有文本时直接粘贴 HUD snapshot，远端 `/finish` 与 raw capture cleanup 后台执行。
- 验证：`cargo check -p ainput-desktop` 通过；打包和 Windows 交互进程见本轮 OPLOG。

2026-05-11 preview.75 online Parakeet realtime HUD partial:

- 根因确认：`preview.74` 的 online adapter 虽然使用 NVIDIA streaming API 做最终转写，但 `/chunk` 固定返回 `text=""`，所以 HUD 按住期间始终空白，松手 `/finish` 后才一次性出字。
- 修复：`nvidia_parakeet_online_sidecar.py` 在 session 创建时打开同一个 NVIDIA streaming gRPC 会话，`/chunk` 把音频送入队列，并返回当前最新 partial。
- 修复：adapter 开启 `interim_results=True`，维护 final segments + interim text；`/finish` 关闭队列并返回最终文本。
- 兼容：Windows AInput 现有 sidecar HTTP contract 不变，不需要改热键/HUD 主链路。
- 验证：Windows 访问 `/health` 返回 `streaming_partials=true`。
- 验证：已知 WAV 按 240ms chunk、120ms 间隔模拟实时输入，`/finish` 前收到 32 次非空 partial；首个 partial 约 466ms 出现。
- 验证：最终文本仍为 `我现在的问题是所有各个口袋词发过来的消息全部留在1个框里， 所以我翻早起来特别痛苦。`

2026-05-11 preview.74 online NVIDIA Parakeet ASR:

- 新增临时在线 backend：`nvidia_parakeet_online`。
- 默认配置已切到 `voice.mode = "streaming"`、`voice.streaming.backend = "nvidia_parakeet_online"`、`sidecar_url = "http://vps-jp.tail4b5213.ts.net:18765"`、`sidecar_auto_start = false`、`gpu_enabled = false`。
- `vps-jp` adapter `/health` 从 Windows 可访问，返回 Parakeet 模型、16k sample rate、5 个 key。
- 已知 WAV 通过在线 adapter 实测转写，11.38s 音频耗时约 2.15s。
- Windows 真机 `cargo fmt --all -- --check` 已通过。
- Windows 真机 `cargo check -p ainput-desktop` 已通过；仍有既有 dead-code warnings。
- Windows 真机 `cargo test -p ainput-shell render_config_file_contains_streaming_ai_rewrite_section` 已通过。
- Windows 真机打包已通过，产出 `dist\ainput-1.0.0-preview.74\` 与 `dist\ainput-1.0.0-preview.74.zip`。
- 已启动到 Windows 交互桌面：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.74\ainput-desktop.exe`，`SessionId=1`。
- `run-ainput.bat` 与 HKCU Run 自启动均指向 `preview.74`。
- 包内启动日志确认 backend 为 `NVIDIA Parakeet online ASR`，且出现 `local model preload skipped`；未出现本地 Qwen model preload。
- 已停止 `.72` 遗留的 WSL Qwen/vLLM 进程；复查 WSL 中无 qwen/vllm sidecar 进程。

2026-05-11 preview.72 Qwen context echo guard:

- 根因确认：Qwen sidecar 在坏音频/低信号场景下可能把 `[voice.streaming.qwen3].context` 直接作为 partial/final 文本吐出；之前的拦截点太晚，提示词会先进入 HUD 或 fast HUD snapshot。
- 修复：`apply_qwen_sidecar_partial_update` 在写入 `last_display_text`、发送 HUD partial、更新 voice history 或 paste 之前先执行 context echo guard。
- 修复：release final 路径在 final HUD ack 和 paste 前二次阻断 context echo；fast HUD snapshot 也拒绝提交 prompt-like `last_display_text`。
- 保持：不改 Qwen context，不改标点策略，不恢复 offline final，不启用应用层 AI rewrite。
- Windows 真机 `cargo fmt` 已执行。
- Windows 真机 `cargo test -p ainput-desktop qwen_context_echo -- --nocapture` 已通过，3/3 pass。
- Windows 真机 `cargo test -p ainput-desktop worker::tests:: -- --nocapture` 已通过，72/72 pass。
- Windows 真机 `scripts\package-release.ps1 -Version 1.0.0-preview.72` 已通过，产出 `dist\ainput-1.0.0-preview.72\` 与 `dist\ainput-1.0.0-preview.72.zip`。
- 包内配置已确认：`voice.streaming.ai_rewrite.enabled = false`。
- 已启动到 Windows 交互桌面：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.72\ainput-desktop.exe`，`SessionId=1`，PID `37176`。
- Qwen sidecar `/health` 返回 `ok=true`、`model=Qwen/Qwen3-ASR-0.6B`、`idle_unload_ms=3600000`、`effective_enforce_eager=false`。

2026-05-09 preview.54 V20 punctuation + tray version visibility:

- 本轮修正所有默认硬编码标点插入问题，不再因为 `然后 / 现在 / 还是 / 或者 / 比如` 等词强行加逗号，也不再靠 `吗 / 是不是 / 啊 / 呀 / 啦` 这类词表硬猜问号或感叹号。
- 流式 HUD preview 会剥掉未锚定的模型生成 `？/！`；final 只允许已被源文本锚定的问号保留，未锚定尾部 `？/！` 会降级成句号，避免 `这个怎么？回事啊？` 这种错误上屏。
- 本地 rewrite helper 不再给流式文本自动补逗号/句末标点，也移除了项目特定的 `简直就是灾难，标点符号` 强制改写。
- 托盘 tooltip 改为 `ainput 1.0.0-preview.54` 开头；任务栏托盘图标右键菜单第一组新增禁用项 `当前版本：1.0.0-preview.54`，用于直接确认正在运行哪一版。
- Windows 真机验证已通过：`cargo fmt --check`、`cargo test -p ainput-rewrite` 16/16、`cargo test -p ainput-desktop streaming_punctuation` 4/4、`cargo test -p ainput-desktop` 106/106。
- Windows 真机打包已通过，产出 `dist\ainput-1.0.0-preview.54\` 与 `dist\ainput-1.0.0-preview.54.zip`；文件版本为 `1.0.0-preview.54`。
- 已在 Windows console session 3 启动 `dist\ainput-1.0.0-preview.54\ainput-desktop.exe`，PID 89388；打包 exe 中已确认包含 `当前版本：1.0.0-preview.54` 与 `ainput 1.0.0-preview.54` 文案。

2026-05-09 preview.53 V19 HUD truth 单链路：

- 本轮只改流式 V19 主链路；AI rewrite 只进入 Roadmap / Future Work，不作为当前版本功能继续推进。
- 新增并执行 `specs\streaming-hud-truth-single-chain-v19\`：按下 `Ctrl` 立即打开空白 HUD，按住期间持续 streaming ASR，松开后停麦但 HUD 保持可见，drain 完成后粘贴 HUD truth snapshot 一次并关闭 HUD。
- 生产路径不再构建/调用 release-time offline final recognizer；最终提交来自 streaming ASR + `StreamingState`，报告字段固定包含 `hud_paste_equal`、`offline_final_invoked=false`、`ai_rewrite_mutation_count`。
- ASR-facing 音频目标为 mono/16k；优先尝试直接 mono/16k capture，设备不支持时在 ASR 入口前逐块 normalize。
- Windows 真机 `cargo test -p ainput-desktop` 已通过，103/103 pass。
- 用户 preview.51 失败 raw：`dist\ainput-1.0.0-preview.51\logs\streaming-raw-captures\streaming-raw-1778293694109.wav` 已用 V19 链路回放通过；无 `案例来实失败` 重复尾巴，`hud_paste_equal=true`，`offline_final_invoked=false`，`ai_rewrite_mutation_count=0`。
- Windows 真机 raw corpus 已用 `target\debug\ainput-desktop.exe` 和 `dist\ainput-1.0.0-preview.53\ainput-desktop.exe` 各跑通过，`overall_status=pass`，`cases_total=2`。
- `scripts\run-streaming-latency-benchmark.ps1` 已改成 V19 报表口径：统计 `final_commit_ms / hud_paste_equal / offline_final_invoked / ai_rewrite_mutation_count`，并在 offline final 被调用、HUD/paste 不一致或 AI 改写发生 mutation 时 fail。
- Windows 真机打包已通过，产出 `dist\ainput-1.0.0-preview.53\` 与 `dist\ainput-1.0.0-preview.53.zip`。

2026-05-02 preview.50 流式 AI HUD 尾巴改写：

- 本轮只改流式 AI 尾巴改写；非流式 `Alt+Z`、流式 `Ctrl` 按住说话、剪贴板 + `Ctrl+V` 上屏主链路、ASR 模型、标点模型、GPU 设置均未改。
- 新增并执行 `specs\streaming-ai-rewrite-v16\`：按住 `Ctrl` 时 AI 可逐步改写 HUD 当前尾巴；松开 `Ctrl` 立即取消 AI epoch，迟到结果不会更新 HUD 或最终提交。
- 默认端点固定为 vps-jp `cliproxyapi` 8317：`http://vps-jp.tail4b5213.ts.net:8317/v1/chat/completions`；模型固定为 NVIDIA `qwen/qwen3.5-122b-a10b`。
- API key 只从 Windows User 环境变量 `AINPUT_CLIPROXYAPI_8317_KEY` 读取；配置、README、日志和包内 TOML 不写入 key。
- 远端模型实测通过：`target\debug\ainput-desktop.exe test-ai-rewrite "我觉的这个工能不太队"` 返回 `我觉得这个功能不太对`。
- Windows 真机 `cargo fmt`、`cargo fmt --check`、`cargo check -p ainput-desktop`、`cargo test -p ainput-desktop ai_rewrite -- --nocapture`、`cargo test -p ainput-desktop streaming -- --nocapture`、`cargo test -p ainput-desktop hotkey -- --nocapture`、`cargo test -p ainput-rewrite -- --nocapture`、`cargo test -p ainput-shell streaming_ai_rewrite -- --nocapture` 均已通过。
- Windows 真机 `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1` 已通过，产出 `dist\ainput-1.0.0-preview.50\` 与 `dist\ainput-1.0.0-preview.50.zip`。
- Windows 真机 `.\scripts\run-streaming-full-audit.ps1 -Version 1.0.0-preview.50 -LatencyRepeats 1 -LiveCaseLimit 3` 已通过：`overall_status=pass`，P0=0、P1=0、P2=0，报告：`tmp\streaming-full-audit\20260502-013018-999\full-audit-report.json`。
- 包内配置已确认：`voice.streaming.ai_rewrite.enabled = true`，endpoint/model/api_key_env 指向 8317/qwen/env name；文本制品扫描未发现 key 值。
- 已启动到 Windows 交互桌面：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.50\ainput-desktop.exe`，PID `64444`。

2026-05-02 preview.49 流式 offline final 乱码尾巴修复：

- 本轮只改流式松手 final 选择；非流式 `Alt+Z`、流式 `Ctrl` 按住说话、剪贴板 + `Ctrl+V` 上屏主链路、AI 语义改写、GPU 均未改。
- 用户复现：HUD 为 `还是得稳住慎重`，松手上屏却变成 `还是得稳住慎重We住慎重ong。`。
- 根因：live 日志显示污染已经出现在 final commit envelope 阶段，`last_hud_target_text` 正确，但 `final_offline_raw_text / final_candidate_text / resolved_commit_text` 选中了带 ASCII 噪声的 offline final 尾巴。
- 新增 `specs\streaming-offline-final-garbled-tail-v15\`，把“HUD 正确但 final offline 尾巴污染”列为 P0 类回归。
- 修复点：`select_streaming_final_raw_text` 先拒绝“英文噪声包着 HUD 已显示中文尾巴”的 offline raw；`select_streaming_commit_text` 再拒绝已经拼成 `HUD + garbled duplicate tail` 的 candidate display。
- 回归覆盖：`We住慎重ong。` 不再污染最终提交；`这个功能支持 Windows 版本。` 这类真实中英混合新增尾巴不会被误杀。
- Windows 真机 `cargo fmt --check`、`cargo check -p ainput-desktop`、`cargo test -p ainput-desktop garbled -- --nocapture`、`cargo test -p ainput-desktop streaming -- --nocapture`、`cargo test -p ainput-desktop hotkey -- --nocapture`、`cargo test -p ainput-rewrite -- --nocapture` 均已通过。
- Windows 真机 `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1` 已通过，产出 `dist\ainput-1.0.0-preview.49\` 与 `dist\ainput-1.0.0-preview.49.zip`。
- Windows 真机 `.\scripts\run-streaming-full-audit.ps1 -Version 1.0.0-preview.49 -LatencyRepeats 1 -LiveCaseLimit 3` 已在干净进程环境通过：`overall_status=pass`，P0=0、P1=0、P2=0，报告：`tmp\streaming-full-audit\20260502-000910-084\full-audit-report.json`。
- 已启动到 Windows 交互桌面：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.49\ainput-desktop.exe`，PID `20096`。

2026-05-01 preview.48 流式首字测速口径修正：

- 本轮只改流式测速与验收口径；非流式 `Alt+Z`、流式 `Ctrl` 按住说话、剪贴板 + `Ctrl+V` 上屏主链路、AI 语义改写、GPU 均未改。
- 新增 `specs\streaming-first-partial-latency-v14\`，把速度指标从单纯 `audio_start -> first_partial` 修正为优先看 `speech_start -> first_partial`，避免真实 raw 样本开头静音把 HUD 速度误判为慢。
- 小模型 `streaming-zipformer-small-bilingual-zh-en` 已实测拒绝：墙钟更快但 5 条里 3 条失败，不能切默认模型。
- 新增 replay 报告字段：`speech_start_ms`、`first_partial_after_speech_ms`、`first_partial_processing_elapsed_ms`、`first_partial_processing_lag_ms`；latency benchmark 和 full audit 已同步读取新字段。
- Windows 真机 `cargo check -p ainput-desktop`、`cargo test -p ainput-desktop streaming -- --nocapture`、`cargo test -p ainput-desktop hotkey -- --nocapture`、`cargo test -p ainput-rewrite -- --nocapture` 均已通过。
- Windows 真机 `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1 -Version 1.0.0-preview.48` 已通过，产出 `dist\ainput-1.0.0-preview.48\` 与 `dist\ainput-1.0.0-preview.48.zip`。
- Windows 真机 `.\scripts\run-streaming-full-audit.ps1 -Version 1.0.0-preview.48 -LatencyRepeats 1 -LiveCaseLimit 3` 已通过：`overall_status=pass`，P0=0、P1=0、P2=0，报告：`tmp\streaming-full-audit\20260501-223721-227\full-audit-report.json`。
- `preview.48` 全量门禁通过项：包完整性、`cargo fmt --check`、`cargo check -p ainput-desktop`、hotkey 单测、streaming 单测、rewrite 单测、v12 replay、startup idle、streaming selftest、raw corpus、synthetic live E2E、wav live E2E、latency benchmark。
- v14 最快安全结果仍是当前 Paraformer 双语模型：`paraformer_bilingual_asr6_chunk80` 在 5 条样本中 `failed=0/5`，`speech->first avg=588ms / p50=540ms / p95=900ms`；默认 `asr6/chunk60` 为 `avg=600ms / p50=540ms / p95=920ms`，差异很小，本轮不改默认配置。
- 已启动到 Windows 交互桌面：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.48\ainput-desktop.exe`，PID `38548`。

2026-05-01 preview.46 流式全量 bug 审计与启动误触发修复：

- 本轮只修流式输出基础链路；非流式 `Alt+Z`、流式 `Ctrl` 按住说话、剪贴板 + `Ctrl+V` 主链路均未改；AI 语义改写和 GPU 仍不启用。
- 新增 `scripts\run-streaming-full-audit.ps1` 与 `specs\streaming-full-bug-audit-v13\`，把包完整性、hotkey、startup idle、v12 replay、selftest、raw corpus、synthetic/wav live E2E、latency benchmark 收成一条总控门禁。
- 有效基线审计 `1.0.0-preview.45` 发现 P0：启动空闲期间会被残留的 modifier-only 立即触发分支误送 `VoicePressed`，产生短 raw capture；这对应“没按也自己识别一次”的风险。
- 修复点：删除单独 `Ctrl` 热键的旧通用立即触发分支，只保留“延迟判定 / 组合键取消 / 不吞 Ctrl”的专门路径；新增 hotkey 单测 `modifier_only_ctrl_triggers_only_after_delay`。
- Windows 真机 `cargo test -p ainput-desktop hotkey -- --nocapture` 已通过，7/7 pass。
- Windows 真机 `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1` 已通过，产出 `dist\ainput-1.0.0-preview.46\` 与 `dist\ainput-1.0.0-preview.46.zip`。
- Windows 真机 `.\scripts\run-streaming-full-audit.ps1 -Version 1.0.0-preview.46 -LatencyRepeats 1 -LiveCaseLimit 3` 已通过为 `pass_with_p2`：P0=0、P1=0、P2=1。
- `preview.46` 全量门禁通过项：`cargo fmt --check`、`cargo check -p ainput-desktop`、`cargo test hotkey`、`cargo test streaming`、`cargo test ainput-rewrite`、v12 replay、startup idle、streaming selftest、raw corpus、synthetic live E2E、wav live E2E、latency benchmark。
- `preview.46` startup idle 30 秒已通过，确认不会在空闲启动时自触发录音。
- `preview.46` raw/synthetic/wav live 已通过，未复现重复上屏、HUD/上屏不一致、明显漏尾字、ghost `I/yeah`。
- 速度残留 P2：当前基线 `paraformer_bilingual_asr6_chunk60` 首个 partial `p50=660ms / p95=1860ms / avg=1020ms`；最快安全候选 `chunk80` 仅到 `avg=1008ms / p95=1840ms`，收益太小，本轮不改配置。下一轮要真正提“说话到 HUD”的速度，应优先研究模型/partial emission，而不是 CPU 线程或标点。
- 已启动到 Windows 交互桌面：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.46\ainput-desktop.exe`，收口时已验证该路径进程在运行。

2026-05-01 preview.45 流式尾字、孤立 `I` 与标点词组修复：

- 本轮只改流式输出链路；非流式 `Alt+Z`、流式 `Ctrl` 按住说话、剪贴板 + `Ctrl+V` 主链路均未改。
- final 选择先保留 `preview.43` 的 HUD 最终真相源，再在 `display/candidate` 二选一之后统一做最终清洗，避免 HUD/上屏尾部残留 `I`、`标点，符号`、`强治/强距`。
- offline final hard budget 从 `350ms` 放宽到 `650ms`，避免可用尾字修复结果刚好晚到时被丢弃；固定样本 `short-tail-da.wav` 已覆盖“不是很大”的“大”。
- tail-window offline final 如果只识别出短英文幻觉 `I/Yeah/Okay/...`，中文上下文下会拒绝拼接；若显示尾部是“...不”且离线尾巴是 `I`，本地修成“对”。
- `crates\ainput-rewrite` 增加本轮已观察误识别清洗：`强治/强距 -> 简直`、`标点，符号 -> 标点符号`、中文尾部孤立 `I` 删除、`不I -> 不对`。
- replay 报告的 `final_text` 已对齐真实最终 commit 文本，先补完终止句号再进入内容/质量门禁，避免报告和 HUD/上屏口径分叉。
- 新增固定回归资产：`fixtures\streaming-user-regression-v12\`，覆盖短句尾字、重复 `I`、短句完整尾字、标点词组 + `不I`。
- Windows 真机 `cargo fmt --check` 已通过。
- Windows 真机 `cargo check -p ainput-desktop` 已通过。
- Windows 真机 `cargo test -p ainput-rewrite -- --nocapture` 已通过，16/16 pass。
- Windows 真机 `cargo test -p ainput-desktop final_commit -- --nocapture` 已通过，5/5 pass。
- Windows 真机 `cargo test -p ainput-desktop streaming -- --nocapture` 已通过，32/32 pass。
- Windows 真机 `cargo test -p ainput-desktop hotkey -- --nocapture` 已通过，6/6 pass，覆盖 `Ctrl+A/C/V` 不应被单独 `Ctrl` 热键吞掉。
- 包内 `dist\ainput-1.0.0-preview.45\ainput-desktop.exe replay-streaming-manifest fixtures\streaming-user-regression-v12\manifest.json` 已通过，4/4 pass：
  - `我试了短句的话，问题好像不是很大。`
  - `很奇怪还是会漏字和重复。`
  - `我刚想说短句的话，问题不是很大结果又给我漏了最后那个字。`
  - `简直就是灾难，标点符号都不对。`
- Windows 真机 `.\scripts\run-startup-idle-acceptance.ps1 -Version 1.0.0-preview.45 -IdleSeconds 30 -Runs 1 -InteractiveTask` 已通过，启动空闲不自触发录音。
- Windows 真机 `.\scripts\run-streaming-live-e2e.ps1 -Version 1.0.0-preview.45 -Synthetic -InteractiveTask` 已通过，3/3 pass，HUD flash/panel 全 0。
- Windows 真机 `.\scripts\run-streaming-live-e2e.ps1 -Version 1.0.0-preview.45 -Wav -InteractiveTask -CaseLimit 3` 已通过，3/3 pass，HUD flash/panel 全 0。
- Windows 真机 `.\scripts\run-streaming-raw-corpus.ps1 -ExePath .\dist\ainput-1.0.0-preview.45\ainput-desktop.exe -RawDir .\dist\ainput-1.0.0-preview.43\logs\streaming-raw-captures -ShortCount 1 -LongCount 1` 已通过，2/2 pass，`final_missing_chars=0`。
- `preview.44` 因打包脚本未先创建 `fixtures\streaming-user-regression-v12` 目录被废弃；`preview.45` 已修复打包目录并作为当前可测版本。
- 已启动到 Windows 交互桌面：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.45\ainput-desktop.exe`，PID `59416`。

2026-05-01 HUD 最终真相源收口：

- 流式松开 `Ctrl` 后，worker 不再直接用内部 `commit_text` 上屏；最终文本必须先发给 UI，HUD 完整显示并返回 ack 后，才允许提交。
- UI 收到 final 文本后立即完整刷新 HUD，不走逐字动画；worker 只用 HUD ack text 上屏。
- 流式 HUD ack 后进入 exact delivery，`ainput-output` 不再二次补标点、删标点或做术语修正，避免 HUD 与实际上屏分叉。
- Windows 真机 `cargo fmt --check` 已通过。
- Windows 真机 `cargo check -p ainput-desktop` 已通过。
- Windows 真机 `cargo test -p ainput-desktop final_commit -- --nocapture` 已通过，4/4 pass。
- Windows 真机 `cargo test -p ainput-desktop streaming -- --nocapture` 已通过，31/31 pass。
- Windows 真机 `cargo test -p ainput-desktop -- --nocapture` 已通过，86/86 pass。
- Windows 真机 `cargo test -p ainput-output -- --nocapture` 已通过，9/9 pass。
- Windows 真机 `cargo test -p ainput-shell -- --nocapture` 已通过，6/6 pass。
- Windows 真机 `cargo test -p ainput-rewrite -- --nocapture` 已通过，16/16 pass。
- Windows 真机 `.\scripts\run-startup-idle-acceptance.ps1 -Version 1.0.0-preview.43 -IdleSeconds 30 -Runs 1 -InteractiveTask` 已通过。
- Windows 真机 `.\scripts\run-streaming-live-e2e.ps1 -Version 1.0.0-preview.43 -Synthetic -InteractiveTask` 已通过，3/3 pass。
- Windows 真机 `.\scripts\run-streaming-live-e2e.ps1 -Version 1.0.0-preview.43 -Wav -InteractiveTask -CaseLimit 3` 已通过，3/3 pass。
- Windows 真机 `.\scripts\run-streaming-raw-corpus.ps1 -ExePath .\dist\ainput-1.0.0-preview.43\ainput-desktop.exe -RawDir .\dist\ainput-1.0.0-preview.37\logs\streaming-raw-captures -ShortCount 1 -LongCount 1` 已通过，2/2 pass。
- wav E2E `sentence_03` 已确认 `hud_final_ack == output_commit_request == target_readback`：`然后，不管我说多少个字，它永远只能显示出来两个字。`
- 已打包 `dist\ainput-1.0.0-preview.43\` 与 `dist\ainput-1.0.0-preview.43.zip`。
- 已启动到 Windows 交互桌面：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.43\ainput-desktop.exe`。

2026-04-29 V3 源码收口：

- Windows 真机 `cargo fmt --check -- apps/ainput-desktop/src/streaming_state.rs apps/ainput-desktop/src/worker.rs apps/ainput-desktop/src/overlay.rs apps/ainput-desktop/src/main.rs apps/ainput-desktop/src/ai_rewrite.rs apps/ainput-desktop/src/streaming_fixtures.rs crates/ainput-shell/src/lib.rs crates/ainput-output/src/lib.rs` 已通过
- Windows 真机 `cargo check -p ainput-desktop` 已通过
- Windows 真机 `cargo test -p ainput-desktop streaming` 已通过
- Windows 真机 `cargo test -p ainput-output` 已通过
- Windows 真机 `cargo test -p ainput-rewrite` 已通过
- Windows 真机 `cargo test -p ainput-shell` 已通过
- Windows 真机 `cargo test -p ainput-desktop ai_rewrite` 已通过
- Windows 真机 `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-selftest.ps1` 已通过，6/6 cases pass，keywords gate 100%
- Windows 真机 `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1` 已通过，产出 `dist\ainput-1.0.0-preview.24\` 与 `dist\ainput-1.0.0-preview.24.zip`
- Windows 真机 `dist\ainput-1.0.0-preview.24\ainput-desktop.exe bootstrap` 已通过
- Windows 真机包内 release exe 跑 `replay-streaming-manifest fixtures\streaming-selftest\manifest.json` 已通过，进程退出码 `0`
- 未覆盖：`scripts\live-streaming-acceptance.ps1` 是人工按住热键说话验收，不能在 SSH 无人值守环境中伪造通过

2026-04-30 V3 前台失败修复：

- 根据真实前台日志修复 HUD 与最终上屏分叉：录音中的 HUD 不再经过离线标点模型，避免最新尾巴被标点模型裁短
- 默认关闭 `[voice.streaming.endpoint].enabled`，避免 `480ms` 级短停顿把半句话冻结成已提交前缀
- 松手尾音收集从 `120ms` 加到 `260ms`，最终解码静音 padding 从 `240ms` 加到 `360ms`，降低最后一个字被截掉的概率
- 增加常见流式误识别归一化：`hot/hud -> HUD`、`证确 -> 正确`、`土字 -> 吐字`
- Windows 真机 `cargo check -p ainput-desktop` 已通过
- Windows 真机 `cargo test -p ainput-desktop streaming` 已通过
- Windows 真机 `cargo test -p ainput-desktop worker::tests` 已通过
- Windows 真机 `cargo test -p ainput-rewrite` 已通过
- Windows 真机 `cargo test -p ainput-shell` 已通过
- Windows 真机 `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-selftest.ps1` 已通过，6/6 cases pass，keywords gate 100%

2026-04-30 streaming live E2E 自测闭环：

- 新增 `scripts\run-streaming-live-e2e.ps1`，支持 `-InteractiveTask` 把测试进程投到当前 Windows console session，避免 SSH 非交互会话里键盘事件无法进入桌面的假失败
- 新增 `run-streaming-live-e2e-synthetic` 命令、`fixtures\streaming-hud-e2e\manifest.json` 和专用 Win32 测试输入框
- live E2E 报告会写入 `tmp\streaming-live-e2e\<timestamp>\report.json` 与 `timeline.jsonl`
- 每个 case 会校验 `HUD final display == output commit text == target readback`
- 每个 case 还会校验 HUD 稳定性：流式期间位置/尺寸变化超过 `3px` 直接失败，alpha 下降或不可见采样直接失败
- 流式提交在原生 `EDIT/RichEdit` 输入框优先走 Windows `EM_REPLACESEL`，非流式模式保持原来的剪贴板恢复路径
- acceptance 目标框在清空后会再次聚焦，并记录 `focused_hwnd` / `edit_is_focused`，避免焦点丢失造成假通过或假失败
- Windows 真机 `cargo check -p ainput-desktop` 已通过
- Windows 真机 `cargo test -p ainput-desktop acceptance` 已通过
- Windows 真机 `cargo test -p ainput-desktop streaming` 已通过
- Windows 真机 `cargo test -p ainput-output` 已通过
- Windows 真机 `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-selftest.ps1` 已通过，6/6 cases pass
- Windows 真机 `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1` 已通过，产出 `dist\ainput-1.0.0-preview.24\` 与 `dist\ainput-1.0.0-preview.24.zip`
- 源码态 `.\scripts\run-streaming-live-e2e.ps1 -Synthetic -InteractiveTask` 已通过，3/3 cases pass，报告：`tmp\streaming-live-e2e\20260430-101731-345`
- 源码态 `.\scripts\run-streaming-live-e2e.ps1 -Wav -InteractiveTask` 已通过，6/6 cases pass，报告：`tmp\streaming-live-e2e\20260430-101743-605`
- 包内 `dist\ainput-1.0.0-preview.24\scripts\run-streaming-live-e2e.ps1 -Synthetic -InteractiveTask` 已通过，3/3 cases pass，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-102339-342`
- 包内 `dist\ainput-1.0.0-preview.24\scripts\run-streaming-live-e2e.ps1 -Wav -InteractiveTask` 已通过，6/6 cases pass，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-102352-551`

2026-04-30 HUD 单行黑色胶囊动态面板：

- 流式 HUD 默认改为黑色半透明胶囊背景、白色文字、居中显示，替代原来的白色大面板
- HUD 按当前显示文字的实际宽度动态调整：短文本是一小块底板，文字增加时从任务栏上方中心向两边延长
- 流式和 final 都走同一套单行尺寸计算，不再使用固定宽度、多行高度的 streaming 大面板
- 文本测量改为单行 `DT_SINGLELINE`，不自动换行；超长文本只受屏幕安全宽度限制
- `config\hud-overlay.toml` 默认值改为：`width_px = 1600`，`min_width_px = 52`，`min_height_px = 50`，`padding_x_px = 14`，`padding_y_px = 8`，`text_align = "center"`，`background_color = "#0B0B0B"`，`background_alpha = 190`
- live E2E 稳定性门禁改为检查中心点稳定：允许宽度随字数增长，但 `max_center_x_delta_px`、top、height 不能异常漂移
- live E2E 新增视觉门禁：`hud_white_panel` 防白色面板，`hud_multiline_panel` 防回到多行大面板，`hud_short_text_wide_panel` 防短文本仍显示大面板
- 打包脚本不再从旧 dist 保留 HUD 的白色尺寸/颜色/对齐配置；新包强制使用黑色单行胶囊默认样式，同时继续保留字体和显示保留时间
- Windows 真机 `cargo check -p ainput-desktop` 已通过
- Windows 真机 `cargo test -p ainput-desktop acceptance` 已通过，1/1 pass
- Windows 真机 `cargo test -p ainput-shell` 已通过，6/6 pass
- Windows 真机 `cargo test -p ainput-desktop streaming` 已通过，24/24 pass
- Windows 真机 `cargo test -p ainput-output` 已通过，9/9 pass
- Windows 真机 `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-selftest.ps1` 已通过，6/6 cases pass
- 源码态 `.\scripts\run-streaming-live-e2e.ps1 -Synthetic -InteractiveTask` 已通过，3/3 cases pass，报告：`tmp\streaming-live-e2e\20260430-140234-142`，`hud_center` 最大 `0/0`，`hud_panel` 全 `0/0/0`
- 源码态 `.\scripts\run-streaming-live-e2e.ps1 -Wav -InteractiveTask` 已通过，6/6 cases pass，报告：`tmp\streaming-live-e2e\20260430-140252-617`，`hud_center` 最大 `1/0`，`hud_panel` 全 `0/0/0`
- 包内 `dist\ainput-1.0.0-preview.24\scripts\run-streaming-live-e2e.ps1 -Synthetic -InteractiveTask` 已通过，3/3 cases pass，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-140634-134`，`hud_center` 最大 `0/0`，`hud_panel` 全 `0/0/0`
- 包内 `dist\ainput-1.0.0-preview.24\scripts\run-streaming-live-e2e.ps1 -Wav -InteractiveTask` 已通过，6/6 cases pass，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-140652-187`，`hud_center` 最大 `1/0`，`hud_panel` 全 `0/0/0`
- `dist\ainput-1.0.0-preview.24.zip` 已重建，大小 `456341974` bytes，时间 `2026-04-30 14:06:21`

2026-04-30 Exactly-once 上屏与自触发保护：

- 修复流式按住说话热键为 `Ctrl` 时，最终上屏阶段程序自身 `Ctrl+V` 有概率被全局热键钩子误判成下一轮语音的问题
- 流式最终提交和非流式输出共享保护：程序输出期间临时屏蔽语音热键识别，避免一次 release 后出现两次上屏
- DirectPaste / NativeEdit 上屏前清理中文 IME composition，避免残留拼音如 `wan`、`ngl`、`us`、`gxi` 被提交到目标框
- 流式 DirectPaste 前稳定等待提升到 `120ms`；Win32/RichEdit 目标优先走 `NativeEdit`，非原生目标再回退 DirectPaste
- live E2E 新增提交后 `1500ms` 观察窗口，目标框如果出现 `final+final` 或 `final+错误片段` 会失败为 `target_duplicate_commit` / `target_extra_commit_fragment`
- live E2E 新增 `output_commit_count_mismatch`，任意 case 的 `commit_request_count != 1` 直接失败，防止一次 release 产生两次上屏请求
- live E2E 执行前会停掉旧 `ainput-desktop.exe` 托盘进程并复查残留；源码态 E2E 会先 build 最新 debug exe，避免旧二进制污染测试
- Windows 真机 `cargo check -p ainput-desktop` 已通过
- Windows 真机 `cargo test -p ainput-desktop hotkey` 已通过，4/4 pass
- Windows 真机 `cargo test -p ainput-desktop acceptance` 已通过，5/5 pass
- Windows 真机 `cargo test -p ainput-desktop streaming` 已通过，30/30 pass
- Windows 真机 `cargo test -p ainput-output` / `cargo test -p ainput-shell` / `cargo test -p ainput-rewrite` 均已通过
- Windows 真机 `scripts\run-streaming-selftest.ps1` 已通过，6/6 cases pass
- 源码态 synthetic live E2E 已通过，报告：`tmp\streaming-live-e2e\20260430-175340-791`，3/3 pass，`bad_commit_count=0`，`bad_readback=0`
- 源码态 wav live E2E 连续通过，报告：`tmp\streaming-live-e2e\20260430-175123-400`、`tmp\streaming-live-e2e\20260430-175228-953`，均 6/6 pass，`bad_commit_count=0`，`bad_readback=0`
- 包内 synthetic live E2E 已通过，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-175630-487`，3/3 pass，`bad_commit_count=0`，`bad_readback=0`
- 包内 wav live E2E 连续通过，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-175703-183`、`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-175813-905`，均 6/6 pass，`bad_commit_count=0`，`bad_readback=0`
- `dist\ainput-1.0.0-preview.24.zip` 已重建，大小 `456367554` bytes，时间 `2026-04-30 17:55:57`
- raw corpus 本轮未覆盖：当前 `logs\streaming-raw-captures` 没有足够大的 raw wav；本轮问题属于输出 exactly-once，不依赖 raw ASR 回放

2026-04-30 启动空闲误触发修复：

- 问题根因：流式模式曾把配置里的 `hotkeys.voice_input` 覆盖成单独 `Ctrl`，用户没有按 `Alt+Z` 也可能因普通 Ctrl 操作触发语音识别
- 修复方向：流式和非流式都使用同一个配置热键；默认仍是 `Alt+Z`
- 热键 hook 启动时会 reset 状态并加启动冷却，避免启动瞬间残留按键状态触发录音
- 语音热键触发日志会标注来源：keyboard primary、modifier-only 或 mouse middle
- 新增 `scripts\run-startup-idle-acceptance.ps1`，用于验证启动后不按热键时不会录音、不会上屏、不会产生 raw capture
- Windows 真机 `cargo check -p ainput-desktop` 已通过
- Windows 真机 `cargo test -p ainput-desktop hotkey` 已通过，4/4 pass
- Windows 真机 `cargo test -p ainput-desktop acceptance` 已通过，5/5 pass
- Windows 真机 `cargo test -p ainput-desktop streaming` 已通过，30/30 pass
- Windows 真机 `cargo test -p ainput-output` / `cargo test -p ainput-shell` / `cargo test -p ainput-rewrite` 已通过
- Windows 真机 `scripts\run-streaming-selftest.ps1` 已通过，6/6 cases pass
- 源码态 startup idle 已通过，报告：`tmp\startup-idle-acceptance\20260430-195103-794`，2/2 pass，`expected_voice_hotkey=Alt+Z`
- 包内 startup idle 已通过，报告：`dist\ainput-1.0.0-preview.24\tmp\startup-idle-acceptance\20260430-195614-253`，3/3 pass，`expected_voice_hotkey=Alt+Z`
- 包内 synthetic live E2E 已通过，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-195810-447`，3/3 pass
- 包内 wav live E2E 已通过，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-195830-871`，6/6 pass
- `dist\ainput-1.0.0-preview.24.zip` 已重建；包内 `scripts\run-startup-idle-acceptance.ps1` 可直接运行

2026-04-30 语义标点与尾字保护：

- 本轮不接入 AI rewrite；`voice.streaming.ai_rewrite.enabled = false` 继续保持关闭
- 停顿 endpoint 不再把 pause 当句末：停顿只 flush 尾音和 reset streaming recognizer，不再调用标点模型强制补 `。！？；`
- endpoint rollover 不再整段 `freeze_with_committed_text`；只冻结已经存在明确句末标点的前缀，没结束的 live tail 继续可改
- preview/final 标点统一去重，新增 raw/live 门禁拦截 `，，`、`,,`、`。。`、`？？`、`？！`、`，。` 等重复或冲突标点
- final 提交新增非流式 SenseVoice 兜底校对：当 streaming final 少尾字而非流式结果是同前缀更长文本时，用非流式结果补尾巴
- 实时轻量语义逗号新增 `另外/然后/还是/尤其是/或者/比如` 等连接词处理，只插逗号，不靠停顿插句号
- `scripts\run-streaming-raw-corpus.ps1` 新增 `raw_final_tail_dropped`、`raw_duplicate_punctuation`、`raw_punctuation_forced_by_pause` 门禁，并跳过太短无法产生 partial 的 raw
- Windows 真机 `cargo check -p ainput-desktop` 已通过
- Windows 真机 `cargo test -p ainput-desktop streaming` 已通过，30/30 pass
- Windows 真机 `cargo test -p ainput-desktop acceptance` 已通过，1/1 pass
- Windows 真机 `cargo test -p ainput-output` 已通过，9/9 pass
- Windows 真机 `cargo test -p ainput-shell` 已通过，6/6 pass
- Windows 真机 `cargo test -p ainput-rewrite` 已通过，16/16 pass
- Windows 真机 `scripts\run-streaming-selftest.ps1` 已通过，6/6 cases pass
- 源码态 raw corpus 抽样已通过，报告：`tmp\streaming-raw-corpus\20260430-145208-844`，4 条 pass，`final_missing_chars=0`，无重复标点/无 pause 强插句号
- 源码态 synthetic live E2E 已通过，报告：`tmp\streaming-live-e2e\20260430-145346-906`，3/3 pass，HUD flash/panel 全 0
- 源码态 wav live E2E 已通过，报告：`tmp\streaming-live-e2e\20260430-145402-148`，6/6 pass，HUD flash/panel 全 0
- 包内 raw corpus 抽样已通过，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-raw-corpus\20260430-150531-347`，当前包内有效 raw 样本 1 条 pass
- 包内 synthetic live E2E 已通过，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-150531-487`，3/3 pass
- 包内 wav live E2E 已通过，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-150548-099`，6/6 pass
- `dist\ainput-1.0.0-preview.24.zip` 已重建，大小 `456359936` bytes，时间 `2026-04-30 15:05:14`

2026-04-30 按住停顿补尾字与实时标点：

- 本轮不接入 AI rewrite；`voice.streaming.ai_rewrite.enabled = false` 保持关闭，基础流式验收不混入语义改写
- `[voice.streaming.endpoint]` 默认启用，`pause_ms = 720`，`min_segment_ms = 900`，`tail_padding_ms = 480`
- 按住不松但检测到停顿时，应用层 endpoint 会 soft finalize 当前片段：补短静音、调用 streaming `input_finished()`、刷新 HUD/稳定前缀，然后 reset recognizer 继续听；不会提前上屏
- 实时 preview 和停顿边界都会复用常驻标点模型；标点结果如果导致内容字减少会被拒绝，避免标点模型裁掉尾字
- 新增 `scripts\run-streaming-raw-corpus.ps1`，默认从最近 raw captures 中抽短句和长句各 2 条，不需要每轮跑满 20 条
- raw corpus 门禁会检查至少覆盖短句+长句、最后一个 HUD partial 与 final 的内容字差距不超过 1、长语音 final 带标点时 partial 阶段也必须已经有标点
- 打包脚本会把 `scripts\run-streaming-raw-corpus.ps1` 一并放入 dist，包内也能直接跑 raw 抽样验收
- 打包脚本会在重建 dist 前暂存 `logs\streaming-raw-captures\`，zip 完成后再恢复到 dist 目录，避免后续重打包清掉本地近 20 条 raw 留样；本轮旧 dist 中的 20 条样本已被重打包清空，当前只能后续重新积累
- Windows 真机 `cargo check -p ainput-desktop` 已通过
- Windows 真机 `cargo test -p ainput-desktop streaming` 已通过，24/24 pass
- Windows 真机 `cargo test -p ainput-output` 已通过，9/9 pass
- Windows 真机 `cargo test -p ainput-desktop acceptance` 已通过，1/1 pass
- Windows 真机 `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-selftest.ps1` 已通过，6/6 cases pass
- 源码态 raw corpus 抽样已通过，报告：`tmp\streaming-raw-corpus\20260430-121618-781`，4 条 pass，短句+长句覆盖，`final_extra_chars=0`，partial/final 均有标点
- 源码态 `.\scripts\run-streaming-live-e2e.ps1 -Synthetic -InteractiveTask` 已通过，3/3 cases pass，HUD move/size/flash 全为 0，报告：`tmp\streaming-live-e2e\20260430-121740-655`
- 源码态 `.\scripts\run-streaming-live-e2e.ps1 -Wav -InteractiveTask` 已通过，6/6 cases pass，HUD move/size/flash 全为 0，报告：`tmp\streaming-live-e2e\20260430-121756-340`
- 包内 raw corpus fixture 抽样已通过，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-raw-corpus\20260430-123441-839`，4 条 pass，短句+长句覆盖，`final_extra_chars=0`，partial/final 均有标点
- 包内 `dist\ainput-1.0.0-preview.24\scripts\run-streaming-live-e2e.ps1 -Synthetic -InteractiveTask` 已通过，3/3 cases pass，HUD move/size/flash 全为 0，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-123514-026`
- 包内 `dist\ainput-1.0.0-preview.24\scripts\run-streaming-live-e2e.ps1 -Wav -InteractiveTask` 已通过，6/6 cases pass，HUD move/size/flash 全为 0，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-123527-953`
- `dist\ainput-1.0.0-preview.24.zip` 已重建，大小 `456340280` bytes，时间 `2026-04-30 12:34:14`

2026-04-30 流式松手提交、HUD 残影与原始录音留样修复：

- 修复流式 Ctrl+V fallback 在粘贴前恢复旧剪贴板的问题；流式 fallback 现在写入识别文本后不提前恢复旧剪贴板，避免松手贴出其他剪贴板内容
- 流式提交前会等待语音热键修饰键释放，减少 Ctrl 仍按住时发 Ctrl+V 的顺序问题
- 松手收尾改为语音活动驱动 drain：最小等待 `160ms`，检测尾音静稳，最长 `900ms`，再做 final decode 和最终 HUD flush
- 最终解码静音 padding 增加到 `720ms`，避免最后一个字因为松手瞬间中断被吃掉
- `StreamingStarted` 会清空 HUD target/display/message/window text；live E2E 增加 `hud_after_case_reset`，新一句开头残留上一句会失败为 `hud_stale_text`
- HUD final flush 和提交后的完成态都保持流式稳定尺寸；live E2E 增加 `hud_after_commit_hold`，提交后 HUD 不可见或不等于 final text 会失败
- live E2E 每次 commit 前写入旧剪贴板哨兵，目标框读到哨兵会失败为 `clipboard_stale_paste`
- `probe-streaming-live` 和真实流式热键路径都会写入 `logs\streaming-raw-captures\`，每次保存 wav + json，自动只保留最近 `20` 组
- Windows 真机 `cargo test -p ainput-output` 已通过
- Windows 真机 `cargo test -p ainput-desktop acceptance` 已通过
- Windows 真机 `cargo test -p ainput-desktop streaming` 已通过
- Windows 真机 `cargo test -p ainput-desktop worker::tests::raw_capture_writer_keeps_only_recent_twenty_wavs` 已通过
- Windows 真机 `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-streaming-selftest.ps1` 已通过，6/6 cases pass
- 源码态 `.\scripts\run-streaming-live-e2e.ps1 -Synthetic -InteractiveTask` 已通过，3/3 cases pass，报告：`tmp\streaming-live-e2e\20260430-110516-575`
- 源码态 `.\scripts\run-streaming-live-e2e.ps1 -Wav -InteractiveTask` 已通过，6/6 cases pass，报告：`tmp\streaming-live-e2e\20260430-110528-184`
- 源码态 `.\target\debug\ainput-desktop.exe probe-streaming-live 1` 已写入：`logs\streaming-raw-captures\streaming-raw-1777514764552.wav` + `.json`
- Windows 真机 `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\package-release.ps1` 已通过，产出 `dist\ainput-1.0.0-preview.24\` 与 `dist\ainput-1.0.0-preview.24.zip`
- 包内 `dist\ainput-1.0.0-preview.24\scripts\run-streaming-live-e2e.ps1 -Synthetic -InteractiveTask` 已通过，3/3 cases pass，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-111750-538`
- 包内 `dist\ainput-1.0.0-preview.24\scripts\run-streaming-live-e2e.ps1 -Wav -InteractiveTask` 已通过，6/6 cases pass，报告：`dist\ainput-1.0.0-preview.24\tmp\streaming-live-e2e\20260430-111805-306`
- 包内 `dist\ainput-1.0.0-preview.24\ainput-desktop.exe probe-streaming-live 1` 已写入：`dist\ainput-1.0.0-preview.24\logs\streaming-raw-captures\streaming-raw-1777515538735.wav` + `.json`
- `dist\ainput-1.0.0-preview.24.zip` 已重建，大小 `456334310` bytes，时间 `2026-04-30 11:17:34`

2026-04-21 preview.24 打包收口：

- Windows 真机 `cargo fmt --all` 已通过
- Windows 真机 `cargo check -p ainput-desktop` 已通过
- Windows 真机 `cargo test -p ainput-desktop streaming_` 已通过
- Windows 真机 `cargo test -p ainput-shell streaming_ai_rewrite_` 已通过
- Windows 真机 `powershell -ExecutionPolicy Bypass -File .\scripts\package-release.ps1` 已通过
- Windows 真机 `powershell -ExecutionPolicy Bypass -File .\scripts\streaming-regression.ps1 -Version 1.0.0-preview.24` 已通过
- Windows 真机 `.\dist\ainput-1.0.0-preview.24\ainput-desktop.exe bootstrap` 已通过
- Windows 真机 `python .\scripts\readme_closeout_guard.py .` 已通过
- 已成功打包 `dist\ainput-1.0.0-preview.24\` 和 `dist\ainput-1.0.0-preview.24.zip`

## 接手提示

- 当前运行：`1.0.0-preview.80` 已启动到 Windows 交互桌面，作为中英混说实测候选。
- 当前冻结：`1.0.0-preview.78` 已在 2026-05-11 经用户真实使用确认，仍是中文体感冻结基线和快速回滚点。
- 当前基线：识别速度快、HUD 实时、松手上屏快；后续改动不得用未实测方案覆盖这条已确认链路。
- 当前进度：`1.0.0-preview.80` 是独立在线流式 ASR 默认版，默认模式为 `online_streaming`，只在 zh-CN CTC 上做安全 code-switch 修复。
- 当前入口：`C:\Users\sai\ainput\dist\ainput-1.0.0-preview.80\ainput-desktop.exe`。
- 当前包：`dist\ainput-1.0.0-preview.80\` 与 `dist\ainput-1.0.0-preview.80.zip`。
- 当前主链：在线流式模式按下 `Ctrl` 打开 HUD，按住期间通过 `vps-jp` Parakeet CTC zh-CN adapter 持续返回 partial；松手时 HUD 有文本就立刻粘贴 HUD snapshot，远端 finish/cleanup 后台完成。
- 回滚点：`dist\ainput-1.0.0-preview.78\` 是当前中文体感快速回滚；`dist\ainput-1.0.0-preview.72\` 保留为本机 Qwen 版本，冻结参数仍是 `gpu_memory_utilization = 0.30` 与 `sidecar_idle_unload_ms = 3600000`。
- 安全边界：NVIDIA key 只从 `vps-jp` 8317 生产配置读取，不写入 Windows 包；本轮不修改 `cliproxyapi` 8317 生产服务本体。
- 收口经验：AInput 前台链路只以真实 Windows hotkey/HUD/上屏体感放行为最终验收；`/health`、compile、package 和日志只能作为辅助证据。

## 当前边界

- 当前流式默认走 `vps-jp` 临时在线 NVIDIA Parakeet adapter；极速语音识别仍保留 SenseVoice 离线整段识别
- preview.80 没有切模型和部署；只启用了低强度可禁用 speech context，并加精确后处理
- 默认流式 ASR 依赖 NVIDIA 在线服务；应用层 AI rewrite 暂不属于当前版本，后续另开独立 spec
- AI 语义改写已从当前版本范围移出；当前上屏文本只来自 HUD truth snapshot
- `HUD 即最终结果` 是当前约束；短停顿 endpoint 和 release-time offline final 不能重新接回默认提交链
- `scripts\run-streaming-live-e2e.ps1 -InteractiveTask` 是当前无人值守前台 synthetic 验收入口
- `scripts\live-streaming-acceptance.ps1` 仍然只用于人工真实麦克风热键验收
- 语音热键与截图热键已经可配置，但当前仍以编辑 `ainput.toml` 为主
- 截图热键走 Windows 原生 `RegisterHotKey`
- 语音为了保留“按住说话/松开停止”的行为，仍需要低层按键监听配合
- 按键精灵录制与回放当前也依赖低层键盘鼠标 hook
- 开机自动启动通过当前用户的 `Run` 注册表项实现
- 不同应用对直接粘贴的前台体验可能略有差异
- 某些不支持 UI Automation 的输入框，会退到统一的未知上下文策略

## 仓库卫生要求

- 根 `README.md` 是唯一当前进度标准
- 每次影响前台体验、模型、默认配置、发包版本或验收方式的改动，都要同步回写这里
- 发包收口默认要求同时满足：
  - 相关代码已验证
  - README 已同步
  - 已提交并推送远端
  - `git status --short` 为空

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

## 历史交接：preview.72 Qwen context echo guard 收口

preview.72 保留为本机 Qwen 回滚版本：

- `dist\ainput-1.0.0-preview.72\`
- `dist\ainput-1.0.0-preview.72.zip`

preview.72 的流式默认后端是 `qwen3_sidecar`，通过 WSL2 使用原版 `Qwen/Qwen3-ASR-0.6B`。当前冻结的 Qwen 参数是：

- `chunk_size_sec = 0.18`
- `unfixed_chunk_num = 4`
- `unfixed_token_num = 5`
- `max_new_tokens = 64`
- `enforce_eager = false`
- `sidecar_idle_unload_ms = 3600000`

本轮架构边界：

- Qwen 流式路径以 HUD 当前文本为最终真相源，松开热键时优先快速提交 HUD 快照。
- Qwen 流式路径不再本地强行补 `。` / `？` / 逗号；旧句末补标点调用保留为注释，方便以后恢复。
- 应用层 AI rewrite 当前关闭；本阶段不再叠加 8317 改写请求，后续若恢复必须另开 spec 并保证不绕过 HUD truth。
- Qwen context echo guard 必须在 HUD truth update 前执行；提示词不能先显示到 HUD 再消失。
- 非流式 / fast SenseVoice 路径与 Qwen 流式路径保持独立，继续保留需要的手动标点逻辑。
- WSL sidecar 由 Windows 侧 `powershell.exe Start-Process wsl.exe` 拉起，WSL 内运行 `.venv/bin/python -m uvicorn qwen3_asr_sidecar:app`，日志在 `/home/sai/ainput-qwen3-asr/qwen3_asr_sidecar.log`。

preview.72 验证记录：

- `cargo fmt`
- `cargo test -p ainput-desktop qwen_context_echo -- --nocapture`：3/3 passed
- `cargo test -p ainput-desktop worker::tests:: -- --nocapture`：72/72 passed
- `scripts\package-release.ps1 -Version 1.0.0-preview.72`
- Windows 交互会话计划任务启动后，`ainput-desktop.exe` 运行于 `SessionId=1`，路径是 `dist\ainput-1.0.0-preview.72\ainput-desktop.exe`。
- 包内配置确认 `voice.streaming.ai_rewrite.enabled = false`。
- `/health` 返回 `idle_unload_ms=3600000`、`requested_enforce_eager=false`、`effective_enforce_eager=false`、`enforce_eager_fallback=true`。

构建正式版：

```powershell
cargo build --release -p ainput-desktop
```

正式版不会弹黑色命令行窗口。

推荐直接用发包脚本按当前工作区版本产出便携包：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\package-release.ps1
```

当前发布目录结构使用：

- `dist\ainput-1.0.0-preview.50\`
- `dist\ainput-1.0.0-preview.50.zip`

发包前门禁：

- `.\scripts\run-streaming-full-audit.ps1 -Version 1.0.0-preview.50 -LatencyRepeats 1 -LiveCaseLimit 3`
- `.\scripts\run-streaming-selftest.ps1 -Version 1.0.0-preview.50`
- `.\scripts\run-startup-idle-acceptance.ps1 -Version 1.0.0-preview.50 -IdleSeconds 30 -Runs 1 -InteractiveTask`
- `.\scripts\run-streaming-live-e2e.ps1 -Version 1.0.0-preview.50 -Synthetic -InteractiveTask`
- `.\scripts\run-streaming-live-e2e.ps1 -Version 1.0.0-preview.50 -Wav -InteractiveTask`
- `dist\ainput-1.0.0-preview.50\ainput-desktop.exe replay-streaming-manifest fixtures\streaming-user-regression-v12\manifest.json`
- `.\scripts\run-streaming-raw-corpus.ps1 -ExePath .\dist\ainput-1.0.0-preview.50\ainput-desktop.exe -RawDir .\dist\ainput-1.0.0-preview.43\logs\streaming-raw-captures -ShortCount 1 -LongCount 1`
- `python .\scripts\readme_closeout_guard.py .`

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
- 以 HUD 为核心的真流式语音输入体验
- 语音、截图、录屏、按键精灵四条主链路的稳定常驻
- 后台维护动作与前台主链路解耦
