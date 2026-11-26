# victory-merged-amd

A merged runtime of the VIctory family: it runs an Xbox Series title natively on
Windows from files you supply, with the console's GPU programs translated to
DXIL and its graphics run on Direct3D 12. This tree brings together the best
public work of several forks and adds its own, aimed at **an AMD Radeon RX 9070 XT
(RDNA4, 16 GB) on a PC with little RAM** (16 GB, one memory channel).

**This repository contains no game files, keys, executables or assets.** You need
your own copy. Not affiliated with or endorsed by any game's publisher or by
Microsoft.

## Where it stands (2026-11-26)

Measured on one PC: Radeon RX 9070 XT, Ryzen 7 9850X3D, 16 GB RAM in one DIMM,
Windows 11, Series S profile with FSR 4 (FidelityFX SDK 2.4).

| | |
|---|---|
| Menus, videos, loading screens | 30 fps |
| Opening scenes, in the world | **about 10 fps** (99-103 ms a frame); was 4.4 fps before the merges |
| Picture | lit, textured; the indirect light is estimated from the frame (no blocks of stale probe memory); skin in shadow is still too dark without ray tracing |
| Stability | 7-15 minute runs without a crash; the device losses in the meshlet culling are fixed (raw twins) |
| Memory | about 21 GB private, pagefile 32-48 GB recommended with 16 GB of RAM |

The frame is bound by the CPU side (preparing submissions, about 60 ms) and by
memory: with 16 GB of RAM the title pages. A second memory stick is the single
biggest gain on such a PC.

## What is merged

Each entry names its source; details, conflicts and measurements are in
[PORTING.md](PORTING.md) (in Russian). All sources are GPL-3.0-or-later.

- **[Marvisak/VIctory](https://github.com/Marvisak/VIctory)**: the base of everything:
  loader, system libraries, Direct3D 12 back end, shader recompiler.
- **[PastiiJay/klbf-lab](https://github.com/PastiiJay/klbf-lab)**: the base tree of this
  repository (KLBF Lab beta): AMD workarounds, the Series S path with FSR, pipeline pre-build.
- **[fxceless/victory-fork](https://github.com/fxceless/victory-fork)**: the relay and late
  waits, copies in the card's memory, indirect draw windows, async submit and present.
- **[avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX)**: the whole
  shader translator (merged three-way with ours), raw twins of buffer views, the indirect
  light estimate, the heap pass beside the recording, card steps, slots that hold their memory.
- **[Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD)**: the guard
  against the AMD shader compiler crashing on RDNA4, the magenta filter, the ambient stand-in.
- **[DexNom/VIctory](https://github.com/DexNom/VIctory)**: shader fixes (quad reads, cube
  gradients, vertex output interpolation).
- **Neptune (VIctory Edit.)**: VRAM-only windows, mouse and keyboard.
- Also: Geckomacabre/vx-layer, MiamiGame/XTP, Itsbatt/victory-unified, JoshuuDev/victory-amd.

Own work in this tree (see PORTING.md): VRAM-only windows ported to this base, pageable CPU
sides of card copies, texture staging release, a memory census, card-merge check mode, a reaper
thread, descriptor-heap sync beside the preparing, async present of multi-plane frames, a
pre-build that survives the RDNA4 compiler's random crashes, scripted keys.

## Build

Windows 10/11 x64, the MSVC build tools, Rust (the toolchain is pinned in
`rust-toolchain.toml`):

```powershell
cargo build --release -p victory
```

## Run

1. Pre-build the shaders and pipelines once per build of the translator (resumable; it goes
   on after the driver crashes, which the RDNA4 compiler does at random):

   ```powershell
   scripts\prebuild.ps1 -Game <folder with your copy>
   ```

2. Play with the settings measured best on the RX 9070 XT:

   ```powershell
   scripts\play.ps1 -Game <folder with your copy> -Ffx <FidelityFX SDK>\Kits\FidelityFX\signedbin
   ```

   Keyboard and mouse: click the picture (or F9) to capture the mouse.

Every `VICTORY_...` switch is described in a comment where it is read.

## License

GPL-3.0-or-later, as VIctory. See [LICENSE](LICENSE). History and credits of the
upstream projects are in their own repositories.
