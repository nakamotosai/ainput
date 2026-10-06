; Inno Setup script for ainput — Windows installer (per-user, no admin needed).
; Build: iscc /DAppVersion=0.2.0 /DSourceDir=<portable folder> scripts\ainput.iss
; Produces a per-user install under %LOCALAPPDATA%\Programs\ainput with Start Menu
; entry, optional run-at-login, and a clean uninstaller.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\dist\ainput-win64"
#endif

[Setup]
AppId={{8F2B7C1E-4A6D-4E2B-9C3F-1A2B3C4D5E6F}
AppName=ainput
AppVersion={#AppVersion}
AppPublisher=nakamotosai
AppPublisherURL=https://input.saaaai.com/
AppSupportURL=https://github.com/nakamotosai/ainput
AppUpdatesURL=https://github.com/nakamotosai/ainput/releases
DefaultDirName={autopf}\ainput
DefaultGroupName=ainput
DisableProgramGroupPage=yes
; Per-user install by default (no admin prompt); a "for all users" mode is offered.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir=..\dist
OutputBaseFilename=ainput-{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayName=ainput 语音听写
; Unsigned build: SmartScreen may warn on first run.

[Languages]
Name: "chinese"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务:"; Flags: unchecked
Name: "autostart"; Description: "开机自动启动 ainput"; GroupDescription: "附加任务:"; Flags: checkedonce

[Files]
; Ship the whole portable folder (exe + DLLs + models + config + assets).
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
Name: "{group}\ainput"; Filename: "{app}\ainput.exe"; WorkingDir: "{app}"
Name: "{group}\卸载 ainput"; Filename: "{uninstallexe}"
Name: "{autodesktop}\ainput"; Filename: "{app}\ainput.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Registry]
; Optional run-at-login (quoted path so spaces are safe). HKCU so no admin needed.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "ainput"; ValueData: """{app}\ainput.exe"""; Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{app}\ainput.exe"; Description: "立即启动 ainput"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; The app keeps runtime state next to the exe under state\ (logs/history/config).
; Leave user data on uninstall by default (safer); the uninstaller removes the
; program files it installed.
Type: filesandordirs; Name: "{app}\state"
