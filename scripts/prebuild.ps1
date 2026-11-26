# Translates every shader of the title's pack and makes every pipeline into the shader cache and
# the pipeline library that scripts\play.ps1 runs with, again and again until it ends: the AMD
# shader compiler on RDNA4 crashes at random, and the pre-build goes on where it stopped (a
# pipeline is left out only after it crashed the driver twice on its own).
#   scripts\prebuild.ps1 -Game <folder with your copy>
param(
    [Parameter(Mandatory)][string]$Game,
    [string]$Cache = "$env:LOCALAPPDATA\victory\cache-merged-amd",
    [int]$Jobs = 6,
    [int]$MaxRuns = 300
)
$root = Split-Path -Parent $PSScriptRoot
$victory = Join-Path $root 'target\release\victory.exe'
if (-not (Test-Path $victory)) { Write-Host "No $victory. Build it first (README.md)."; return }
$pack = Join-Path $Game 'shaders.rpf'
if (-not (Test-Path $pack)) { Write-Host "No shader pack in $Game."; return }
Get-ChildItem Env:VICTORY_* | ForEach-Object { Remove-Item "Env:$($_.Name)" }
# The translation depends on these: the same as scripts\play.ps1.
$env:VICTORY_SHADER_OPT = '0'
$skip = Join-Path $root 'docs\klbf\skip-amd.txt'
if (Test-Path $skip) { $env:VICTORY_SKIP_PIPELINES = (Get-Content $skip -Raw).Trim() }
New-Item -ItemType Directory -Force $Cache | Out-Null
$env:VICTORY_CACHE_DIR = $Cache
for ($i = 1; $i -le $MaxRuns; $i++) {
    & $victory translate --cache --pipelines --jobs $Jobs $pack
    if ($LASTEXITCODE -eq 0) { Write-Host "pre-build done after $i run(s)"; return }
    Write-Host ("run {0}: the driver crashed (0x{1:X8}); going on" -f $i, $LASTEXITCODE)
}
