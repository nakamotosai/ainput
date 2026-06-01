# Streaming Worker V2 PLAN

> 状态：已被 `../streaming-realtime-rewrite-v3/` 取代。本文只保留历史背景；继续开发和验收以 V3 `SPEC.md` / `PLAN.md` / `TASKLIST.md` 为准。

## 第 1 步：接通真正在线流式解码

- 在 `apps/ainput-desktop/src/worker.rs` 为流式模式改用 `StreamingZipformerRecognizer`。
- 录音开始时创建 `StreamingZipformerStream`。
- 录音过程中持续采集增量样本、喂流、增量 decode，并按现有节流策略更新 HUD 文本。
- 当在线 partial 明显偏短时，自动触发同模型整段 preview rescue，把 HUD 从残句拉回完整趋势。

## 第 2 步：收口最终提交链路

- 松手时保留最后一批增量样本采集、`input_finished`、`decode_available`。
- 删除流式模式里对累计录音再次切回 `SenseVoice` 离线识别的路径。
- 最终提交统一改成 streaming 模型整段 rescore，并记录 `online_raw_text` / `final_rescore_text`。
- 为流式模式单独降低热键释放等待和粘贴稳定等待。
- 补齐关键 timing 日志，便于后续继续压缩延迟。

## 第 3 步：验证与收口

- Windows 真机编译 `ainput-desktop`。
- 跑 `ainput-desktop` 相关测试或等价回归。
- 追加固定 wav 的流式回归脚本，避免再完全依赖人肉口述复现。
- 必要时重打便携包。
- 回写 `README.md`、`MISTAKEBOOK.md` 和任务级 Spec/Plan。
