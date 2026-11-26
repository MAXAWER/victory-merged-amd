# Starts the title with the settings measured best on a Radeon RX 9070 XT (README.md).
#   scripts\play.ps1 -Game <folder with your copy> [-Ffx <FidelityFX SDK>\Kits\FidelityFX\signedbin]
# -Console  series-s (default, 720p with FSR when -Ffx is given) or series-x
# -More     other settings: -More @{VICTORY_PANEL='1'}
param(
    [Parameter(Mandatory)][string]$Game,
    [string]$Ffx = "",
    [ValidateSet('series-s', 'series-x')][string]$Console = 'series-s',
    [string]$Cache = "$env:LOCALAPPDATA\victory\cache-merged-amd",
    [hashtable]$More = @{}
)
$root = Split-Path -Parent $PSScriptRoot
$victory = Join-Path $root 'target\release\victory.exe'
if (-not (Test-Path $victory)) { Write-Host "No $victory. Build it first (README.md)."; return }
$exe = Get-ChildItem -LiteralPath $Game -Filter '*_scarlett_master.exe' -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $exe) { Write-Host "No title executable in $Game."; return }

# (Other settings left over in this window go.)
Get-ChildItem Env:VICTORY_* | ForEach-Object { Remove-Item "Env:$($_.Name)" }
$settings = @{
    # KLBF's RX 9070 XT basics
    VICTORY_SHADER_OPT = '0'; VICTORY_LATE_WAIT = '3'; VICTORY_WIDE_BACK = '1'; VICTORY_PIPELINE_WAIT_MS = '24'
    VICTORY_RELAY_COPY_QUEUE = '0'; VICTORY_D3D12_DRED = '1'
    # memory: VRAM-only windows, pageable CPU sides of card copies
    VICTORY_CARD_WINDOWS_MB = '4096'; VICTORY_CARD_WINDOW_UPLOAD = '1'; VICTORY_CARD_SIDE_PAGEABLE = '1'
    # victory-fork's submit and draw-window set
    VICTORY_INDIRECT_WINDOW = '1'; VICTORY_WINDOW_JUMP = '1'; VICTORY_INDIRECT_GATHER = '1'; VICTORY_CARD_REHASH_RUNS = '1'
    VICTORY_COPY_PLAIN = '1'; VICTORY_JOIN_PARTS = '1'; VICTORY_BACK_OWN_VIEW = '1'; VICTORY_ASYNC_SUBMIT = '1'
    VICTORY_QUEUE2 = '1'; VICTORY_TEXTURE_NO_WAIT = '1'; VICTORY_MERGE_CARD = 'check'; VICTORY_VIEW_KNOWN = '1'
    VICTORY_ASYNC_PRESENT = '1'; VICTORY_HEAP_SYNC_ASIDE = '1'
    # the title's own 30 s watchdog off (a long first frame no longer ends the run)
    VICTORY_NO_WATCHDOG = '1'
    # (VICTORY_TEXTURE_DROP_MIPS=1, VICTORY_BAKE_TEXTURES_MS=30000, VICTORY_PORT_CONSTANTS=1 and
    # VICTORY_CARD_STEPS=2 are off again: they were judged on a quiet scene only and are being
    # checked against the cutscenes; see PORTING.md.)
}
$skip = Join-Path $root 'docs\klbf\skip-amd.txt'
if (Test-Path $skip) { $settings.VICTORY_SKIP_PIPELINES = (Get-Content $skip -Raw).Trim() }
if ($Ffx) { $settings.VICTORY_FFX_DIR = $Ffx }
foreach ($k in $More.Keys) { $settings[$k] = $More[$k] }
foreach ($k in $settings.Keys) { Set-Item "Env:$k" $settings[$k] }
New-Item -ItemType Directory -Force $Cache | Out-Null
$env:VICTORY_CACHE_DIR = $Cache
Push-Location $root
try { & $victory run --console $Console $exe.FullName --execute } finally { Pop-Location }
