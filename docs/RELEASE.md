# ainput 发布手册

对外发布走**双方案**：便携绿包（zip）+ Windows 安装包（setup.exe），三通道分发：
GitHub Releases、Hugging Face 镜像、官网 https://input.saaaai.com/。

## 0 · 前置

- `gh` 已登录（`gh auth status`）
- `hf` 已登录（`hf auth whoami`）
- Inno Setup 6 已装（`C:\Program Files (x86)\Inno Setup 6\ISCC.exe`）
- 站点仓库 `cnjpinput` 可推

## 1 · 定版本号

改 `Cargo.toml` 的 `version`（正式版不带 `-preview`，如 `0.2.0`）。所有打包脚本自动从
Cargo.toml 派生版本号，无需改其它文件。

## 2 · 构建 + 测试

```powershell
cargo build --release
cargo test
```

## 3 · 打两个包

```powershell
# 便携绿包 -> dist\ainput-<ver>-win64.zip
.\scripts\make-portable.ps1 -Overwrite

# Windows 安装包 -> dist\ainput-<ver>-setup.exe（从便携包编译）
.\scripts\build-installer.ps1
```

产物：
- `dist\ainput-<ver>-win64.zip` — 解压即用，含模型，无 `state/`
- `dist\ainput-<ver>-setup.exe` — 每用户安装，含开始菜单/可选开机自启/卸载器

## 4 · 算校验和

```powershell
Get-FileHash .\dist\ainput-<ver>-win64.zip  -Algorithm SHA256
Get-FileHash .\dist\ainput-<ver>-setup.exe  -Algorithm SHA256
```

## 5 · 发 GitHub Release

```bash
gh release create v<ver> \
  "dist/ainput-<ver>-win64.zip" \
  "dist/ainput-<ver>-setup.exe" \
  --repo nakamotosai/ainput \
  --title "ainput <ver>" \
  --notes-file docs/release-notes-<ver>.md
```

## 6 · 传 Hugging Face 镜像

```bash
hf upload nakamotosai/cnjp-input "dist/ainput-<ver>-win64.zip" "ainput-<ver>-win64.zip"
hf upload nakamotosai/cnjp-input "dist/ainput-<ver>-setup.exe" "ainput-<ver>-setup.exe"
```

## 7 · 更新官网

改站点仓库的 `content.ts`：版本号、下载链接（GitHub + HF）、大小文案。
提交并推送 `cnjpinput` 仓库（触发站点自动部署）。**注意：官网是另一台机器/仓库的自动部署，
推送前先确认部署方式。**

## 8 · 收尾核对

- [ ] `dist\` 两个产物都在，SHA256 已记
- [ ] GitHub release 资产可下载
- [ ] HF 镜像已更新
- [ ] 官网显示新版本号 + 新下载链接
- [ ] 仓库工作树干净（`git status`）

## 附：签名（当前未做）

exe 未做 Authenticode 签名 → 用户下载会撞 SmartScreen。拿到 EV/OV 代码签名证书后：
在打包脚本里加 `signtool sign /fd SHA256 /f <cert.pfx> /p <pw> /tr <ts> ainput.exe`，
对 `ainput.exe` 与最终 `setup.exe` 都签，再走上面流程。
