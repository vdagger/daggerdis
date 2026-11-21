# albus.ps1 — запуск и управление GUI Albus Audio из-под агента (DSH).
#
# Зачем: exe — консольное приложение, и при обычном Start-Process оно
# наследует stdout/stderr вызывающего шелла. DSH-терминал ждёт EOF на этом
# пайпе, поэтому вызов «висит», хотя окно уже открылось. Редирект stdio в
# файлы отвязывает приложение от пайпа — запуск возвращается за ~100 мс,
# а консольный вывод приложения остаётся в журнале в _ALBUS_AGENT_DIR.
#
# Каталог для скриншотов/логов: $env:TEMP\albus-agent
#
# Примеры:
#   pwsh -NoProfile -File tools\agent\albus.ps1 -Action start
#   pwsh -NoProfile -File tools\agent\albus.ps1 -Action shot
#   pwsh -NoProfile -File tools\agent\albus.ps1 -Action click -X 500 -Y 640
#   pwsh -NoProfile -File tools\agent\albus.ps1 -Action keys -Combo ctrl+m
#   pwsh -NoProfile -File tools\agent\albus.ps1 -Action keys -Text "привет"
#   pwsh -NoProfile -File tools\agent\albus.ps1 -Action stop

param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('start', 'stop', 'shot', 'click', 'keys', 'focus', 'resize', 'info')]
    [string]$Action,

    [int]$X = 40,
    [int]$Y = 40,
    [int]$Width = 1120,
    [int]$Height = 700,
    [string]$Text = '',
    [string]$Combo = '',
    [string]$Out = '',
    [switch]$Double,
    [switch]$NoKill,
    [switch]$Visible
)

$ErrorActionPreference = 'Stop'
$Repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Exe = Join-Path $Repo 'target\release\albus.exe'
if (-not $Out) { $Out = Join-Path $env:TEMP 'albus-agent' }

if (-not (Test-Path $Out)) { New-Item -ItemType Directory -Path $Out -Force | Out-Null }

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

if (-not ('AlbusWin' -as [type])) {
    Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class AlbusWin {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, UIntPtr extra);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
}
"@
}

function Get-AlbusWindow {
    Get-Process albus -ErrorAction SilentlyContinue |
        Where-Object { $_.MainWindowHandle -ne 0 } |
        Select-Object -First 1
}

function Restore-AlbusWindow {
    $p = Get-AlbusWindow
    if (-not $p) { throw 'Окно Albus не найдено — сначала -Action start' }
    [AlbusWin]::ShowWindow($p.MainWindowHandle, 9) | Out-Null   # SW_RESTORE
    # Windows не отдаёт foreground чужому процессу (foreground lock), поэтому
    # на миг поднимаем окно в TOPMOST: этого хватает, чтобы клик/скриншот
    # попали в Albus, а не в окно поверх него.
    [AlbusWin]::SetWindowPos($p.MainWindowHandle, [IntPtr](-1), 0, 0, 0, 0, 0x0001 -bor 0x0002 -bor 0x0040) | Out-Null
    [AlbusWin]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
    Start-Sleep -Milliseconds 120
    [AlbusWin]::SetWindowPos($p.MainWindowHandle, [IntPtr](-2), 0, 0, 0, 0, 0x0001 -bor 0x0002) | Out-Null
    Start-Sleep -Milliseconds 60
    return $p
}

function Test-AlbusShot {
    $p = Get-AlbusWindow
    if (-not $p) { throw 'Окно Albus не найдено — сначала -Action start' }
    [AlbusWin]::ShowWindow($p.MainWindowHandle, 9) | Out-Null
    # Оставляем TOPMOST на время съёмки и снимаем после неё: иначе окно DSH
    # успевает вернуться наверх между подъёмом и захватом экрана.
    [AlbusWin]::SetWindowPos($p.MainWindowHandle, [IntPtr](-1), 0, 0, 0, 0, 0x0001 -bor 0x0002 -bor 0x0040) | Out-Null
    Start-Sleep -Milliseconds 150
    return $p
}

function Undo-AlbusShot($p) {
    if ($p) {
        [AlbusWin]::SetWindowPos($p.MainWindowHandle, [IntPtr](-2), 0, 0, 0, 0, 0x0001 -bor 0x0002) | Out-Null
    }
}

function Send-Combo([string]$spec) {
    $map = @{
        'ctrl' = 0x11; 'shift' = 0x10; 'alt' = 0x12; 'win' = 0x5B
        'esc' = 0x1B; 'enter' = 0x0D; 'tab' = 0x09; 'space' = 0x20
        'left' = 0x25; 'up' = 0x26; 'right' = 0x27; 'down' = 0x28
    }
    $parts = $spec.ToLower().Split('+') | ForEach-Object { $_.Trim() } | Where-Object { $_ }
    $vks = foreach ($p in $parts) {
        if ($map.ContainsKey($p)) { $map[$p] }
        elseif ($p.Length -eq 1) { [int][char]::ToUpper($p) }
        elseif ($p -match '^f(\d+)$') { 0x6F + [int]$Matches[1] }
        else { throw "Неизвестная клавиша: $p" }
    }
    foreach ($vk in $vks) { [AlbusWin]::keybd_event([byte]$vk, 0, 0, [UIntPtr]::Zero) }
    Start-Sleep -Milliseconds 40
    [array]::Reverse($vks)
    foreach ($vk in $vks) {
        [AlbusWin]::keybd_event([byte]$vk, 0, 2, [UIntPtr]::Zero)   # KEYEVENTF_KEYUP
        Start-Sleep -Milliseconds 20
    }
}

function Save-Shot([string]$path) {
    $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

switch ($Action) {
    'start' {
        if (-not $NoKill) {
            Get-Process albus -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
            Start-Sleep -Milliseconds 400
        }
        $outLog = Join-Path $Out 'albus.out.log'
        $errLog = Join-Path $Out 'albus.err.log'
        $styles = if ($Visible) { 'Normal' } else { 'Hidden' }
        $sw = [Diagnostics.Stopwatch]::StartNew()
        # Ключ: редирект stdio — иначе приложение держит пайп тула и вызов висит.
        $p = Start-Process -FilePath $Exe -WorkingDirectory $Repo `
            -RedirectStandardOutput $outLog -RedirectStandardError $errLog `
            -WindowStyle $styles -PassThru
        $deadline = [DateTime]::Now.AddSeconds(25)
        $win = $null
        while ([DateTime]::Now -lt $deadline) {
            Start-Sleep -Milliseconds 250
            $win = Get-AlbusWindow
            if ($win) { break }
            if ($p.HasExited) { break }
        }
        if ($win) {
            # Окно только что создано и ещё раскладывается: первый SetWindowPos
            # может быть съеден winit'ом, поэтому добиваемся нужного прямоугольника
            # повтором, а не одним вызовом.
            $want = $false
            for ($i = 0; $i -lt 12; $i++) {
                [AlbusWin]::SetWindowPos($win.MainWindowHandle, [IntPtr]::Zero, $X, $Y, $Width, $Height, 0x0040) | Out-Null
                Start-Sleep -Milliseconds 120
                $r = New-Object AlbusWin+RECT
                [AlbusWin]::GetWindowRect($win.MainWindowHandle, [ref]$r) | Out-Null
                if ($r.Left -eq $X -and $r.Top -eq $Y -and ($r.Right - $r.Left) -eq $Width -and ($r.Bottom - $r.Top) -eq $Height) {
                    $want = $true
                    break
                }
            }
            [AlbusWin]::SetForegroundWindow($win.MainWindowHandle) | Out-Null
            Start-Sleep -Milliseconds 200
            "started pid=$($win.Id) window='$($win.MainWindowTitle)' ready_ms=$($sw.ElapsedMilliseconds) placed=$want"
        } else {
            "started pid=$($p.Id) но окно не появилось за 25 с; хвост лога:"
            if (Test-Path $outLog) { Get-Content $outLog -Tail 15 }
            if (Test-Path $errLog) { Get-Content $errLog -Tail 15 }
            exit 2
        }
    }
    'stop' {
        $before = @(Get-Process albus -ErrorAction SilentlyContinue).Count
        Get-Process albus -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
        "stopped=$before"
    }
    'focus' {
        $w = Restore-AlbusWindow
        "focused pid=$($w.Id)"
    }
    'resize' {
        $w = Restore-AlbusWindow
        [AlbusWin]::SetWindowPos($w.MainWindowHandle, [IntPtr]::Zero, $X, $Y, $Width, $Height, 0x0040) | Out-Null
        "resized pid=$($w.Id) -> $X,$Y ${Width}x${Height}"
    }
    'shot' {
        $w = Test-AlbusShot
        Start-Sleep -Milliseconds 200
        $path = Join-Path $Out ("shot-{0:HHmmss}.png" -f (Get-Date))
        Save-Shot $path
        $r = New-Object AlbusWin+RECT
        [AlbusWin]::GetWindowRect($w.MainWindowHandle, [ref]$r) | Out-Null
        Undo-AlbusShot $w
        "shot=$path window=$($r.Left),$($r.Top),$($r.Right),$($r.Bottom) pid=$($w.Id)"
    }
    'click' {
        Restore-AlbusWindow | Out-Null
        [AlbusWin]::SetCursorPos($X, $Y) | Out-Null
        Start-Sleep -Milliseconds 80
        [AlbusWin]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)   # LEFTDOWN
        Start-Sleep -Milliseconds 60
        [AlbusWin]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)   # LEFTUP
        if ($Double) {
            Start-Sleep -Milliseconds 60
            [AlbusWin]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
            Start-Sleep -Milliseconds 60
            [AlbusWin]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
        }
        "clicked $X,$Y double=$Double"
    }
    'keys' {
        Restore-AlbusWindow | Out-Null
        if ($Combo) {
            Send-Combo $Combo
            "combo=$Combo"
        }
        if ($Text) {
            # Через буфер обмена: SendKeys не умеет кириллицу.
            Set-Clipboard -Value $Text
            Send-Combo 'ctrl+v'
            "typed=$($Text.Length) chars"
        }
    }
    'info' {
        $w = Get-AlbusWindow
        if (-not $w) { 'albus не запущен'; exit 1 }
        $r = New-Object AlbusWin+RECT
        [AlbusWin]::GetWindowRect($w.MainWindowHandle, [ref]$r) | Out-Null
        "pid=$($w.Id) title='$($w.MainWindowTitle)' rect=$($r.Left),$($r.Top),$($r.Right),$($r.Bottom) responding=$($w.Responding)"
    }
}
