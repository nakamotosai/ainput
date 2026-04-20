# ainput 项目错题本

## 适用范围

- 这份错题本只记录 `ainput` 这个项目里已经反复踩过、且后续很容易再次复发的问题。
- 目标不是追责，而是下次遇到同类症状时，先排除最常见的错误路径，减少返工。

## 流式语音 HUD / 预览链路

### 不能在没有真机热键实测前宣称“流式模式已修好”

- 这条链路同时跨了热键、录音、识别、HUD、改写、粘贴六层，单看代码或单跑编译都不能证明最终可用。
- 至少要做一次真实的“按住热键持续说话 -> HUD 持续更新 -> 松手整段提交”的现场验证，再说完成。

### 看到 HUD 只显示两个字时，先查识别链路，不要先怪 UI

- 之前连续返工的根因，不是 HUD 截断文本，而是预览链路只产出了两个字。
- 排障顺序必须是：
  1. 先看 `logs/ainput.log` 里有没有 `streaming partial updated`
  2. 再看 `logs/voice-history.log` 末尾是否真的有完整结果
  3. 最后才讨论 HUD 的排版或窗口尺寸

### 流式预览不稳定时，不要死磕“真 streaming partial”假设

- 之前硬追 zipformer 在线 partial，结果速度慢、文本短、错误重复。
- 对 `ainput` 这种“按住说话、松手整段提交”的场景，稳定优先，必要时直接退到“累计音频 + 周期整段重识别”的预览方案。

### 纯在线 zipformer 在真机麦克风上又只出前几个字时，直接回混合链，不要再从 HUD 和热键层绕圈

- `1.0.0-preview.1` 这次复发里，`total_chunks_fed` 和 `total_decode_steps` 一直在涨，说明录音、喂流、decode 都没停；真正坏掉的是 `online get_result()` 长时间只给出前几个字。
- 这类症状下，继续调 HUD、调热键释放等待、调粘贴速度都没有意义，根因在于“纯在线 final / partial 不能单独扛住真实长句口述”。
- 正确回退路线是：
  1. HUD 保留在线 partial。
  2. 预览周期固定追加同模型 `transcribe_samples()` rescue。
  3. 松手最终提交统一再跑一次同模型整段 rescore。
  4. 日志必须同时打印 `online / rescue / selected_source`，否则下次还会误判成 UI 问题。

### HUD 需求要按用户真实验收来做，不要自作主张加多余状态行

- 用户明确要的只是文字本身。
- 占位符只有 `请说话`，收尾只有 `已识别`，其余说明、标题、双行状态都属于噪音。

### HUD 微流式 diff 不能拿“当前正在显示的文本”直接和新目标做全局公共前缀

- `preview.10` 这轮的剧烈抖动，根因不是模型，而是 HUD 每次都拿 `hud_display_message` 去和新文本算 LCP；一旦中间某个字被修正，显示串就会被截回很短的前缀，然后再整段重新长出来。
- 正确做法是把 HUD 拆成三层：
  1. 已结束句子的冻结前缀
  2. 上一轮目标尾巴
  3. 当前实际显示尾巴
- 后续只允许：
  - 冻结前缀单向增长，不回退
  - 修正只作用在未冻结尾巴
  - 只有新增尾巴继续逐字吐出
- 只要用户再次反馈“HUD 会先缩短再变长”，第一检查点就该回到这里，而不是继续盲调识别频率。

### 空的 StreamingPartial 不能再覆盖 HUD 占位符

- `preview.11` 暴露出的新回归，不是识别失败，而是 worker 在说话过程中会间歇发出“当前 preview 为空”的 `StreamingPartial`。
- 如果主线程把这种空 partial 重新翻译成 `请说话`，HUD 就会表现成“明明识别到了字，但一闪又被打回占位符”。
- 正确规则是：
  - `请说话` 只允许在开始录音、还没出现任何识别文字前显示。
  - 一旦 `raw_text / prepared_text` 出过非空内容，后续空 partial 必须直接忽略，不能覆盖当前 HUD。

### HUD 占位符绝不能写进微流式 committed/target/display 状态

- `preview.12` 继续失败的真正根因，是我把 `请说话` 当成普通 HUD 文本写进了 `HudMicrostreamState`。
- 这样第一条真实 partial 到来时，内部状态的 `committed_prefix` 还是 `请说话`，后续真实识别文本全都变成“挂在 `请说话` 后面”的尾巴；用户看到的就会像 HUD 永远卡在占位符上。
- 正确做法是：
  - 占位符只做显示，不进入 committed/target/display 任一内部字段。
  - 第一条真实 partial 到来时，内部状态必须仍然是空白初始态。
  - 如果此时需要热重载 HUD，也只能根据“placeholder_active / target / display”重新选显示文案，不能把占位符回写到流状态里。

### HUD 位置不能拍脑袋固定，要基于工作区和任务栏

- 用户要的是“屏幕正下方、任务栏上方”，不是左下角，也不是简单的屏幕底边。
- Windows 上优先用工作区 `SPI_GETWORKAREA` 算 HUD 的基准位置，再叠加可调偏移参数。

### 参数化如果只是写进配置结构但没接到窗口实现，等于没做

- 之前已经把 `HudOverlayConfig` 写进了配置层，但 `overlay.rs` 仍然使用旧常量，最终效果完全不会变。
- 下次只要做 UI 参数化，必须同时核对：字体、颜色、对齐、宽高、padding、圆角、锚点、透明度都是否真的参与窗口创建和布局计算。

### HUD 配置文件的写法和读法不能各写各的

- 之前 `render_hud_overlay_config_file()` 把文档写成了 `[layout] / [font] / [background]` 三个表，但读取时却直接按平铺字段反序列化，结果用户改的大部分值都被静默吃掉，最后看起来像“完全没生效”。
- 以后只要改配置文件结构，必须同步加一条“写出来再读回去”的回归测试，不能只看渲染后的文档长得像对的。

### 旧坏格式配置要做迁移兼容，不能假设用户手里的文件总是干净的

- 这个项目已经发过会把键值挤进注释行尾的坏版 `hud-overlay.toml`；表面上还能打开，但 TOML 会把整行都当注释，导致 `font_height_px`、`width_px` 这类值完全读不到。
- 修复新代码时，不能只让“新生成的文件”正确，还要能把用户磁盘上已经留下来的坏文件抢救并重写。

### 运行时根目录判断不能把 `AGENTS.md` 当项目标记

- Windows 用户主目录本身就可能有 `AGENTS.md`，如果把它当成项目根标记，便携版从别的工作目录启动时就会去读错 `config`，用户会以为“我改了参数却没反应”。
- 运行时根目录判断必须优先依赖真正稳定的项目 / 运行时标记，不能用过于宽泛的人类协作文档文件名。

### 热加载路径必须只读，不能一边监听一边自写回

- 之前 HUD 热加载每次检测到文件变化后，又走了会重写 `hud-overlay.toml` 的加载路径，结果程序自己不停刷新文件修改时间，形成约 `250ms` 一次的自激重载和肉眼闪烁。
- 以后只要做“监听配置文件变化”的功能，必须单独检查热加载路径里有没有任何写文件动作。

### 首次启动异常，优先怀疑 worker / recognizer 仍在懒加载

- 这个项目的首帧体验很容易被模型首次加载拖慢。
- 如果用户说“第一次不正常，后面就好了”，优先考虑启动时预热当前模式对应的 worker，而不是只盯录音或 HUD。

### 真流式能力已经在 ASR 层封装好时，不要继续在桌面层伪装“流式”

- 这次排查里，`crates/ainput-asr` 早就有 `StreamingZipformerRecognizer` 和 `StreamingZipformerStream`，真正的问题是桌面流式 worker 还在用离线 `SenseVoiceRecognizer` 对累计音频反复整段重跑。
- 以后只要出现“HUD 已经出字，但松手后还要再等一段”的现象，先核对桌面层是否真的在消费在线流接口，而不是继续优化假流式预览。

### HUD 上已经看到的文字，松手后不能再走一遍整段离线 ASR

- 对“按住说话、松手整段提交”的模式来说，松手后的正确收尾应该是在线流 `input_finished + drain`，而不是把全部累计音频再次送去做全量转写。
- 如果松手后又做一次整段 ASR，体感上一定会出现“HUD 有字但最终上屏还要等”的落差，后面再怎么调小粘贴等待也只是治标。

### 上屏延迟要拆成多段 timing 看，不能只看一个总耗时

- 这条链路至少要拆开看 `final_drain`、`rewrite`、`hotkey_release_wait`、`output` 四段；否则会把 ASR 收尾、热键释放等待和粘贴稳定等待混在一起，误判真正的慢点。
- 以后只要用户继续反馈“HUD 已经好了但上屏还慢”，优先看日志里的分段 timing，再决定是继续压输出链路，还是流式模型收尾本身还不够快。

### 在线 transducer 不能直接长期吃“任意碎片麦克风样本”然后指望稳定长句输出

- 这次复发时，日志已经明确显示：整段录音有 4 到 15 秒，但在线结果长期只有 `这招`、`喂你好` 这类两三个字，`final_decode_steps` 也几乎是 0。
- 处理这类模型时，优先按固定块喂流，再在松手收尾时补尾部静音 padding；不要把几十毫秒级的零碎样本每次都直接喂进去后就拿 `get_result()` 当完整趋势。

### 在线 final 比 HUD 已稳定文本更短时，不能盲信 final 覆盖整句

- 真流式模型在松手 `drain` 后，`final` 结果不一定总比最后一个 HUD 文本更完整。
- 如果 `final` 明显比 HUD 已稳定文本短，直接拿它覆盖会让用户感知成“HUD 明明已经对了，提交时又倒退了”。
- 更稳的做法是：
  1. 先保留状态机里已经稳定的 `display_text`
  2. 再把在线 `final candidate` 和它做长度/前缀对比
  3. 只有 `final` 不明显短缺时，才让它接管最终提交文本

### 流式模式不能把“在线 final 恰好够长”当成唯一提交依据

- 这次最顽固的问题，是在线流在真实口述里经常只给出前几个字；即使 HUD 已经看起来像在工作，松手后的 `online_raw_text` 依然可能是残句。
- 对 `ainput` 这种“按住说话、松手整段提交”的产品形态，最终提交必须优先稳定完整，而不是死守纯在线 final。
- 更稳的做法是：
  1. HUD 继续优先吃在线 partial。
  2. 一旦在线 partial 明显偏短，按节流频率触发同模型整段 preview rescue。
  3. 松手最终提交统一走同一个 streaming 模型的整段 rescore，再做改写和上屏。

### 流式链路一定要留固定 wav 回归脚本，不能每次都靠人肉口述复现

- 这个问题之所以反复，就是因为“说一长段话只出前几个字”之前没有自动化门槛，只能靠用户一次次口头复现。
- 至少要保留一条能直接运行的回归脚本，固定跑 streaming 模型自带长 wav，并检查输出字符数不再退化到老 bug 的 2 到 3 个字级别。
- 以后只要改流式 worker、preview 节流、final 提交链路或模型接线，先跑这条脚本，再让用户上手。

### 只在“在线 partial 看起来太短”时才做整段补救，门槛还是太弱

- 这次又复发的关键点，不是“整段补救完全没有”，而是补救被放在在线 partial 之后，并且还要先满足“看起来太短”这个条件。
- 一旦在线 `get_result()` 长时间为空，或者短到还没过那道门槛，整段补救根本不会执行，HUD 就会继续卡在前几个字。
- 更稳的做法是：HUD 每个预览周期都直接跑一次同模型整段重识别，再和在线结果择优；不要让整段预览依赖在线 partial 先成功。

### 从 Linux 给 Windows 打预览版，不要再依赖远端 Windows SSH 的 cargo 环境

- 这次 Windows 真机源码是对的，但远端 SSH 构建环境先后踩了 `cargo.exe` 关联异常、`os error 448`、大小写 `.lib` 文件名等一串非业务问题。
- 稳定做法是：
  1. Linux 侧用 `cargo xwin` 交叉编译 Windows 目标。
  2. `SHERPA_ONNX_LIB_DIR` 显式指向项目里的 Windows 静态库目录。
  3. 首次准备好 xwin CRT / SDK 后，后续直接在本机产出 exe，再拷回 Windows 打包。

### 打发布包时，`release build` 和 `package` 不能并行跑

- 这次收口里一度出现过“源码已经编到新版本，但包内 exe 还是旧哈希”的问题，根因是并行执行了 `cargo build --release` 和 `package-release.ps1`，打包脚本先拷走了旧的 `target/release/ainput-desktop.exe`。
- 以后只要涉及发布包，顺序必须固定为：先完成 `release build`，再串行执行打包，然后核对包内 exe 和 `target/release` 的哈希一致。

## Windows 构建链路

### `C:\Users\sai\.cargo\bin` 里的 rustup 代理 exe 不能盲信符号链接版

- 这次 Windows 真机的 `cargo.exe`、`rustc.exe`、`rustfmt.exe` 表面上都在 PATH 里，`where cargo` 也能找到，但真正执行时会报 “No application is associated with the specified file for this operation” 或 “The system cannot execute the specified program.”
- 根因不是 PATH，也不是 `.exe` 文件关联，而是这些代理文件被装成了指向 `rustup.exe` 的符号链接；在这台机器上，这批 symlink exe 会被 PowerShell 识别成 `untrusted mount point`，结果根本不能执行。
- 真正有效的修法是把这些代理替换成真实的 `rustup.exe` 副本文件，再重新验证：
  - `cargo --version`
  - `rustc --version`
  - `cargo metadata --format-version 1 --no-deps`
  - `cargo build --release -p ainput-desktop`
- 以后如果 Windows 真机又出现“`rustup` 能跑但 `cargo` 不能跑”的情况，优先先查：
  1. `Get-Item C:\Users\sai\.cargo\bin\cargo.exe | Format-List FullName,Attributes,LinkType,Target`
  2. 是否还是 `SymbolicLink -> rustup.exe`
  3. 再决定要不要整批重建代理
