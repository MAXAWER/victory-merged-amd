# One-time setup: checks the PC, builds the runtime, remembers the folder of your copy and
# translates every shader once. Start it with Install.cmd.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root

# (a) the PC
Write-Host "== Checking the PC =="
if (-not [Environment]::Is64BitOperatingSystem) { Write-Host "A 64-bit Windows is needed."; exit 1 }
$os = Get-CimInstance Win32_OperatingSystem
Write-Host "Windows: $($os.Caption) (build $($os.BuildNumber))"
if ([int]$os.BuildNumber -lt 10240) { Write-Host "Windows 10 or 11 is needed."; exit 1 }
$gpus = @(Get-CimInstance Win32_VideoController | ForEach-Object { $_.Name })
Write-Host ("Video card(s): " + ($gpus -join '; '))
$ramGb = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 1)
Write-Host "RAM: $ramGb GB"
if ($ramGb -lt 31) { Write-Host "WARNING: under 32 GB of RAM. The title may run out of memory; a 32-48 GB page file helps." }

# (b) Rust and the C++ build tools
Write-Host ""
Write-Host "== Looking for Rust =="
$cargo = $null
$cmd = Get-Command cargo -ErrorAction SilentlyContinue
if ($cmd) { $cargo = $cmd.Source }
elseif (Test-Path -LiteralPath "$env:USERPROFILE\.cargo\bin\cargo.exe") { $cargo = "$env:USERPROFILE\.cargo\bin\cargo.exe" }
if (-not $cargo) {
    Write-Host "Rust (rustup) and the Visual Studio C++ build tools are needed to build the runtime."
    Write-Host "Commands to install them:"
    Write-Host "  winget install Rustlang.Rustup"
    Write-Host '  winget install Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive"'
    $a = Read-Host "Run them now? (Y/N)"
    if ($a -match '^[Yy]') {
        winget install Rustlang.Rustup
        winget install Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive"
        Write-Host ""
        Write-Host "Done. Close this window, open a new one and run Install.cmd again."
    } else {
        Write-Host "Install them yourself, then run Install.cmd again."
    }
    exit 1
}
Write-Host "cargo: $cargo"

# (c) build
Write-Host ""
Write-Host "== Building (the first time takes a while) =="
$ErrorActionPreference = 'Continue'   # cargo writes progress to stderr
& $cargo build --profile diag -p victory --features diagnostics
$ErrorActionPreference = 'Stop'
$victory = Join-Path $root 'target\diag\victory.exe'
if (-not (Test-Path -LiteralPath $victory)) {
    Write-Host "The build did not produce $victory. Read the messages above."
    exit 1
}

# (d) the folder of your copy
Write-Host ""
Write-Host "== Your copy of the title =="
$game = $null
try {
    Add-Type -AssemblyName System.Windows.Forms
    $dlg = New-Object System.Windows.Forms.FolderBrowserDialog
    $dlg.Description = "Choose the folder of your own copy (it holds *_scarlett_master.exe and shaders.rpf)"
    if ($dlg.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { $game = $dlg.SelectedPath }
} catch { }
if (-not $game) { $game = Read-Host "Folder of your copy" }
$game = ([string]$game).Trim().Trim('"')
$exe = $null
if ($game) { $exe = Get-ChildItem -LiteralPath $game -Filter '*_scarlett_master.exe' -ErrorAction SilentlyContinue | Select-Object -First 1 }
if (-not $exe -or -not (Test-Path -LiteralPath (Join-Path $game 'shaders.rpf'))) {
    Write-Host "That folder needs *_scarlett_master.exe and shaders.rpf. Run Install.cmd again and pick the right one."
    exit 1
}
$cfgDir = "$env:LOCALAPPDATA\victory"
New-Item -ItemType Directory -Force -Path $cfgDir | Out-Null
$cfgJson = ConvertTo-Json -InputObject ([ordered]@{ game = $game })
Set-Content -LiteralPath "$cfgDir\merged-amd.json" -Value $cfgJson -Encoding UTF8
Write-Host "Saved: $cfgDir\merged-amd.json"

# (e) translate every shader once (same settings and cache folder as play.ps1)
Write-Host ""
Write-Host "== Translating the shaders (about two minutes) =="
$cache = "$env:LOCALAPPDATA\victory\cache-merged-amd"
Get-ChildItem Env:VICTORY_* | ForEach-Object { Remove-Item "Env:$($_.Name)" }
$settings = & (Join-Path $PSScriptRoot 'settings.ps1')
$settings['VICTORY_CACHE_DIR'] = $cache
foreach ($k in $settings.Keys) { Set-Item -LiteralPath "Env:$k" -Value ([string]$settings[$k]) }
& $victory shaders (Join-Path $game 'shaders.rpf')
if ($LASTEXITCODE -ne 0) { Write-Host "The shader translation reported an error (code $LASTEXITCODE)."; exit 1 }

# (f) how to play
Write-Host ""
Write-Host "== Ready =="
Write-Host "Start the title with Play.cmd."
Write-Host "Keys: WASD move, IJKL camera, Enter/Space = A, Backspace = B, X = X, Y = Y, Q/E = bumpers, Z/C = triggers, Esc = menu. The mouse looks around after a click on the picture or F9 (F9 again lets it go)."
Write-Host "An XInput gamepad also works."
Write-Host "The first minutes of every new scene stutter while the driver compiles pipelines."
Write-Host "After a first play session, run scripts\warm.ps1 to make them ahead."
