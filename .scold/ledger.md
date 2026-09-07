# scold ledger — F:\ainput

## 2026-08-25 · policy:user-visible-asset-unverified (count=1)

**场景**：把运行实例从 `dist\ainput-0.1.2-win64\` 切到 `target\release\ainput.exe` 后，用户发现托盘 logo 消失（回退成系统通用图标）。

**Bad**
- 换了部署形态（dist 绿包 → dev 构建目录）却没验证任何用户可见面（托盘图标/HUD/模型加载），多次启动测试都带病运行
- `load_runtime_icon()` 只认 `<exe目录>/assets/app.ico`，dev 目录没有该文件即静默降级；而 build.rs 明明已把 app.ico 嵌进 exe 资源，代码却不用

**Good / 下次强制**
- 运行位置/部署形态变更 = 视同新交付物，托盘图标、HUD、模型加载等用户可见面必须逐项过一遍再报完成
- 图标加载现在有内嵌资源回退链：sidecar `assets/app.ico` → exe 内嵌 ICON(id=1) → 系统默认

**升级到哪**
- 结构性修复（Kill）：`src/tray.rs#load_tray_icon` 增加 `load_embedded_icon()` 回退——sidecar 缺失时从 exe 内嵌资源加载，同因物理性不可复发
- 项目 AGENTS.md 加规则一行（见该文件）

**复验方法**
- 悬停托盘图标看是否为 ainput logo（非白色通用图标）
- 删除 `target/release/assets/app.ico` 再重启，图标仍应正常（走内嵌资源）
