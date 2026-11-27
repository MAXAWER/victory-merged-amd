# Starts the title with the settings measured best on a Radeon RX 9070 XT (README.md).
#   scripts\play.ps1 [-Game <folder with your copy>] [-Console series-x|series-s]
# -Game         folder of your copy; when omitted it is read from "$env:LOCALAPPDATA\victory\merged-amd.json" (written by Install.cmd)
# -Panel        shows an on-screen frame-rate panel (VICTORY_PANEL=1)
# -RayTracing   builds the ray-tracing structures for the title's indirect light (VICTORY_BVH_BUILDER=1); works on an RX 9070 XT, about 14-15 fps in heavy scenes
# -Fps30        leaves the frame interval setting out
# -More         other settings: -More @{VICTORY_AMBIENT='1.0'}
# -VR           the headset mode over SteamVR (README.md, "VR"): the picture for each eye, hands, the controllers as the pad
# -VRMode       3d (the frame as a relief around you, from its depth buffer), cinema (a flat screen in the room), immersive (a wide screen fixed to the head)
# -VREmu        the headset mode with no headset: SteamVR's stand-in headset in a window and hands that move by themselves
param(
    [string]$Game = "",
    [ValidateSet('series-x', 'series-s')][string]$Console = 'series-x',
    [switch]$Panel,
    [switch]$RayTracing,
    [switch]$Fps30,
    [string]$Cache = "$env:LOCALAPPDATA\victory\cache-merged-amd",
    [hashtable]$More = @{},
    [switch]$VR,
    [ValidateSet('3d', 'cinema', 'immersive')][string]$VRMode = '3d',
    [switch]$VREmu
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
if ($RayTracing) { $settings['VICTORY_BVH_BUILDER'] = '1'; $settings['VICTORY_RAYS_MISS'] = '0' }   # rays really traced
if ($VR -or $VREmu) { $settings['VICTORY_VR'] = '1'; $settings['VICTORY_VR_MODE'] = $VRMode }
function Stop-SteamVR {
    Get-Process vrmonitor, vrdashboard, vrwebhelper, steamvr_room_setup, vrcompositor, vrserver -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
}
if ($VREmu) {
    # SteamVR with settings of its own, its null driver as the headset: your own SteamVR settings are
    # not touched. A SteamVR that is running would be the one joined, so it is closed first.
    if (Get-Process vrserver, vrmonitor, vrcompositor -ErrorAction SilentlyContinue) { Write-Host 'Closing the running SteamVR to start the stand-in headset.'; Stop-SteamVR }
    $emu = "$env:LOCALAPPDATA\victory\vr-emu"
    New-Item -ItemType Directory -Force -Path "$emu\logs" | Out-Null
    Copy-Item (Join-Path $PSScriptRoot 'vr-emu\steamvr.vrsettings') "$emu\steamvr.vrsettings" -Force
    Copy-Item (Join-Path $PSScriptRoot 'vr-emu\chaperone_info.vrchap') "$emu\chaperone_info.vrchap" -Force
    $settings['VICTORY_VR_EMU'] = '1'; $settings['VR_CONFIG_PATH'] = $emu; $settings['VR_LOG_PATH'] = "$emu\logs"
}
foreach ($k in $More.Keys) { $settings[$k] = [string]$More[$k] }
foreach ($k in $settings.Keys) { Set-Item -LiteralPath "Env:$k" -Value ([string]$settings[$k]) }

Set-Location -LiteralPath $root
& $victory run --console $Console $exe.FullName --execute
$code = $LASTEXITCODE
# The stand-in headset's SteamVR is not left running: the next start of SteamVR is your own.
if ($VREmu) { Stop-SteamVR }
exit $code
