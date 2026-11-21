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
    [ValidateSet('start', 'stop', 'shot', 'click', 'keys', 'focus', 'resize', 'info', 'windows')]
    [string]$Action,

    # Какому экземпляру адресовано действие. 0 — первый найденный (одиночный
    # прогон). При нескольких узлах (group.ps1) без этого нельзя: Get-Process
    # возвращает окна в произвольном порядке.
    [int]$TargetPid = 0,

    # Выбор окна по заголовку внутри экземпляра: у Albus это 'Albus Audio'
    # (главное) и 'Albus — трансляция' (окно показа). MainWindowHandle после
    # открытия видео указывает уже на окно трансляции, и без фильтра клик
    # уходит не туда.
    [string]$WindowTitle = '',

    [int]$X = 40,
    [int]$Y = 40,
    [int]$Width = 1120,
    [int]$Height = 700,
    [string]$Text = '',
    [string]$Combo = '',
    [string]$Out = '',
    [switch]$Double,
    # -Client: X,Y — координаты внутри клиентской области окна (0,0 — левый
    # верхний угол содержимого), а не по экрану. Окно может уехать (eframe
    # пересоздаёт/сдвигает его), а раскладка внутри не зависит от позиции —
    # такие клики не промахиваются после resize.
    [switch]$Client,
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
    [StructLayout(LayoutKind.Sequential)]
    public struct POINT { public int X, Y; }
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, UIntPtr extra);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint attachFrom, uint attachTo, bool attach);
    [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);

    // Перечисление окон процесса. Get-Process.MainWindowHandle отдаёт ровно
    // одно окно, а у Albus их два: главное ('Albus Audio') и окно показа
    // ('Albus — трансляция'). Без списка нельзя адресовать нужное.
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int n);

    public static IntPtr[] WindowsOf(uint pid) {
        var list = new System.Collections.Generic.List<IntPtr>();
        EnumWindows(delegate(IntPtr h, IntPtr l) {
            if (!IsWindowVisible(h)) return true;
            uint p;
            GetWindowThreadProcessId(h, out p);
            if (p != pid) return true;
            var sb = new System.Text.StringBuilder(512);
            GetWindowTextW(h, sb, 512);
            if (sb.Length == 0) return true;
            list.Add(h);
            return true;
        }, IntPtr.Zero);
        return list.ToArray();
    }

    public static string TitleOf(IntPtr h) {
        var sb = new System.Text.StringBuilder(512);
        GetWindowTextW(h, sb, 512);
        return sb.ToString();
    }

    // Активация чужого окна. Одного SetForegroundWindow мало: Windows отдаёт
    // фокус только «своему» процессу (foreground lock), а при нескольких
    // экземплярах Albus клик уходил не туда — окно получало его, но оставалось
    // неактивным, и egui событие мыши не применял.
    // Порядок: Alt (отдаёт нам право на смену фокуса) → TOPMOST → BringWindowToTop
    // → SetForegroundWindow → проверка; если фокус всё ещё чужой —
    // AttachThreadInput к потоку текущего фокуса и повтор.
    public static bool Activate(IntPtr h) {
        ShowWindow(h, 9);                                     // SW_RESTORE
        keybd_event(0x12, 0, 0, UIntPtr.Zero);                // Alt down
        keybd_event(0x12, 0, 2, UIntPtr.Zero);                // Alt up
        System.Threading.Thread.Sleep(40);
        SetWindowPos(h, new IntPtr(-1), 0, 0, 0, 0, 0x0001 | 0x0002);  // HWND_TOPMOST
        BringWindowToTop(h);
        SetForegroundWindow(h);
        bool ok = GetForegroundWindow() == h;
        if (!ok) {
            uint pid;
            IntPtr fg = GetForegroundWindow();
            uint fgThread = GetWindowThreadProcessId(fg, out pid);
            uint myThread = GetCurrentThreadId();
            if (AttachThreadInput(myThread, fgThread, true)) {
                SetForegroundWindow(h);
                BringWindowToTop(h);
                AttachThreadInput(myThread, fgThread, false);
                ok = GetForegroundWindow() == h;
            }
        }
        SetWindowPos(h, new IntPtr(-2), 0, 0, 0, 0, 0x0001 | 0x0002);  // HWND_NOTOPMOST
        return ok;
    }
}
"@
}

function Get-AlbusWindows {
    foreach ($pr in @(Get-Process albus -ErrorAction SilentlyContinue)) {
        foreach ($h in [AlbusWin]::WindowsOf([uint32]$pr.Id)) {
            [pscustomobject]@{ Hwnd = $h; Pid = $pr.Id; Title = [AlbusWin]::TitleOf($h) }
        }
    }
}

function Get-AlbusWindow {
    $all = @(Get-AlbusWindows)
    if ($TargetPid -ne 0) {
        $all = @($all | Where-Object { $_.Pid -eq $TargetPid })
        if (-not $all) { throw "Экземпляр с pid=$TargetPid не найден (окон Albus: всего $(@(Get-AlbusWindows).Count))" }
    }
    if ($WindowTitle) {
        $hit = @($all | Where-Object { $_.Title -like "*$WindowTitle*" })
        if ($hit) { return $hit | Select-Object -First 1 }
        throw "Окно с заголовком '$WindowTitle' не найдено у pid=$TargetPid (есть: $($all.Title -join ', '))"
    }
    # По умолчанию — главное окно, а не окно трансляции.
    $main = @($all | Where-Object { $_.Title -notlike '*трансляция*' })
    if ($main) { return $main | Select-Object -First 1 }
    return $all | Select-Object -First 1
}

function Restore-AlbusWindow {
    $p = Get-AlbusWindow
    if (-not $p) { throw 'Окно Albus не найдено — сначала -Action start' }
    $script:LastFocused = [AlbusWin]::Activate($p.Hwnd)
    Start-Sleep -Milliseconds 150
    return $p
}

function Test-AlbusShot {
    $p = Get-AlbusWindow
    if (-not $p) { throw 'Окно Albus не найдено — сначала -Action start' }
    [void][AlbusWin]::Activate($p.Hwnd)
    # Оставляем TOPMOST на время съёмки и снимаем после неё: иначе окно DSH
    # успевает вернуться наверх между подъёмом и захватом экрана.
    [AlbusWin]::SetWindowPos($p.Hwnd, [IntPtr](-1), 0, 0, 0, 0, 0x0001 -bor 0x0002 -bor 0x0040) | Out-Null
    Start-Sleep -Milliseconds 150
    return $p
}

function Undo-AlbusShot($p) {
    if ($p) {
        [AlbusWin]::SetWindowPos($p.Hwnd, [IntPtr](-2), 0, 0, 0, 0, 0x0001 -bor 0x0002) | Out-Null
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
        $TargetPid = $p.Id     # дальше ищем окно именно этого процесса
        $win = $null
        while ([DateTime]::Now -lt $deadline) {
            Start-Sleep -Milliseconds 250
            try { $win = Get-AlbusWindow } catch { $win = $null }
            if ($win) { break }
            if ($p.HasExited) { break }
        }
        if ($win) {
            # Окно только что создано и ещё раскладывается: первый SetWindowPos
            # может быть съеден winit'ом, поэтому добиваемся нужного прямоугольника
            # повтором, а не одним вызовом.
            $want = $false
            for ($i = 0; $i -lt 12; $i++) {
                [AlbusWin]::SetWindowPos($win.Hwnd, [IntPtr]::Zero, $X, $Y, $Width, $Height, 0x0040) | Out-Null
                Start-Sleep -Milliseconds 120
                $r = New-Object AlbusWin+RECT
                [AlbusWin]::GetWindowRect($win.Hwnd, [ref]$r) | Out-Null
                if ($r.Left -eq $X -and $r.Top -eq $Y -and ($r.Right - $r.Left) -eq $Width -and ($r.Bottom - $r.Top) -eq $Height) {
                    $want = $true
                    break
                }
            }
            [AlbusWin]::SetForegroundWindow($win.Hwnd) | Out-Null
            Start-Sleep -Milliseconds 200
            "started pid=$($win.Pid) window='$($win.Title)' ready_ms=$($sw.ElapsedMilliseconds) placed=$want"
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
        "focused pid=$($w.Pid) hwnd=$($w.Hwnd) title='$($w.Title)'"
    }
    'resize' {
        $w = Restore-AlbusWindow
        [AlbusWin]::SetWindowPos($w.Hwnd, [IntPtr]::Zero, $X, $Y, $Width, $Height, 0x0040) | Out-Null
        "resized pid=$($w.Pid) -> $X,$Y ${Width}x${Height}"
    }
    'shot' {
        $w = Test-AlbusShot
        Start-Sleep -Milliseconds 200
        $path = Join-Path $Out ("shot-{0:HHmmss}.png" -f (Get-Date))
        Save-Shot $path
        $r = New-Object AlbusWin+RECT
        [AlbusWin]::GetWindowRect($w.Hwnd, [ref]$r) | Out-Null
        Undo-AlbusShot $w
        "shot=$path window=$($r.Left),$($r.Top),$($r.Right),$($r.Bottom) pid=$($w.Pid) title='$($w.Title)'"
    }
    'click' {
        $w = Restore-AlbusWindow
        # Свежий handle после активации: eframe может пересоздать окно, и
        # старый hwnd тогда указывает в никуда.
        $w = Get-AlbusWindow
        $r = New-Object AlbusWin+RECT
        [AlbusWin]::GetWindowRect($w.Hwnd, [ref]$r) | Out-Null
        $sx = $X; $sy = $Y
        if ($Client) {
            # Точка отсчёта — левый верхний угол клиентской области (без рамки
            # и заголовка), а не окна: раскладка egui считается от неё.
            $origin = New-Object AlbusWin+POINT
            [AlbusWin]::ClientToScreen($w.Hwnd, [ref]$origin) | Out-Null
            $sx = $origin.X + $X; $sy = $origin.Y + $Y
        }
        [AlbusWin]::SetCursorPos($sx, $sy) | Out-Null
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
        "clicked screen=$sx,$sy client=$($Client.IsPresent) focused=$($script:LastFocused) rect=$($r.Left),$($r.Top),$($r.Right),$($r.Bottom)"
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
    'windows' {
        foreach ($w in Get-AlbusWindows) {
            $r = New-Object AlbusWin+RECT
            [AlbusWin]::GetWindowRect($w.Hwnd, [ref]$r) | Out-Null
            "pid=$($w.Pid) hwnd=$($w.Hwnd) title='$($w.Title)' rect=$($r.Left),$($r.Top),$($r.Right),$($r.Bottom)"
        }
    }
    'info' {
        $w = Get-AlbusWindow
        if (-not $w) { 'albus не запущен'; exit 1 }
        $r = New-Object AlbusWin+RECT
        [AlbusWin]::GetWindowRect($w.Hwnd, [ref]$r) | Out-Null
        "pid=$($w.Pid) title='$($w.Title)' rect=$($r.Left),$($r.Top),$($r.Right),$($r.Bottom)"
    }
}
