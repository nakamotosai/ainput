# ainput TASKLIST

说明：
每一轮做完，直接勾选；每一轮未完成项保留到下一轮继续推进。

---

## Round 0：方案冻结与启动资产

- [x] 产品名固定为 `ainput`
- [x] 项目根目录固定为 `C:\Users\sai\ainput`
- [x] 技术路线固定为 `Rust 主程序 + sherpa-onnx Rust API`
- [x] ASR 模型固定为 `SenseVoiceSmall`
- [x] 最终运行时默认不引入 Python
- [x] 建立 `AGENTS.md`
- [x] 建立 `README.md`
- [x] 建立 `SPEC.md`
- [x] 建立 `PLAN.md`
- [x] 建立 `TASKLIST.md`
- [x] 建立 `ARCHITECTURE.md`
- [x] 建立 `DECISIONS.md`
- [x] 建立 `WORKFLOW.md`
- [x] 建立 `OPLOG.md`
- [x] 建立 Rust workspace 骨架文件

完成判定：

- [x] 新会话进入目录后可直接按文档继续

---

## Round 1：Rust workspace 骨架

- [x] 建立 `apps/ainput-desktop`
- [x] 建立 `crates/ainput-shell`
- [x] 建立 `crates/ainput-audio`
- [x] 建立 `crates/ainput-asr`
- [x] 建立 `crates/ainput-rewrite`
- [x] 建立 `crates/ainput-output`
- [x] 建立 `crates/ainput-data`
- [x] 建立基础配置加载
- [x] 建立基础日志初始化
- [x] 跑通 `cargo check`

完成判定：

- [x] workspace 可检查通过

---

## Round 2：ASR 链路选型与最小打通

- [x] 确认使用 sherpa-onnx Rust API 还是 C API 落地
- [x] 固定模型目录约定
- [x] 打通 wav 文件到文本的最小识别
- [x] 打通麦克风录音到文本的最小识别
- [x] 建立 ASR 错误日志
- [x] 记录性能观察

完成判定：

- [x] 本机可得到一条离线识别文本

---

## Round 3：热键、录音、状态机

- [x] 设计按住 `Ctrl+Win` 说话状态机
- [x] 接入 `Ctrl+Win` 全局热键
- [x] 接入麦克风录音
- [x] 处理开始/停止录音
- [x] 建立失败恢复策略

完成判定：

- [x] 热键按住说话主流程可用

---

## Round 4：输出注入

- [x] 剪贴板输出
- [x] 自动粘贴输出
- [x] 插入失败降级
- [x] 最近结果缓存

完成判定：

- [ ] 至少一个 IDE 输入框可用
- [ ] 至少一个浏览器输入框可用

---

## Round 5：极简 UI 与托盘

- [x] 托盘入口
- [x] 录音状态反馈
- [ ] 最近结果预览
- [x] 最小设置入口

完成判定：

- [x] UI 足够轻，且不干扰主流程

---

## Round 6：后续增强能力

- [ ] 设计术语数据结构
- [ ] 建立内置词表
- [ ] 建立用户词表
- [ ] 实现大小写规范
- [ ] 实现空格规则
- [ ] 完成至少 20 个术语
- [ ] 定义模式系统
- [ ] 实现提示词转换第一版

完成判定：

- [ ] 识别后文本增强能力可单独启用

---

## Round 7：模板与账本

- [ ] `debug_root_cause`
- [ ] `fix_minimal`
- [ ] `review_code`
- [ ] `refactor_safely`
- [ ] `write_tests`
- [ ] `explain_code`
- [ ] `spec_first`
- [ ] `command_only`
- [ ] 完成至少 10 条账本模板

完成判定：

- [ ] 至少 8 个场景模板和 10 条账本模板可用

---

## Round 8：打包与回归

- [x] 打包方案确认
- [x] 安装包方案确认
- [x] 安装脚本
- [x] 卸载脚本
- [x] 生成 setup.exe
- [x] 安装/卸载回归一轮
- [x] 模型部署说明
- [x] 回归样例
- [x] 日常实测一轮

完成判定：

- [x] 可形成长期自用版本
