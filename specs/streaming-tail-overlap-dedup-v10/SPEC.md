# Streaming Tail Overlap Dedup V10 SPEC

更新时间：2026-05-01

## 目标

修复流式模式里“看起来像双重上屏”的尾部重复：

- 当 HUD 已经显示一句话尾巴，而松手 final 只识别出尾部窗口时，不能把同一段尾巴再追加一遍。
- 如果 final 正在修正 HUD 尾巴最后 1 个字，例如 `的 -> 了`，也要按“尾部替换/修正”处理，而不是硬拼接。
- 保持一次 release 只提交一次，不改变流式热键和粘贴链路。

## 非目标

- 不改非流式 `Alt+Z`。
- 不改流式 `Ctrl` 热键规则。
- 不改 `clipboard + Ctrl+V` 上屏主链路。
- 不接 AI rewrite。
- 不换模型。

## 真实触发样本

`preview.40` 日志中，`streaming-86`：

- HUD / display：`我都已经设置了多跳思考的`
- offline final tail：`设置了多跳思考了。`
- 错误合成：`我都已经设置了多跳思考的设置了多跳思考了。`

这不是同一个 commit 执行两次；日志里只有一次 `output delivery timing` 和一次 `streaming transcription delivered`。问题在 final commit 文本生成阶段。

## 设计

### 1. Final commit tail overlap repair

在 `select_streaming_commit_text` 里增加一层防守：

- 如果 final candidate 是 `display + appended_tail` 形态；
- 并且 `display` 尾部和 `appended_tail` 头部存在 4 字以上重叠；
- 允许重叠区最多 1 个字符不同；
- 则用 `display` 去掉重叠尾巴后再接 `appended_tail`。

目标输出：

- `我都已经设置了多跳思考的` + `设置了多跳思考了。`
- 合并为 `我都已经设置了多跳思考了。`

### 2. 保留正常追加

以下场景不能被误删：

- `你这些分辨率` + `有问题。` 仍应变成 `你这些分辨率有问题。`
- 短重复语气词如 `喂喂喂` 不靠本规则处理，避免误杀真实重复。

## 验收

- 新增单测覆盖真实失败样本。
- `cargo fmt --check` 通过。
- `cargo check -p ainput-desktop` 通过。
- `cargo test -p ainput-desktop streaming -- --nocapture` 通过。
- `cargo test -p ainput-desktop hotkey -- --nocapture` 通过。
- 包内 startup idle 通过。
- 包内 synthetic / wav live E2E 通过。
- raw corpus 抽样通过。
- 打新 preview，不覆盖旧版，并启动最新 exe 到 Windows 交互桌面。
