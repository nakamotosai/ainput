# ainput 发布前全量体检审计（2026-10-06）

> 方法：7 个独立审计维度并行发现（发布打包 / 安全隐私 / 正确性与 panic / 首次运行 UX /
> 死代码与构建 / 文档真实性 / 空闲暂停新功能对抗审查），每条发现再由独立代理对抗核验。
> **61 项发现，57 项确认**（1 blocker / 8 high / 25 medium / 23 low，3 项被推翻）。
> 本轮已修其中**真 bug 与合规**部分；产品决策项（签名证书、发布渠道、私有文件去留）留待拍板。

---

## 一、已修复（本轮）

| # | 级别 | 问题 | 修复 |
|---|---|---|---|
| 1 | **blocker** | 原始录音（用户说的每一句话）**静默落盘**成 `.wav`（`state/logs/audio/`，最多 200 条），所有构建都开，用户不知情 | 改为**默认关闭**，仅 `AINPUT_DUMP_AUDIO=1` 开启（`src/pipeline/mod.rs`） |
| 2 | high | 日志明文写入**听写文本 + 目标窗口标题/进程**，永不清理 | 新增 `log_text()`：发布版只写**字符数**，`AINPUT_LOG_TEXT=1` 才写全文；窗口标题同脱敏（`src/pipeline/mod.rs`、`src/output.rs`） |
| 3 | high | 热键默认 `MouseX1`（鼠标侧键），与 README/站点/产品承诺的 **CapsLock 矛盾** | 代码默认 + 发货 config 改为 **CapsLock**（本机靠 `hotkey-user.toml` 仍用 MouseX1） |
| 4 | high | `make-portable.ps1` 通配拷贝**所有** target DLL，误带 275MB CUDA/TensorRT provider（CPU 推理根本不用） | 改**白名单**：只带 onnxruntime + sherpa + cargs；实测包从 ~338MB 降到 **164MB** |
| 5 | high | 绿包不含 MSVC 运行库 `VCRUNTIME140.dll`（硬依赖），干净机器"解压即用"会**启动失败** | 打包补 `vcruntime140.dll` / `vcruntime140_1.dll` / `msvcp140.dll` |
| 6 | high | 唤醒词硬编码为开发者昵称 **"老蔡老蔡"** 并显示在托盘 | 默认改中性词；旧词保留兼容；托盘标签读**实时**唤醒词（`src/voice_command.rs`、`src/tray.rs`） |
| 7 | high | 所有文档化的打包命令**版本号写死且过期**（`-Version 0.1.3` 等），按文档必失败 | `make-portable.ps1` 版本号**从 Cargo.toml 自动派生**；`build-dist.bat`/README/AGENTS/HEALTHCHECK 同步 |
| 8 | medium | 第三方许可**文本缺失**（只有 URL 占位），onnxruntime 未列 | 补 onnxruntime(MIT) 条目；写入真实 SenseVoice **MIT** 许可全文 |
| 9 | medium | config 损坏 → 启动**硬崩**无兜底 | 备份为 `.toml.bad` 并回落默认（`src/config.rs`） |
| 10 | medium | 自启注册表值**未加引号**，安装路径含空格即失效 | 写入时加引号（`src/tray.rs`） |
| 11 | medium | "清历史"不删录音文件，隐私残留 | `history::clear` 一并删 `audio/`（`src/history.rs`） |
| 12 | medium | README 下载段/构建路径/超时默认(5000 vs 实际 15000)/隐私说明过期 | 全部校正 |
| 13 | low | 打包脚本直接调用时 `cargo` 不在 PATH | 脚本自动补 `~/.cargo/bin` |
| 14 | low | 设计文档含私有路径/设备 ID（即将公开） | 已脱敏 |

**验证**：`cargo build --release` 通过；`cargo test` **138 passed / 0 failed**；实跑新构建——空闲 30s 后麦克风电源请求释放（USB 行消失）、日志无明文语音、无 `audio/` 目录；打包实测 164MB、无 CUDA、含 VCRUNTIME、无 `state/` 泄漏。

提交：`ebf61b0`（审计修复）+ 后续打包修复。版本 `0.1.29 → 0.1.30-preview`。

---

## 二、待你拍板（未擅自改）

### 发布流程类（不是代码 bug，是"怎么发"）
- **exe 未签名** → 公网下载触发 SmartScreen"Windows 已保护你的电脑"，很多人会放弃安装。需要代码签名证书（EV/OV）+ 签名步骤。
- **无发布手册**：0.1.2/0.1.3 的 release notes 只在 gitignore 的 `dist/` 里，没有可复现的发布步骤（GitHub + HuggingFace + 站点三通道）。
- **版本号带 `-preview`**：对外发布建议去掉后缀，出一个正式版本号。
- **线上全是旧的**：GitHub 最新 `v0.1.3`、站点 JS 还写 `v0.1.2`、README 原写 v0.1.3 —— 都落后本地 26+ 个提交。发新版需要：定版本号 → 打包 → 建 GitHub release + 传 HF 镜像 → 更新站点与 README。

### 仓库卫生类（你的私有工作流文件）
- 公开仓库目前跟踪了 `AGENTS.md`、`.scold/ledger.md`、`.scold/state.json` —— 里面有 本机私有路径等和内部事故记录。是否 `git rm --cached` 移出 + 加 `.gitignore`，由你决定（这是你的开发规范文件，我没动）。

### 功能/质量类（可选）
- 93 条编译警告（死代码：整块云/流式 ASR 已禁用但仍编译构造；未用依赖 `hound` 等）—— 建议清理，非阻塞。
- `config/ainput.toml` 是**开发机个人配置**（`wezterm` 白名单、调过的时间参数），发版前应换成干净默认。
- `personal-dictionary.json` 陈旧且引用已移除的云 ASR 功能。
- 听写历史无上限增长、无轮转（隐私 + 磁盘）。

---

## 三、空闲暂停新功能（上一轮改动）的审计结论

- 实测通过：空闲暂停释放电源请求、按键恢复、首音节不丢（详见 `docs/mic-idle-release-design.md`）。
- 审计要求补的验收：`mic_idle_pause_ms=0` 与默认的**首音节 A/B**（≥20 句爆破音开头）尚未做——建议发布前跑一遍。
- 设计文档承诺的 `mic_resume_priming_ms` 配置项**实际未实现**（当前用常量）；要么补上要么改文档。

---

*审计由 7 维发现 + 逐条对抗核验（共 68 个代理）产出；原始 JSON 见会话留档。*
