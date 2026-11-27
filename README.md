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
The mouse looks around after a click on the picture (or F9).

Options of `scripts\play.ps1` (pass them to `Play.cmd` too):

| Option | Meaning |
| --- | --- |
| `-Game <folder>` | Folder of your copy. Default: the one saved by `Install.cmd`. |
| `-Console series-x\|series-s` | Console profile. Default `series-x`. |
| `-Panel` | On-screen frame-rate panel. |
| `-RayTracing` | The title's ray-traced light. Works on an RX 9070 XT, about 14-15 fps in heavy scenes. |
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

## Where it stands (2026-11-27)

Measured on: RX 9070 XT, Ryzen 7 9850X3D, 32 GB DDR5-6000, Windows 11, Series X
profile, the title's 4K mode with the world drawn at 1920x1080. One 7-minute run
from one save: the opening cutscenes into the first playable scene.

| What | Result |
| --- | --- |
| Whole run | median 43 ms a frame (23.0 fps); 90 % of frames under 68 ms |
| Menus and loading screens | up to 60 fps |
| Cutscene close-ups (1,400-1,600 draws) | 27 fps |
| First playable scene (about 3,900 draws) | 26 fps |
| The same scene looking at a shore with trees (about 6,200-6,700 draws) | 21.5-22 fps |
| Heaviest cutscene shots (4,500-6,000 draws) | about 20 fps, single frames far longer |
| Memory | about 29 GB private (16-17 GB working set), 12 GB of video memory |
| Picture | no flicker in the measured scenes: 47 tile flashes over a 7-minute recording (160-200 before), three pictures going back (13-28 before), no trees coming and going (a third to a half of all picture changes before) |
| Previous base of this repo, same save and PC | median 76 ms (13 fps) |

These are the settings with a clean picture. The fast settings of the base (presents sent ahead, steps done on the card) gave 21-23 fps over the same run with flickering light, black patches and trees that came and went; PORTING.md has the comparison.

Every run is recorded at 60 frames a second and looked through by a script for frames held, pictures going back and tiles flashing.

## VR (SteamVR)

`Play-VR.cmd` starts the title in the headset mode (`scripts\play.ps1 -VR`). SteamVR must be installed; it is started if it is not running.

| | |
| --- | --- |
| `3d` (default) | The game's frame as a relief around you, rebuilt from its depth buffer: in cutscenes you stand where the camera stands, with real depth in both eyes and parallax when you move your head. Drawn by the mod at the headset's rate, so it stays smooth while the game runs at 20-25 fps. |
| `cinema` | A flat screen standing in the room (`-VRMode cinema`). |
| `immersive` | A wide flat screen fixed to your head; turning your head turns the game's camera (`-VRMode immersive`). |
| Hands | A palm and five fingers follow each controller. The grip closes the hand into a fist, the trigger bends the index finger. A fist thrown forward presses the game's melee button (B; `-More @{VICTORY_VR_STRIKE='a'}` for another), with a tick in the controller. |
| Controllers | Left stick and trigger, right stick and trigger, the face buttons as A/B/X/Y, both sticks pressed for the menu. The keyboard and mouse keep working. |
| Keys | F6 next view, F7 centre the view where you look, F8 / F9 make the relief smaller / larger. |
| No headset | `Play-VR.cmd -VREmu`: SteamVR's built-in stand-in headset in a window, hands that clench, strike and open by themselves, a head that sways a little. Your own SteamVR settings are not touched (the stand-in keeps its own in `%LOCALAPPDATA%\victory\vr-emu`). |

What it is not yet: the game's own first-person camera, and hands that are the character's. The relief is one frame seen from one place, so what the camera did not see (behind people, around corners) is dark when you lean far to the side. The game's field of view is not read from the game: `-More @{VICTORY_VR_FOV='60'}` changes the assumed one (50 degrees).

## Known problems

- Not yet 30 fps: 22-26 fps in the playable scenes with the clean settings. At a shore with 6,500 draws the thread that submits prepares the frame's 37 submissions for about 24 ms and the card works 32 ms (its own timestamps); the two overlap badly: the card idles about 12 ms a frame while submissions are prepared, and the present waits about 8 ms for it at the end.
- Palms and trees can drop out in some cutscene shots. The base's fix for it (`VICTORY_PORT_CONSTANTS_ORDER=1`) is merged but off by default here: on the RX 9070 XT it makes the card work 78 ms a frame instead of 32 (91 ms a frame at the shore against 46).
- Hitches of 0.2-0.9 s when a scene loads on a PC with 32 GB of RAM: the process holds 22-24 GB of it and Windows pages (the work on memory is described in PORTING.md).
- Two pipelines are left out because the AMD shader compiler crashes on them.
- 29 pipelines that the title creates through two unimplemented device methods are not drawn.
- With ray tracing on, 14-15 fps in heavy scenes.

## What is merged

- Base: [avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX), which itself builds on [Marvisak/VIctory](https://github.com/Marvisak/VIctory) and [fxceless/victory-fork](https://github.com/fxceless/victory-fork).
- On top:
  - [Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD): the guard against the AMD shader compiler crashing.
  - [vitutu1/victory-perf](https://github.com/vitutu1/victory-perf): the loader hook that takes out the title's watchdog.
  - [fxceless/victory-fork](https://github.com/fxceless/victory-fork) PR #927 by tippythetaidum: 16-bit buffer channels through unsigned twin views (faces).
  - [Belobug/VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) branch `rx9070xt`: RX 9070 XT and 16 GB fixes, texture uploads in the submission prelude, PC FidelityFX 4.1.2 to 4K.
  - Own work: the fix for flickering light and black patches (a present waits for its own frame), the cause of trees flickering on shores (steps done on the card, now off), mouse look, a rays-miss mode for runs without ray tracing, objects of a command list released on a thread of their own, scripted keys for automated runs, warm refusals, the RX 9070 XT findings (clean-up pass 4, Series X with the 4K mode for a right-sized interface), per-part timing of list reset/close, constants of small shader-filled buffers read on the card instead of waiting for it (four waits a frame gone).

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
