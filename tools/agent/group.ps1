<#
.SYNOPSIS
    Запуск нескольких экземпляров Albus с взаимной дружбой.

.DESCRIPTION
    Проверка групповой комнаты, голоса и трансляции требует нескольких живых
    узлов. Руками это дорого: каждому нужен свой %APPDATA% (свой secret.key и
    свой NodeID), и дружбу надо прописать до запуска — иначе приглашение
    отбрасывается как «от незнакомца».

    Скрипт делает это сам:
      1. для каждого узла создаёт отдельный каталог профиля;
      2. если NodeID ещё неизвестен — запускает экземпляр на пару секунд,
         читает NodeID из журнала и останавливает;
      3. пишет всем узлам friends.json с остальными участниками;
      4. запускает все экземпляры с редиректом stdio (иначе терминал ждёт EOF)
         и печатает pids и NodeID.

    Журналы: %TEMP%\albus-agent\group\p<i>.out.log / .err.log
    Профили: %TEMP%\albus-agent\group\p<i>\appdata\Albus

.EXAMPLE
    pwsh -NoProfile -File tools\agent\group.ps1 -Action start -Count 3
    pwsh -NoProfile -File tools\agent\group.ps1 -Action info
    pwsh -NoProfile -File tools\agent\group.ps1 -Action stop
    pwsh -NoProfile -File tools\agent\group.ps1 -Action clean
#>
param(
    [ValidateSet('start', 'stop', 'clean', 'info')]
    [string]$Action = 'start',
    [int]$Count = 3,
    [string]$Exe = '',
    [int]$ReadySeconds = 12
)

$ErrorActionPreference = 'Stop'
$root = Join-Path $env:TEMP 'albus-agent\group'
$pidFile = Join-Path $root 'pids.txt'

if (-not $Exe) {
    # tools\agent -> tools -> корень репозитория -> target\release
    $Exe = Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'target\release\albus.exe'
}
if ($Action -ne 'clean' -and -not (Test-Path $Exe)) {
    throw "Нет сборки: $Exe — соберите release (build.ps1 -Mode release)"
}

function Get-Profile([int]$i) {
    $appdata = Join-Path $root "p$i\appdata"
    New-Item -ItemType Directory -Force -Path (Join-Path $appdata 'Albus') | Out-Null
    return $appdata
}

function Start-Node([int]$i, [string]$logSuffix = '') {
    $appdata = Get-Profile $i
    $out = Join-Path $root "p$i$logSuffix.out.log"
    $err = Join-Path $root "p$i$logSuffix.err.log"
    $old = $env:APPDATA
    $env:APPDATA = $appdata
    try {
        $p = Start-Process -FilePath $Exe -RedirectStandardOutput $out -RedirectStandardError $err `
            -WindowStyle Hidden -PassThru
    }
    finally {
        $env:APPDATA = $old
    }
    return @{ Id = $p.Id; Out = $out; Err = $err }
}

function Wait-NodeId([string]$out, [int]$seconds = 15) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path $out) {
            $m = Select-String -Path $out -Pattern 'NodeID:\s*([0-9a-f]{64})' | Select-Object -First 1
            if ($m) { return $m.Matches[0].Groups[1].Value }
        }
        Start-Sleep -Milliseconds 300
    }
    throw "NodeID не появился за $seconds с: $out"
}

function Save-Friends([string]$appdata, [hashtable]$names, [string]$self) {
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $list = @()
    foreach ($key in $names.Keys | Sort-Object) {
        if ($key -eq $self) { continue }
        $list += [pscustomobject]@{ node_id = $key; name = $names[$key]; added_at = $now }
    }
    $json = [pscustomobject]@{ friends = $list; incoming = @(); outgoing = @() }
    $path = Join-Path $appdata 'Albus\friends.json'
    $json | ConvertTo-Json -Depth 5 | Set-Content $path -Encoding utf8NoBOM
}

function Add-FriendlyName([hashtable]$names, [string]$nodeId, [int]$index) {
    # Ключи — NodeID; имена «Узел N» по порядку создания.
    $names[$nodeId] = "Узел $index"
}

switch ($Action) {
    'clean' {
        Get-Process albus -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
        Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
        "cleaned $root"
    }
    'stop' {
        $stopped = 0
        if (Test-Path $pidFile) {
            foreach ($id in (Get-Content $pidFile)) {
                $p = Get-Process -Id ([int]$id) -ErrorAction SilentlyContinue
                if ($p) { Stop-Process -Id $p.Id -Force; $stopped++ }
            }
            Remove-Item $pidFile -Force -ErrorAction SilentlyContinue
        }
        "stopped=$stopped"
    }
    'info' {
        foreach ($f in Get-ChildItem $root -Filter 'p*.out.log' -ErrorAction SilentlyContinue) {
            $build = (Select-String -Path $f.FullName -Pattern '\[СБОРКА\]' | Select-Object -Last 1).Line
            $node = (Select-String -Path $f.FullName -Pattern 'NodeID:\s*([0-9a-f]{64})' | Select-Object -First 1)
            $hostLine = (Select-String -Path $f.FullName -Pattern 'Хост комнаты' | Select-Object -Last 1).Line
            $peers = (Select-String -Path $f.FullName -Pattern 'Принято входящее P2P-подключение' | Measure-Object).Count
            "$($f.Name): accepted=$peers host='$hostLine'"
        }
        Get-Process albus -ErrorAction SilentlyContinue |
            Select-Object Id, StartTime | Format-Table -AutoSize | Out-String
    }
    'start' {
        New-Item -ItemType Directory -Force -Path $root | Out-Null
        Get-Process albus -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 500

        # 1. Узнаём NodeID каждого профиля (переиспользуем уже известные ключи:
        #    secret.key лежит в профиле, поэтому NodeID стабилен между запусками).
        $names = @{}
        $nodeIds = @{}
        for ($i = 1; $i -le $Count; $i++) {
            $appdata = Get-Profile $i
            # NodeID зависит только от secret.key: пробный запуск нужен один раз,
            # дальше ключ лежит в профиле и NodeID не меняется.
            $probe = Start-Node $i '.probe'
            try { $nodeId = Wait-NodeId $probe.Out } finally {
                Stop-Process -Id $probe.Id -Force -ErrorAction SilentlyContinue
            }
            Start-Sleep -Milliseconds 400
            $nodeIds[$i] = $nodeId
            Add-FriendlyName $names $nodeId $i
        }

        # 2. Всем — дружба со всеми.
        for ($i = 1; $i -le $Count; $i++) {
            Save-Friends (Get-Profile $i) $names $nodeIds[$i]
        }

        # 3. Запуск всех.
        $pids = @()
        for ($i = 1; $i -le $Count; $i++) {
            $n = Start-Node $i
            $pids += $n.Id
            "p$i pid=$($n.Id) node=$($nodeIds[$i])"
        }
        $pids | Set-Content $pidFile
        Start-Sleep -Seconds $ReadySeconds
        "logs: $root"
    }
}

