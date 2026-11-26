# Makes ahead the pipelines that earlier play sessions drew with (resumable).
#   scripts\warm.ps1 [-Cache <folder>]
param(
    [string]$Cache = "$env:LOCALAPPDATA\victory\cache-merged-amd"
)
$root = Split-Path -Parent $PSScriptRoot
$victory = Join-Path $root 'target\diag\victory.exe'
if (-not (Test-Path -LiteralPath $victory)) { Write-Host "No $victory. Run Install.cmd first."; exit 1 }

Get-ChildItem Env:VICTORY_* | ForEach-Object { Remove-Item "Env:$($_.Name)" }
$settings = & (Join-Path $PSScriptRoot 'settings.ps1')
$settings['VICTORY_CACHE_DIR'] = $Cache
foreach ($k in $settings.Keys) { Set-Item -LiteralPath "Env:$k" -Value ([string]$settings[$k]) }

Set-Location -LiteralPath $root
& $victory warm --used
exit $LASTEXITCODE
