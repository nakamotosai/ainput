param(
    [int]$DelayMs = 1000,
    [switch]$Interference
)

$ErrorActionPreference = "Stop"

$desktop = [Environment]::GetFolderPath("Desktop")
$testFile = Join-Path $desktop "ainput_fake_revision_test.txt"
$logFile = Join-Path $desktop "ainput-fake-revision-notepad-test.log"

$wrongText = "Oh, that's Codex"
$fixedText = "Codex Codex"

function Write-Step {
    param([string]$Message)
    $line = "{0:yyyy-MM-dd HH:mm:ss.fff} {1}" -f (Get-Date), $Message
    Write-Host $line
    Add-Content -Path $logFile -Encoding UTF8 -Value $line
}

trap {
    try {
        Write-Step ("ERROR: " + $_.Exception.Message)
    } catch {
        Write-Host ("ERROR: " + $_.Exception.Message)
    }
    exit 1
}

if (-not ([System.Management.Automation.PSTypeName]"Win32FakeRevision").Type) {
    Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;

public static class Win32FakeRevision
{
    public const int WM_GETTEXT = 0x000D;
    public const int WM_GETTEXTLENGTH = 0x000E;
    public const int WM_SETTEXT = 0x000C;
    public const int EM_SETSEL = 0x00B1;
    public const int EM_REPLACESEL = 0x00C2;
    public const int SW_SHOW = 5;

    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool EnumChildWindows(IntPtr hWndParent, EnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetClassName(IntPtr hWnd, StringBuilder lpClassName, int nMaxCount);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowTextLength(IntPtr hWnd);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowText(IntPtr hWnd, StringBuilder lpString, int nMaxCount);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "SendMessageW")]
    public static extern IntPtr SendMessage(IntPtr hWnd, int Msg, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "SendMessageW")]
    public static extern IntPtr SendMessageString(IntPtr hWnd, int Msg, IntPtr wParam, string lParam);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "SendMessageW")]
    public static extern IntPtr SendMessageStringBuilder(IntPtr hWnd, int Msg, IntPtr wParam, StringBuilder lParam);

    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
}
"@
}

function Get-WindowClassName {
    param([IntPtr]$Hwnd)
    $buffer = New-Object System.Text.StringBuilder 256
    $count = [Win32FakeRevision]::GetClassName($Hwnd, $buffer, $buffer.Capacity)
    if ($count -le 0) {
        return ""
    }
    return $buffer.ToString()
}

function Get-TopLevelWindowTitle {
    param([IntPtr]$Hwnd)
    $length = [Win32FakeRevision]::GetWindowTextLength($Hwnd)
    if ($length -le 0) {
        return ""
    }
    $buffer = New-Object System.Text.StringBuilder ($length + 2)
    [Win32FakeRevision]::GetWindowText($Hwnd, $buffer, $buffer.Capacity) | Out-Null
    return $buffer.ToString()
}

function Get-TopLevelWindowList {
    $items = New-Object System.Collections.Generic.List[object]
    $callback = [Win32FakeRevision+EnumWindowsProc]{
        param([IntPtr]$hwnd, [IntPtr]$lParam)
        $windowProcessId = [uint32]0
        [Win32FakeRevision]::GetWindowThreadProcessId($hwnd, [ref]$windowProcessId) | Out-Null
        $processName = ""
        try {
            if ($windowProcessId -ne 0) {
                $processName = (Get-Process -Id $windowProcessId -ErrorAction Stop).ProcessName
            }
        } catch {
            $processName = ""
        }
        $items.Add([pscustomobject]@{
            Hwnd = $hwnd
            ClassName = Get-WindowClassName -Hwnd $hwnd
            Title = Get-TopLevelWindowTitle -Hwnd $hwnd
            Visible = [Win32FakeRevision]::IsWindowVisible($hwnd)
            Pid = $windowProcessId
            ProcessName = $processName
        }) | Out-Null
        return $true
    }
    [Win32FakeRevision]::EnumWindows($callback, [IntPtr]::Zero) | Out-Null
    return $items
}

function Find-NotepadTopLevelWindow {
    param([string]$ExpectedFileName)

    $lastSummary = ""
    for ($i = 0; $i -lt 80; $i++) {
        Start-Sleep -Milliseconds 100
        $windows = Get-TopLevelWindowList | Where-Object { $_.Visible -and ($_.Title -or $_.ProcessName -match "(?i)notepad") }
        $candidate = $windows |
            Where-Object {
                $_.Title -like "*$ExpectedFileName*" -or
                (($_.Title -match "(?i)notepad") -and ($_.ProcessName -match "(?i)notepad"))
            } |
            Select-Object -First 1

        if ($candidate) {
            Write-Step ("top-level target: hwnd={0} pid={1} process={2} class={3} title=[{4}]" -f $candidate.Hwnd, $candidate.Pid, $candidate.ProcessName, $candidate.ClassName, $candidate.Title)
            return [IntPtr]$candidate.Hwnd
        }

        $lastSummary = (($windows | Select-Object -First 12 | ForEach-Object {
            "{0}/{1}/[{2}]" -f $_.Pid, $_.ProcessName, $_.Title
        }) -join ", ")
    }

    Write-Step "top-level windows seen: $lastSummary"
    throw "FAIL: notepad top-level window was not found"
}

function Get-ChildWindowList {
    param([IntPtr]$Root)
    $items = New-Object System.Collections.Generic.List[object]
    $callback = [Win32FakeRevision+EnumWindowsProc]{
        param([IntPtr]$hwnd, [IntPtr]$lParam)
        $items.Add([pscustomobject]@{
            Hwnd = $hwnd
            ClassName = Get-WindowClassName -Hwnd $hwnd
        }) | Out-Null
        return $true
    }
    [Win32FakeRevision]::EnumChildWindows($Root, $callback, [IntPtr]::Zero) | Out-Null
    return $items
}

function Get-WindowTextRaw {
    param([IntPtr]$Hwnd)
    $length = [Win32FakeRevision]::SendMessage($Hwnd, [Win32FakeRevision]::WM_GETTEXTLENGTH, [IntPtr]::Zero, [IntPtr]::Zero).ToInt64()
    $capacity = [Math]::Max([int]$length + 2, 4096)
    $buffer = New-Object System.Text.StringBuilder $capacity
    [Win32FakeRevision]::SendMessageStringBuilder($Hwnd, [Win32FakeRevision]::WM_GETTEXT, [IntPtr]$capacity, $buffer) | Out-Null
    return $buffer.ToString()
}

function Find-EditableChild {
    param([IntPtr]$Root)
    $children = Get-ChildWindowList -Root $Root
    Write-Step ("child windows: " + (($children | ForEach-Object { "{0}:{1}" -f $_.Hwnd, $_.ClassName }) -join ", "))

    $candidate = $children |
        Where-Object { $_.ClassName -match "(?i)(richedit|richeditd2d)" } |
        Select-Object -First 1

    if (-not $candidate) {
        $candidate = $children |
            Where-Object { $_.ClassName -match "(?i)(^edit$|text|textbox)" } |
            Select-Object -First 1
    }

    if (-not $candidate) {
        throw "FAIL: no Edit/RichEdit-like child window found"
    }

    return [IntPtr]$candidate.Hwnd
}

function Set-NativeText {
    param(
        [IntPtr]$EditHwnd,
        [string]$Text
    )
    [Win32FakeRevision]::SendMessageString($EditHwnd, [Win32FakeRevision]::WM_SETTEXT, [IntPtr]::Zero, $Text) | Out-Null
    $len = $Text.Length
    [Win32FakeRevision]::SendMessage($EditHwnd, [Win32FakeRevision]::EM_SETSEL, [IntPtr]$len, [IntPtr]$len) | Out-Null
}

function Replace-NativeRange {
    param(
        [IntPtr]$EditHwnd,
        [int]$Start,
        [int]$End,
        [string]$Text
    )
    [Win32FakeRevision]::SendMessage($EditHwnd, [Win32FakeRevision]::EM_SETSEL, [IntPtr]$Start, [IntPtr]$End) | Out-Null
    [Win32FakeRevision]::SendMessageString($EditHwnd, [Win32FakeRevision]::EM_REPLACESEL, [IntPtr]1, $Text) | Out-Null
}

function Invoke-FakeRevision {
    param(
        [IntPtr]$EditHwnd,
        [string]$Original,
        [string]$Replacement
    )

    $current = Get-WindowTextRaw -Hwnd $EditHwnd
    Write-Step "before revision readback: [$current]"

    if ($current -ne $Original) {
        Write-Step "ABORTED_SAFE: current text no longer exactly matches original text"
        return "ABORTED_SAFE"
    }

    $start = $current.IndexOf($Original, [StringComparison]::Ordinal)
    if ($start -lt 0) {
        Write-Step "ABORTED_SAFE: original text not found"
        return "ABORTED_SAFE"
    }

    $end = $start + $Original.Length
    Write-Step "native range replace: start=$start end=$end replacement=[$Replacement]"
    Replace-NativeRange -EditHwnd $EditHwnd -Start $start -End $end -Text $Replacement

    Start-Sleep -Milliseconds 120
    $after = Get-WindowTextRaw -Hwnd $EditHwnd
    Write-Step "after revision readback: [$after]"

    if ($after -eq $Replacement) {
        return "PASS"
    }
    return "FAIL"
}

Set-Content -Path $logFile -Encoding UTF8 -Value "ainput fake revision notepad test"
Set-Content -Path $testFile -Encoding UTF8 -Value ""

Write-Step "launching notepad: $testFile"
$process = Start-Process -FilePath "$env:WINDIR\system32\notepad.exe" -ArgumentList "`"$testFile`"" -PassThru

$main = Find-NotepadTopLevelWindow -ExpectedFileName (Split-Path -Leaf $testFile)

[Win32FakeRevision]::ShowWindow($main, [Win32FakeRevision]::SW_SHOW) | Out-Null
[Win32FakeRevision]::SetForegroundWindow($main) | Out-Null
Start-Sleep -Milliseconds 400

$edit = Find-EditableChild -Root $main
Write-Step "editable hwnd: $edit class=$(Get-WindowClassName -Hwnd $edit)"

Write-Step "initial native set text: [$wrongText]"
Set-NativeText -EditHwnd $edit -Text $wrongText
Start-Sleep -Milliseconds 200
$initial = Get-WindowTextRaw -Hwnd $edit
Write-Step "initial readback: [$initial]"

Start-Sleep -Milliseconds $DelayMs

if ($Interference) {
    Write-Step "interference mode: simulating user-side text mutation before revision"
    Set-NativeText -EditHwnd $edit -Text ($wrongText + " abc")
    Start-Sleep -Milliseconds 120
}

$result = Invoke-FakeRevision -EditHwnd $edit -Original $wrongText -Replacement $fixedText
Write-Step "RESULT: $result"

if ($result -eq "FAIL") {
    exit 2
}
if ($result -eq "ABORTED_SAFE") {
    exit 3
}
exit 0

