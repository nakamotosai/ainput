# Streaming Tail Artifact And Punctuation V12 PLAN

更新时间：2026-05-01

## Phase 1: 固定失败样本

- 复制用户最新 raw wav 到固定 fixture 目录。
- 建 manifest，写入 expected text / forbidden artifacts。
- 最小验证：`replay-streaming-wav` 能复现 `preview.43` 的漏字和 `I` 问题。

## Phase 2: 修 final 选择策略

- offline final 已经跑完并返回有效文本时，不再因为旧 350ms hard budget 直接丢弃可修尾字的结果。
- 对 tail-window offline final 的短英文幻觉做拒绝：孤立 `I`、`Yeah`、`Okay` 等不能拼到中文 HUD 尾部。
- 最小验证：新增 worker 单元测试。

## Phase 3: 修本地轻整理

- 增加流式安全清洗：`不I` -> `不对`，中文尾部孤立 `I` 删除。
- 增加已观察到的语音误识别修正：`强治/强距` -> `简直`，`标点，符号` -> `标点符号`。
- 最小验证：`ainput-rewrite` / `ainput-desktop streaming` 测试。

## Phase 4: 扩展验收脚本

- raw replay / manifest 验收增加 forbidden artifacts。
- 保持旧 HUD/readback/重复上屏/ghost/快捷键门禁。
- 最小验证：用户 raw 样本 replay 全部通过。

## Phase 5: 打包与启动

- 打新 preview。
- 运行 startup idle、synthetic live、wav live、raw corpus。
- 启动最新版到 Windows 交互桌面。
- README / TASKLIST / RESULTS 收口。
