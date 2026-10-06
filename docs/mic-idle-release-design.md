# ainput 麦克风空闲释放 — 设计方案

> 目标：让 `ainput` 不再无限占用麦克风（当前导致整机无法自动休眠）。
> 结论先行：**不必重建音频层，`Stream::pause()` 就能放掉电源请求** —— 最小改动约 15–40 行。

---

## 1 · 问题（已证实）

`ainput` 在启动时一次性 `AudioHub::start_default()`（`src/main.rs:309`），cpal 采集流 `_stream` 在**整个进程生命周期内一直 `play()` 着**（`src/audio.rs:30/113/129`；看门狗只在 5s 卡顿后重建，从不主动停）。于是 USB 麦克风（USB 麦克风，`USB 音频设备`）的采集流常年打开 → `usbaudio` 驱动持有 **SYSTEM 电源请求** → 系统无法自动 S3 休眠。

`powercfg /requests` 里那一行 `[DRIVER] USB Audio Device … 音频流当前正在使用中` 就是它。

---

## 2 · 关键实测（本机，2026-10-06）

用与项目同版本的 **cpal 0.17.3** 写探针（`本地探针`），配合提权 `powercfg /requests` 逐步取证：

| 步骤 | USB 音频电源请求 |
|---|---|
| ainput 在跑（基线） | **PRESENT** |
| 杀掉 ainput | ABSENT（证明就是它） |
| `stream.play()` 后 | PRESENT |
| **`stream.pause()` 后** | **ABSENT** |
| `stream.play()`（恢复）后 | PRESENT |
| 再 `pause()` | ABSENT |
| `drop(stream)` | ABSENT |

**要点**：cpal 0.17.3 里 `pause()` → `IAudioClient::Stop()`，`play()` → `Start()`，**同一个句柄来回切**。`pause()` 已经足够放掉电源请求，**不需要 drop 再重建**。恢复是同一个句柄，没有设备重开、没有冷启动延迟。

> 冷启动延迟参考（万一走重建兜底）：稳态重开 12–50ms，进程首开 p90≈262ms。220ms 的 `activation_delay_ms` 窗口足以覆盖稳态重开。

---

## 3 · 推荐方案：空闲暂停 + 按键恢复（pause/resume）

**核心**：空闲时把采集流**暂停**（保留句柄），按键时**恢复**（同一句柄 `play()`）。drop/重建只作兜底（USB 重插/真休眠后句柄失效时）。

### 3.1 状态

`src/audio.rs` 里把 `_stream: Arc<Mutex<Option<cpal::Stream>>>` 换成带状态的结构：

```rust
struct MicState {
    stream: Option<cpal::Stream>, // 句柄一直留着；None 仅出现在重建/兜底
    paused: bool,                 // true = 已 pause，电源请求已释放
}
```

- `Live`：`stream=Some, paused=false`
- `Paused`：`stream=Some, paused=true`（**正常空闲态**）
- `Closed`：`stream=None`（仅兜底重建的瞬态）

### 3.2 转移

- **`start_default`**：照旧 `play()` → `Live`。新增记录 `last_activity_ms`、`ring_ms`。
- **`Live → Paused`**（由监督线程做）：`subscribers.is_empty()` 且 `now - last_activity_ms >= mic_idle_pause_ms` 时，调 `stream.pause()`，置 `paused=true`，`level_milli=0`。
- **`Paused → Live`**（由 `subscribe()` 同步做）：`stream.play()`，`paused=false`，**清空 ring**（避免把暂停前的旧音频当 pre-roll 注入），刷新 `last_callback_ms`。
- **`Closed → Live`**：兜底重建（现有 `build_input_stream` + `play`）。

### 3.3 监督线程（复用现有看门狗）

`spawn_watchdog`（`audio.rs:303-382`）本来就在 2s 轮询、且已持有 stream 句柄 —— 直接改成「空闲暂停」监督器，不新增线程：

```rust
loop {
    sleep(POLL);
    // 有订阅者 = 正在听写，刷新活动时间
    if !state.lock().subscribers.is_empty() { activity.store(now_ms()); }

    let mut mic = self.mic.lock();          // 与 subscribe() 串行化
    if mic.paused { continue; }             // ★ 已暂停：整段跳过，绝不走 stall 重建
    if mic.stream.is_some() {
        let cb_idle = now_ms() - health.last_callback_ms.load();
        if cb_idle >= STALL_THRESHOLD_MS {
            rebuild();                       // 现有卡顿重建（设备被拔等）
        } else if state.lock().subscribers.is_empty()
               && now_ms() - activity.load() >= IDLE_PAUSE_MS {
            if let Some(s) = mic.stream.as_ref() { let _ = s.pause(); }
            mic.paused = true;               // ★ 释放电源请求
            state.lock().ring.clear();
            level_milli.store(0);
        }
    }
}
```

**★ 两条铁律**（否则功能失效）：
1. **`paused` 时整段跳过 stall 分支** —— 暂停后回调停止、`last_callback_ms` 冻结，老的 5s 卡顿判据会误判「卡顿」并把流重建回来，等于白暂停。（这正是核验中唯一被 REFUTE 的点：现看门狗会复活流。）
2. **`paused` 置位要在同一把锁内、先于任何并发 `subscribe()`** —— 避免 subscribe 看到「未暂停」却拿不到回调。

### 3.4 `subscribe()` 冷恢复

```rust
pub fn subscribe(&self, pre_roll_ms: u64) -> AudioSession {
    self.ensure_live();                       // 新增：同步恢复
    // …原有逻辑：prune_disconnected / 推订阅者 / 从 ring 取 pre-roll…
}

fn ensure_live(&self) {
    let mut mic = self.mic.lock();
    self.activity.store(now_ms());            // 已 Live 时也刷新
    if mic.paused {
        if let Some(s) = mic.stream.as_ref() {
            match s.play() {
                Ok(()) => { mic.paused = false;
                            self.health.last_callback_ms.store(now_ms());
                            self.state.lock().ring.clear(); }
                Err(e) => { /* 句柄失效 → 兜底 drop+重建（见 3.5） */ }
            }
        } else { /* Closed → 兜底重建 */ }
    }
}
```

因为恢复是**同一句柄 `play()`（亚毫秒级）**，且 `subscribe()` 在 PrePress（按键按下瞬间，`hotkey.rs:426-434`）就被调用，恢复一定落在 220ms 提交窗口之内 —— 首音节不丢。

### 3.5 兜底：真休眠/USB 重插后重建

`pause()` 后若经历真实 S3 或 USB 重枚举，句柄可能失效：`play()` 返回 `Err`，或恢复后 5s 内无回调。此时走现有 `build_input_stream` + `play` 重建路径（清 ring、重置 `last_callback_ms`）。这条路径**只在异常时触发**，正常空闲不会走。

---

## 4 · 改动清单

| 文件 | 改动 |
|---|---|
| `src/audio.rs` | 核心：`MicState{stream,paused}`；`start_default` 存 `ring_ms`/`last_activity_ms`；`subscribe` 调 `ensure_live()`；`spawn_watchdog` → 加空闲暂停分支 + `paused` 跳过 stall；新增 `IDLE_PAUSE_MS` 常量 |
| `src/main.rs:309` | 把 `config.asr.mic_idle_pause_ms` 传进 `start_default`（其余不动，`hud.bind_audio_level` 不变） |
| `src/config.rs` | `AsrConfig` 加字段 + 默认值（`#[serde(default)]` 保证老配置兼容） |
| `config/ainput.toml` | 文档化新开关 |
| `Cargo.toml` | `0.1.28-preview → 0.1.29-preview` |

**`pipeline/mod.rs` 不用改** —— `subscribe()` 契约不变（仍返回带 pre-roll 的 `AudioSession`），`arm_pre_press_slot`（L479）和遗留 subscribe（L1634）照旧。

---

## 5 · 配置项

```toml
[asr]
mic_idle_pause_ms = 30000   # 空闲多久暂停麦克风；0 = 永不暂停（保留今天的行为）。默认建议 30–60s
mic_resume_priming_ms = 200 # 恢复后最多等多久等首个回调；兜底重建用
```

- `mic_idle_pause_ms = 0` 即**完全回退**到今天的行为（逃生阀）。
- 默认取 30s：正常听写间隙不会来回切；停下来半分钟才释放。
- 与 pause 配合**没有设备断连声**（不像 close），churn 风险低。

---

## 6 · 失败模式与对策（核验产出）

| 风险 | 对策 |
|---|---|
| 看门狗把暂停的流重建回来（**已实测会复发**） | `paused` 时整段跳过 stall 分支（3.3 铁律 1） |
| 恢复瞬间把暂停前的旧音频当 pre-roll | 恢复时 `ring.clear()` |
| `play()` 后立刻被判「卡顿」重建 | 恢复时重置 `last_callback_ms` |
| 锁重入自死锁 | 所有状态改动在**已持有的** `mic` 守卫内完成，不二次 `lock()` |
| 恢复失败（句柄失效） | 兜底 drop+重建（3.5），并在 HUD 提示 |
| 首个用例冷启动慢 | 启动时做一次「开→播→停」预热（可选，把首开从 p90≈262ms 降到 ~13ms） |

---

## 7 · ⚠️ 必须一并处理：`第二个听写程序`

**只改 `ainput` 不足以让电脑睡觉。** 实测电源请求是**按设备**计的：另一个程序再开一路采集流，那一行就回来了（探针里验证过）。而 `第二个听写程序` 有**完全相同的常驻占用**，且**开机自启**：

- `HKCU\…\Run` → `第二个听写程序` = `第二个听写程序\dist\第二个听写程序.exe`
- `HKCU\…\Run` → `ainput` = `F:\projects\ainput\ainput.exe`（另有 `Startup\ainput.lnk`）

`第二个听写程序` 的 AGENTS.md 明确「Do not modify 第二个听写程序」，所以这是**用户决策**，不是本仓改动：

- **选项 A（推荐）**：改完 `ainput` 后，把 `第二个听写程序` 的自启关掉（删 `HKCU\…\Run` 的 `第二个听写程序` 项 + 其 Startup 项），确认不再需要它。
- **选项 B**：把同样的暂停补丁打到 `第二个听写程序` 自己的仓库。

---

## 8 · 验收标准

1. **电源请求线**（判据是**具体那一行**，不是整表为空 —— `旧的内核调用程序` 会一直在）：
   `powercfg /requests` 中 `USB Audio Device (USB 音频设备…)` 那行在**空闲时 ABSENT**、**按住键时 PRESENT**。
2. **真的会睡**：把睡眠超时设短，离开，确认进入 S3。
3. **首音节 A/B**：`mic_idle_pause_ms=0` vs 默认，各 ≥20 句以爆破音开头（北京/塔/打…），判据 0/20 丢字。
4. 用**本项目构建的 exe**（不是 PortAudio 代理）复跑第 1 条。

---

## 9 · 落地步骤（遵 AGENTS.md 铁律）

1. 改 `src/` 前：`src/` 整体备份到 `F:\archive\<时间戳>-ainput-idle-pause\`
2. 按 §4 改码
3. `cargo build --release` + `cargo test`
4. `Cargo.toml` 抬到 `0.1.29-preview`
5. 当轮 `git commit`（一功能一提交）
6. 部署后验三样：Path=根 exe、时间戳=本次、版本号=本次提交
7. 用 §8 验收

---

## 10 · 为什么不选另两条路

- **完全按需开流（on-demand）**：会**丢掉真正的 pre-roll** —— 常驻 ring 能拿到「按下之前」的 160ms，按键才开的流拿不到；改动最大（引用计数、每句开关、释放顺序 vs `drain_release_audio`），收益不比 pause 多。
- **系统信号驱动（息屏/锁屏才释放）**：语义漂亮，但要新增托盘窗口的电源/会话通知面 + 多个 reason 位簿记，**对「整夜不睡」这个具体场景并不比空闲暂停更省**。可作为**后续增强**叠加在 pause 之上。

---

*证据与原始探针：`本地探针目录`（`pause-test-out.txt` 为本表实测），根因报告：`根因报告`。*
