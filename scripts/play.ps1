# Starts the title with the settings measured best on a Radeon RX 9070 XT (README.md).
#   scripts\play.ps1 [-Game <folder with your copy>] [-Console series-x|series-s]
# -Game         folder of your copy; when omitted it is read from "$env:LOCALAPPDATA\victory\merged-amd.json" (written by Install.cmd)
# -Panel        shows an on-screen frame-rate panel (VICTORY_PANEL=1)
# -RayTracing   builds the ray-tracing structures for the title's indirect light (VICTORY_BVH_BUILDER=1); EXPERIMENTAL
# -Fps30        leaves the frame interval setting out
# -More         other settings: -More @{VICTORY_AMBIENT='1.0'}
param(
    [string]$Game = "",
    [ValidateSet('series-x', 'series-s')][string]$Console = 'series-x',
    [switch]$Panel,
    [switch]$RayTracing,
    [switch]$Fps30,
    [string]$Cache = "$env:LOCALAPPDATA\victory\cache-merged-amd",
    [hashtable]$More = @{}
)
$root = Split-Path -Parent $PSScriptRoot
if (-not $Game) {
    $cfg = "$env:LOCALAPPDATA\victory\merged-amd.json"
    if (Test-Path -LiteralPath $cfg) {
        try { $Game = [string](Get-Content -LiteralPath $cfg -Raw | ConvertFrom-Json).game } catch { }
    }
    if (-not $Game) { Write-Host "No game folder. Run Install.cmd, or pass -Game <folder>."; exit 1 }
}
$exe = Get-ChildItem -LiteralPath $Game -Filter '*_scarlett_master.exe' -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $exe) { Write-Host "No title executable (*_scarlett_master.exe) in $Game."; exit 1 }
$victory = Join-Path $root 'target\diag\victory.exe'
if (-not (Test-Path -LiteralPath $victory)) { Write-Host "No $victory. Run Install.cmd first."; exit 1 }

# Other settings left over in this window go.
Get-ChildItem Env:VICTORY_* | ForEach-Object { Remove-Item "Env:$($_.Name)" }
$settings = & (Join-Path $PSScriptRoot 'settings.ps1') -Fps30:$Fps30
$settings['VICTORY_CACHE_DIR'] = $Cache
if ($Panel) { $settings['VICTORY_PANEL'] = '1' }
if ($RayTracing) { $settings['VICTORY_BVH_BUILDER'] = '1' }   # experimental
foreach ($k in $More.Keys) { $settings[$k] = [string]$More[$k] }
foreach ($k in $settings.Keys) { Set-Item -LiteralPath "Env:$k" -Value ([string]$settings[$k]) }

Set-Location -LiteralPath $root
& $victory run --console $Console $exe.FullName --execute
exit $LASTEXITCODE
