# Streaming Tail Artifact And Punctuation V12 SPEC

更新时间：2026-05-01

## 目标

在既有 `preview.43` HUD 最终真相源基础上，继续修流式输出的三类新失败：

- 短句尾字仍会漏，例如“问题好像不是很大”提交成“问题好像不是很。”
- 中文句尾会拼出孤立英文 `I`，例如“重复I。”、“都不I。”。
- 标点和轻整理不能把固定词组硬拆开，例如“标点，符号”；明显语义应收成“标点符号都不对。”。

## 范围

- 只修流式模式。
- 非流式 `Alt+Z` 不动。
- 流式热键仍是按住 `Ctrl` 录音、松开上屏，不改热键策略。
- 保持 `clipboard + Ctrl+V` 主链路，不重写上屏机制。
- 不接 AI rewrite、不换模型、不启 GPU。
- 必须打新 preview，不覆盖 `preview.43`。

## 新增验收

在旧验收标准基础上追加：

1. 尾字门禁  
   `streaming-raw-1777610868873.wav` 必须输出 `我试了短句的话，问题好像不是很大。`，不能少最后的“大”。

2. 孤立 `I` 门禁  
   中文句尾不得出现孤立 `I` / `I。` / `I .` / `I 。`。  
   `streaming-raw-1777610886482.wav` 不能输出 `重复I。`。  
   `streaming-raw-1777610915265.wav` 不能输出 `不I。`。

3. 标点词组门禁  
   `标点符号` 不能被拆成 `标点，符号`。  
   `都不I。` 这类尾巴必须修成 `都不对。`，不能保留英文幻觉。

4. 旧门禁继续保留  
   HUD final ack、output commit、target readback 必须一致；不得双重上屏；不得 ghost `yeah`；不得破坏 `Ctrl+A/C/V`；HUD 不闪不抖；raw corpus 至少覆盖短句和长句。

## 完成标准

- 新 raw 样本固定进测试资产或测试脚本。
- Rust 单元测试覆盖尾字、孤立 `I`、标点词组。
- Windows 真机通过相关 `cargo test`。
- 新 preview 打包到 `dist` 并启动到 Windows 交互桌面。
- README / TASKLIST / 本 spec 记录结果。

## 本轮结果

- 已交付 `1.0.0-preview.45`。
- 固定样本见 `fixtures\streaming-user-regression-v12\`。
- 验收结果见 `RESULTS.md`。
