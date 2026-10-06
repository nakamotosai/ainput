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

## 附：代码签名（已内置，有证书即生效）

打包脚本已内置签名步骤 `scripts/sign-if-available.ps1`：**有证书就签，没有就跳过**（不影响打包）。

拿到 EV/OV 代码签名证书后，任选一种方式启用，然后照常 `make-portable.ps1` + `build-installer.ps1`：

- **PFX 文件**：设环境变量
  ```powershell
  $env:AINPUT_SIGN_PFX = "C:\path\cert.pfx"
  $env:AINPUT_SIGN_PFX_PASS = "<password>"
  ```
- **证书已导入本机**：把证书装进 `Cert:\CurrentUser\My`（带私钥），脚本自动选用。

脚本会对 `ainput.exe` 和最终 `setup.exe` 都签名，并用 DigiCert 时间戳（签名在证书过期后仍有效），签完 `signtool verify` 自检。签名后重新上传 GitHub/HF 资产即可。

> 当前发布未签名 → 用户首次运行可能见 SmartScreen「已保护你的电脑」。
