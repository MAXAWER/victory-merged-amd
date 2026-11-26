# The settings measured best on a Radeon RX 9070 XT (README.md). Returns a hashtable.
# Shared by play.ps1, warm.ps1 and install.ps1 so that all of them use exactly the same set.
param(
    [switch]$Fps30
)
$s = [ordered]@{
    VICTORY_TEXTURE_NO_WAIT    = '1'
    # The relay's first stage.
    VICTORY_RELAY              = '1'
    VICTORY_WIDE_BACK          = '1'
    VICTORY_CARD_REHASH_RUNS   = '1'
    VICTORY_JOIN_COPIES        = '1'
    VICTORY_INDIRECT_WINDOW    = '1'
    VICTORY_RELAY_COPY         = '1'
    # Steps done on the card made trees on a shore come and go from frame to frame (a third to a half of all picture changes; none without them), about 3 fps.
    VICTORY_CARD_STEPS         = '0'
    VICTORY_INDIRECT_BATCH     = '2'
    VICTORY_MAP_THREADS        = '1'
    VICTORY_TITLE_PRIORITIES   = '0'
    VICTORY_CARD_AWAKE         = '1'
    VICTORY_SHOW_AFTER         = '1'
    VICTORY_AMBIENT            = '0.8'
    VICTORY_D3D12_DRED         = 'light'
    VICTORY_JOIN_PARTS         = '8'
    VICTORY_UPLOAD_AHEAD       = '1'
    VICTORY_PORT_CONSTANTS     = '1'
    VICTORY_PORT_DESCRIPTORS   = '1'
    VICTORY_PORT_CBV           = '1'
    VICTORY_PORT_GUARD         = '1'
    VICTORY_PORT_JOURNAL       = '1'
    VICTORY_NO_WATCHDOG        = '1'
    VICTORY_THIN               = '0'
    VICTORY_OUTPUT             = '3840x2160'
    VICTORY_RENDER_SCALE       = '0.5'
    VICTORY_FRAME_INTERVAL_MS  = '16.667'
    VICTORY_PIPELINE_BOOK      = '1'
    # A present waits for the work of its own frame. With presents sent ahead, light and shadow on
    # characters alternated every frame and black patches came and went (recordings of 2026-11-27:
    # 160-200 tile flashes a run against 36); costs about 2 ms a frame.
    VICTORY_LATE_WAIT          = '2'
    # Without the ray-tracing structure builder the title's rays find nothing, every frame alike;
    # part of the shader translation.
    VICTORY_RAYS_MISS          = '1'
}
if ($Fps30) { $s.Remove('VICTORY_FRAME_INTERVAL_MS') }

# With every clean-up pass of the translator the RX 9000 driver's shader compiler crashes on
# about thirty pipelines; without pass 4 two are left.
$gpuNames = @()
try { $gpuNames = @(Get-CimInstance Win32_VideoController -ErrorAction Stop | ForEach-Object { $_.Name }) } catch { }
if ($gpuNames | Where-Object { $_ -match 'RX 90\d0' }) { $s['VICTORY_SHADER_OPT'] = '59' }

return $s
