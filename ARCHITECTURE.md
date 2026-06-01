# ainput ARCHITECTURE

## 1. 架构结论

`ainput` 采用：

- Rust 主程序
- sherpa-onnx 作为 ASR 运行时核心
- SenseVoiceSmall 作为离线识别模型
- 以“热键录音后直接贴入当前输入区域”为首版主目标

最终运行时目标：

- 不带 Python
- 不做系统级 IME
- 不做复杂大窗体

## 2. 主链路

1. 用户按住 `Ctrl+Win`
2. shell 层进入录音状态
3. audio 层采集麦克风 PCM
4. asr 层调用 sherpa-onnx + SenseVoiceSmall
5. output 层决定自动粘贴 / 剪贴板降级 / 最近结果缓存
6. shell/ui 层给出最小反馈

后续增强链路：

1. ASR 输出文本
2. rewrite 层做术语增强
3. rewrite 层做提示词转换
4. output 层按模式输出

## 3. 模块划分

### `apps/ainput-desktop`

- Windows 主程序入口
- 装配所有 crate

### `crates/ainput-shell`

- 全局热键
- 托盘
- 模式切换
- 配置加载
- 主状态机

### `crates/ainput-audio`

- 麦克风采集
- 音频缓冲
- 录音开始/结束

### `crates/ainput-asr`

- sherpa-onnx Rust API 接入
- SenseVoiceSmall 模型加载
- 语言选择与自动识别
- 原始转写输出

### `crates/ainput-rewrite`

- 后续术语增强
- 后续中英混合文本修正
- 后续场景分类
- 后续提示词改写

### `crates/ainput-output`

- 剪贴板
- 自动粘贴
- 降级处理
- 最近结果缓存

### `crates/ainput-data`

- 内置术语表
- 用户术语表
- 场景模板
- 账本模板

## 4. 数据资产

建议统一放到：

- `data/terms/base_terms.json`
- `data/terms/user_terms.json`
- `data/prompts/builtin_prompts.json`
- `data/prompts/user_prompts.json`
- `data/scenarios/scenarios.json`

## 5. 输出模式

- 当前首版：
  - `direct-paste`
  - `clipboard-fallback`
- 后续预留：
  - `raw`
  - `terms`
  - `prompt`
  - `command`

## 6. 风险点

- sherpa-onnx Rust API 的实际集成复杂度需尽快验证
- Windows 输入注入方案需要尽早做可用性验证
- `Ctrl+Win` 作为快捷键在系统层面的冲突与可拦截性需要尽早验证

## 7. 设计取舍

- 先不要做系统级 IME
- 先优先保证常驻主流程稳定
- 先不要做术语增强和提示词转换
- 先不要做复杂 GUI
- 先不要做插件系统
