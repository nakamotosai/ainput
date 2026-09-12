$ws = New-Object -ComObject WScript.Shell
$lnk = $ws.CreateShortcut([Environment]::GetFolderPath('Desktop') + '\ainput.lnk')
$lnk.TargetPath = 'F:\projects\ainput\ainput.exe'
$lnk.WorkingDirectory = 'F:\projects\ainput'
$lnk.IconLocation = 'F:\projects\ainput\ainput.exe,0'
$lnk.Description = 'ainput v0.1.20-preview'
$lnk.Save()
Write-Host ('Shortcut created at: ' + [Environment]::GetFolderPath('Desktop') + '\ainput.lnk')
