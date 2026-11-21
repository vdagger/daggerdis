# build.ps1 — сборка/проверки Albus без пайпов (в песочнице DSH они падают).
#
# Вывод cargo всегда идёт в файл, а скрипт печатает ошибки, итоги тестов
# и хвост журнала:
#   pwsh -NoProfile -File tools\agent\build.ps1 -Mode check
#   pwsh -NoProfile -File tools\agent\build.ps1 -Mode test
#   pwsh -NoProfile -File tools\agent\build.ps1 -Mode clippy
#   pwsh -NoProfile -File tools\agent\build.ps1 -Mode release
#
# Журналы: $env:TEMP\albus-agent\<mode>-<HHmmss>.log

param(
    [ValidateSet('check', 'test', 'clippy', 'release', 'build')]
    [string]$Mode = 'check'
)

$ErrorActionPreference = 'Stop'
$Repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$LogDir = Join-Path $env:TEMP 'albus-agent'
if (-not (Test-Path $LogDir)) { New-Item -ItemType Directory -Path $LogDir -Force | Out-Null }

$cargoArgs = switch ($Mode) {
    'check'   { 'check --all-targets' }
    'test'    { 'test' }
    'clippy'  { 'clippy --all-targets -- -D warnings' }
    'release' { 'build --release' }
    'build'   { 'build' }
}

$log = Join-Path $LogDir ("{0}-{1:HHmmss}.log" -f $Mode, (Get-Date))
cmd /c "cd /d $Repo && cargo $cargoArgs > `"$log`" 2>&1"
$code = $LASTEXITCODE

Write-Output "mode=$Mode log=$log exit=$code"
$errors = Select-String -Path $log -Pattern '^error(\[|:)' -ErrorAction SilentlyContinue
if ($errors) {
    Write-Output '--- errors ---'
    $errors | ForEach-Object { $_.Line }
}
if ($Mode -in @('test', 'check', 'clippy')) {
    Select-String -Path $log -Pattern 'test result:|^warning: `albus`' -ErrorAction SilentlyContinue |
        ForEach-Object { $_.Line }
}
Write-Output '--- tail ---'
Get-Content $log -Tail 8
exit $code
