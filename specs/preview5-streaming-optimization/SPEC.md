# preview.5 真流式标点重构 SPEC

> 状态：已被 `../streaming-realtime-rewrite-v3/` 取代。本文只保留历史背景；继续开发和验收以 V3 `SPEC.md` / `PLAN.md` / `TASKLIST.md` 为准。

## 目标

- 基于当前可用的 `preview.4` 流式链，交付新的 `preview.5`：
  - 按住说话时 HUD 持续出字。
  - HUD 中最近一句允许被自动修正。
  - 松手后一次性上屏完整整句。
  - 标点不再依赖规则硬补，而是改走官方模型能力。
- 保持“真流式 ASR”作为主识别链路，不回退到整段重跑的伪流式方案。

## 用户验收

- 按住热键连续说话时，HUD 文字持续增长，不能只出前几个字。
- 说错后又更正时，HUD 中最近一句可以被自动修正成更接近正确语义的文字。
- 松手后最终上屏文本必须与 HUD 最终显示保持同一条文本链，不允许“HUD 一份、聊天框另一份”。
- 长句不能明显丢字。
- 标点主要来自模型，不接受继续靠本地规则拼凑出逗号和句号。

## 非目标

- 不重做 HUD 样式。
- 不在本轮解决所有长句漏字问题的声学层根因；本轮先把模型链路和文本链路统一正确。
- 不删除极速模式。

## 已确认事实

- 当前项目依赖 `sherpa-onnx = 1.12.33`，Rust API已内置：
  - `OnlineRecognizer`
  - `OfflinePunctuation`
  - `OnlinePunctuation`
- 官方资料显示：
  - 中文官方标点模型是 `sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12-int8`
  - 英文才有官方在线标点模型 `sherpa-onnx-online-punct-en-2024-08-06`
  - 当前更适合的中文真流式 ASR 模型是 `sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30`
- 实测官方模型体积：
  - `sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30` 解压后约 `161M`
  - `sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12-int8` 解压后约 `77M`
  - 现有 `sense-voice` 约 `230M`
  - 三者合计约 `468M`，满足用户要求的 `<=500MB`

## 方案判断

### 不采用的路线

- 不继续沿用 `ainput-rewrite` 里的规则型标点作为主方案。
- 不改成 `SenseVoice` 整段重跑的伪流式路线。
- 不要求“ASR 模型自己直接稳定输出中文标点”，因为官方现成中文真流式模型没有提供这条更稳的能力说明。

### 采用的路线

- 真流式识别继续使用 `StreamingZipformerRecognizer`。
- 标点改为官方中文 `OfflinePunctuation` 模型。
- 标点只作用于“最近一句”，不全文反复改写。
- HUD 与最终提交统一走同一条“已加标点的显示文本”链。

## 设计约束

- 只能冻结已经形成句末边界的前缀。
- 最新一句允许继续变化，并重新经过标点模型。
- 标点模型失败时：
  - 允许回退到无标点原文。
  - 不允许回退到规则拼标点。
- `ainput-rewrite` 仍可保留“空白清理、少量常见错字规整”这类轻量规范化能力，但不再承担主标点职责。

## 实施范围

### 代码

- `crates/ainput-asr`
  - 增加中文离线标点封装与模型发现逻辑。
- `apps/ainput-desktop/src/worker.rs`
  - 预览与最终提交统一接入“最近一句加标点”链路。
- `apps/ainput-desktop/src/streaming_state.rs`
  - 继续只冻结前缀，允许最新一句重复改写。
- `crates/ainput-shell`
  - 增加流式标点模型配置。
- `scripts/package-release.ps1`
  - 打包新的流式 ASR 模型与标点模型。

### 模型与配置

- 新增：
  - `models/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30`
  - `models/punctuation/sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12-int8`
- 配置层统一改成单一真相源，不再出现“配置名、脚本名、目录名”不一致。

## 验收标准

- 流式模式下，HUD partial 在连续说话时持续更新。
- HUD 中出现的最近一句标点，与松手后上屏的最终文本保持一致。
- 明显错误的规则型标点样式消失，例如：
  - “怎么？回事，。”
  - HUD 与最终文本完全不同步
- 相关单测通过：
  - 流式状态机
  - 文本准备/提交链
  - 新增标点封装
- 打包脚本能包含 preview.5 所需模型。

## 失败回退

- 如果官方中文标点模型在当前流式链上导致明显卡顿或频繁失败：
  - 保留真流式 ASR
  - 标点模型仅在松手 final 阶段启用
  - HUD 阶段先只做无标点或轻量刷新
- 但即使触发该回退，也不重新启用规则补标点作为主路线。
