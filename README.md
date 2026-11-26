# victory-merged-amd

A merged runtime of the VIctory family (GPL-3.0-or-later): it runs an Xbox Series
title natively on Windows from files you supply, with the console's GPU programs
translated to DXIL and its graphics run on Direct3D 12. The tree is now built on
[avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX) `main`
at commit `c9c932cbc`, with our changes on top, and is aimed at an AMD Radeon
RX 9070 XT (16 GB) on a PC with 32 GB of RAM.

**This repository contains no game files, keys, executables or assets.** You need
your own copy. Not affiliated with or endorsed by any game's publisher or by
Microsoft.

## Quick install

1. Click Code, then Download ZIP (and unpack it), or `git clone` this repository.
2. Double-click `Install.cmd`. It checks the PC, builds the runtime, asks for the
   folder of your own copy and translates all shaders once (about two minutes).
3. Double-click `Play.cmd`.

Requirements: Windows 10/11 x64; a Direct3D 12 card with 12-16 GB of video memory;
32 GB of RAM recommended, plus a 32-48 GB page file; about 3 GB of disk for the
build; your own copy of the title. If Rust or the Visual Studio C++ build tools are
missing, `Install.cmd` offers to install them with `winget`.

Keys: WASD move, IJKL camera, Enter/Space = A, Backspace = B, X = X, Y = Y,
Q/E = bumpers, Z/C = triggers, Esc = menu. An XInput gamepad also works.

Options of `scripts\play.ps1` (pass them to `Play.cmd` too):

| Option | Meaning |
| --- | --- |
| `-Game <folder>` | Folder of your copy. Default: the one saved by `Install.cmd`. |
| `-Console series-x\|series-s` | Console profile. Default `series-x`. |
| `-Panel` | On-screen frame-rate panel. |
| `-RayTracing` | Builds the ray-tracing structures for the title's indirect light. Experimental. |
| `-Fps30` | Leaves the frame interval setting out. |
| `-Cache <folder>` | Cache folder. Default `%LOCALAPPDATA%\victory\cache-merged-amd`. |
| `-More @{NAME='value'}` | Extra `VICTORY_*` settings. |

The first minutes of every new scene stutter while the driver compiles pipelines.
After a first play session, `scripts\warm.ps1` makes the pipelines that earlier
sessions drew with ahead of time (resumable).

### Быстрая установка

1. Code -> Download ZIP (распаковать) или `git clone`.
2. Дважды щёлкнуть `Install.cmd`: проверка ПК, сборка, выбор папки вашей копии, разовый перевод шейдеров (около двух минут).
3. Дважды щёлкнуть `Play.cmd`.

Нужны Windows 10/11 x64, видеокарта с Direct3D 12 и 12-16 ГБ видеопамяти, лучше 32 ГБ ОЗУ и файл подкачки 32-48 ГБ, около 3 ГБ на диске для сборки и ваша собственная копия игры.

## Where it stands (2026-11-26)

Measured on: RX 9070 XT, Ryzen 7 9850X3D, 32 GB DDR5-6000, Windows 11, Series X
profile, the title's 4K mode with the world drawn at 1920x1080. One 7-minute run
from one save: the opening cutscenes into the first playable scene.

| What | Result |
| --- | --- |
| Whole run | median 44 ms a frame (22.5 fps); 90 % of frames under 113 ms |
| Menus and loading screens | up to 60 fps |
| Cutscene close-ups (1,400-1,600 draws) | 21-34 fps |
| First playable scene (about 3,900 draws) | 25 fps, steady at 39-40 ms |
| Heaviest shots (4,200-5,700 draws) | 17-22 fps |
| Memory | about 30 GB private (23 GB working set), 12 GB of video memory |
| Previous base of this repo, same save and PC | median 76 ms (13 fps) |

## Known problems

- Not yet a stable 30 fps in heavy scenes: the frame is CPU-bound, the card is 65-70 % busy.
- Hitches when a new scene loads.
- Two pipelines are left out because the AMD shader compiler crashes on them.
- 29 pipelines that the title creates through two unimplemented device methods are not drawn.
- Dark or black patches where there is no direct light: the title's ray-traced indirect light is off by default (`-RayTracing` is experimental).
- The painted loading screen jerks.

## What is merged

- Base: [avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX), which itself builds on [Marvisak/VIctory](https://github.com/Marvisak/VIctory) and [fxceless/victory-fork](https://github.com/fxceless/victory-fork).
- On top:
  - [Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD): the guard against the AMD shader compiler crashing.
  - [vitutu1/victory-perf](https://github.com/vitutu1/victory-perf): the loader hook that takes out the title's watchdog.
  - [fxceless/victory-fork](https://github.com/fxceless/victory-fork) PR #927 by tippythetaidum: 16-bit buffer channels through unsigned twin views (faces).
  - Own work: scripted keys for automated runs, warm refusals, the RX 9070 XT findings (clean-up pass 4, Series X with the 4K mode for a right-sized interface), per-part timing of list reset/close.

The history of the previous KLBF-based tree and all measurements are in
[PORTING.md](PORTING.md) (Russian).

## Build by hand

```
cargo build --profile diag -p victory --features diagnostics
```

The result is `target\diag\victory.exe`.

## License

GPL-3.0-or-later, as VIctory. See [LICENSE](LICENSE). History and credits of the
upstream projects are in their own repositories.
