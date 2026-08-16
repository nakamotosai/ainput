$ws = New-Object -ComObject WScript.Shell
$lnk = $ws.CreateShortcut([Environment]::GetFolderPath('Desktop') + '\ainput.lnk')
$lnk.TargetPath = 'F:\ainput\ainput.exe'
$lnk.WorkingDirectory = 'F:\ainput'
$lnk.IconLocation = 'F:\ainput\ainput.exe,0'
$lnk.Description = 'ainput v0.1.3'
$lnk.Save()
Write-Host ('Shortcut created at: ' + [Environment]::GetFolderPath('Desktop') + '\ainput.lnk')
