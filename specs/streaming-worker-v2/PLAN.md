# Streaming Worker V2 PLAN

## 第 1 步：接通真正在线流式解码

- 在 `apps/ainput-desktop/src/worker.rs` 为流式模式改用 `StreamingZipformerRecognizer`。
- 录音开始时创建 `StreamingZipformerStream`。
- 录音过程中持续采集增量样本、喂流、增量 decode，并按现有节流策略更新 HUD 文本。

## 第 2 步：瘦身松手后的最终提交链路

- 松手时只做最后一批增量样本采集、`input_finished`、`decode_available` 和结果收尾。
- 删除流式模式里对累计录音再次整段离线识别的路径。
- 为流式模式单独降低热键释放等待和粘贴稳定等待。
- 补齐关键 timing 日志，便于后续继续压缩延迟。

## 第 3 步：验证与收口

- Windows 真机编译 `ainput-desktop`。
- 跑 `ainput-desktop` 与 `ainput-shell` 相关测试。
- 必要时重打便携包。
- 回写 `README.md`、`MISTAKEBOOK.md` 和任务级 Spec/Plan。
