# victory-merged-amd-VR

A **SteamVR headset mode** for an Xbox Series title run natively on Windows by the
VIctory runtime family (GPL-3.0-or-later), and the merged runtime it lives in.
Many trees tune the runtime's speed; this one's own subject is VR: the title's
frame as a 3D relief that fills the headset's view, hands with fingers at the
controllers, the controllers as the title's pad, body trackers, and a stand-in
headset for trying all of it without one.

The runtime is built on
[avatarmanz/VIctory-7900XTX](https://github.com/avatarmanz/VIctory-7900XTX) `main`
at commit `014d52ded`, with our changes on top, and is aimed at an AMD Radeon
RX 9070 XT (16 GB) on a PC with 32 GB of RAM.

**This repository contains no game files, keys, executables or assets.** You need
your own copy. Not affiliated with or endorsed by any game's publisher, by
Microsoft or by Valve. SteamVR's hand and controller models are read from your
own SteamVR install when the mod runs; none are carried here.

## The VR mod

`Play-VR.cmd` starts the title in the headset mode (`scripts\play.ps1 -VR`).
SteamVR must be installed; it is started if it is not running. The code is in
`crates/victory-gpu/src/vr/` (about 3,500 lines, no dependency besides SteamVR's
own `openvr_api.dll`, loaded from your install).

| | |
| --- | --- |
| `3d` (default) | The game's frame rebuilt as a relief from its depth buffer and shown as high as the headset's own view: no screen and no frame around it. Past the frame's edges the scene goes on (mirrored, at the scene's own depth), and beyond that its blurred colours. Both eyes get real depth, and leaning moves near things against far ones. It is drawn by the mod at the headset's rate, so your head stays smooth while the game runs at 20-27 fps. |
| `cinema` | A flat screen standing in the room (`-VRMode cinema`). |
| `immersive` | The relief fixed to your head; turning your head turns the game's camera (`-VRMode immersive`). For playing. |
| Hands | SteamVR's own hand model at each controller (31 bones, skinned by the mod), its fingers bent by grip and trigger. A fist thrown forward presses the game's melee button (B; `-More @{VICTORY_VR_STRIKE='a'}` for another), with a tick in the controller. |
| Controllers | While watching (`3d`, `cinema`) your controllers are shown as SteamVR draws them; F10 goes between hands and controllers, and a closing hand is always a hand. Left stick and trigger, right stick and trigger, the face buttons as A/B/X/Y, both sticks pressed for the menu. Keyboard, mouse and gamepad keep working. |
| Body trackers | Trackers worn on the body (waist, feet, knees, elbows, chest, shoulders: the roles set in SteamVR) are read and shown as markers, and `vr::body()` gives their places to code that would move a body with them. They do not move the character yet. |
| Keys | F6 next view, F7 centre the view where you look, F8 / F9 the frame smaller / larger, Ctrl+F8 / Ctrl+F9 the world nearer / further (more / less depth between the eyes), F10 hands or controllers. |
| No headset | `Play-VR.cmd -VREmu`: SteamVR's built-in stand-in headset in a window, hands that clench, strike and open by themselves, trackers at the waist and feet, a head that sways. `-More @{VICTORY_VR_EMU_POSE='reach'; VICTORY_VR_EMU_SWAY='calm'}` holds the hands out, palms up. Your own SteamVR settings are not touched (the stand-in keeps its own in `%LOCALAPPDATA%\victory\vr-emu`). |

### How it works

- **The frame.** Where the title's frame goes to the window, the back buffer and
  the depth buffer the title draws its frames with are copied into one of three
  snapshots. Neither side waits for the other.
- **Our own scene.** A SteamVR scene application with a Direct3D 12 queue of its
  own submits both eyes at the headset's rate: a sheet of 624 by 351 cells, each
  corner put back where the title's camera saw it. One frame of ours costs the
  card 0.25 ms at 1176x1320 an eye (the card's own timestamps).
- **A turning head opens nothing.** A head turns on the neck, which moves the
  eyes some centimetres sideways: enough to look behind everything near, where
  the title's one camera saw nothing. So the relief's camera goes with the eyes
  round the neck and stays behind only by what the neck itself moves (leaning,
  stepping).
- **What the camera did not see.** Each eye still sees a little behind the edge
  of a near thing. For each pixel the mod finds, from how the depth goes across
  its triangle, whether it lies over such an edge and how far this eye sees the
  near side moved off the far one; then the near thing keeps its own texels, to
  the texel, and the rest takes the far side carried on, mirrored at the edge,
  so that the background's texture goes on instead of one colour smeared.

### Settings

Pass them as `-More @{NAME='value'}`.

| Setting | Meaning |
| --- | --- |
| `VICTORY_VR_FOV` | How high the frame is shown, degrees. Default: the headset's own view (a little over it in `3d`). |
| `VICTORY_VR_SCALE` | The world's scale: under 1 nearer and smaller, with more depth between the eyes. Default 1. |
| `VICTORY_VR_PARALLAX` | How much of your leaning moves near things against far ones: 1 all of it, 0 none (the depth of two eyes only). |
| `VICTORY_VR_SKIRT` | How far past its edges the relief goes on, as a part of the frame. Default 0.15. |
| `VICTORY_VR_NEAREST` | Nothing is shown nearer than this, metres. Default 0.3. |
| `VICTORY_VR_GRID` | Cells across the frame. Default 480. |
| `VICTORY_VR_HANDS` | 1 hands always, 0 controllers always. Default: by the view. |
| `VICTORY_VR_HAND_AT`, `VICTORY_VR_HAND_TILT` | The wrist's place from the controller's own origin (`right,up,back` in metres; default `0,-0.035,0.115`) and the fingers lifted by so many degrees: for controllers the hand sits wrongly on. |
| `VICTORY_VR_SKIN` | The hands' colour, `r,g,b` from 0 to 1. |
| `VICTORY_VR_FOLLOW` | 0: the picture does not come after a head turned further than 25 degrees. |
| `VICTORY_VR_EYE_MOST` | The most pixels across an eye's picture. Default 2400. |

### Where the mod stands

- This form of it has been run with SteamVR's stand-in headset and emulated
  controllers only. An earlier form was tried in a real headset; what was found
  there (a frame around the picture, a dark spot in each eye behind near things,
  flat hands) is what this form answers. It has not been back in a real headset
  yet.
- It shows the title's one camera. In a cutscene you cannot look behind you: the
  frame fills the view and goes on mirrored past its edges, and turning further
  shows its blurred colours.
- The frame is magnified to fill the view (the title's own field of view is not
  read from the game), so things look larger than life. Ctrl+F8 brings the world
  nearer, for a truer size at the price of more that the camera did not see.
- The hands are SteamVR's hand model, not the character's own arms, and there is
  no first-person camera of the mod's own: both need the title's camera and
  skeleton found among its constants, which is not done. (The title has a
  first-person view of its own when the character takes the phone out.)
- With the stand-in headset the title's first playable scene ran at 13-16 fps
  against 26-27 fps without the headset mode, though a frame of ours costs the
  card 0.25 ms; the cause is not found yet. Cutscenes ran at 25-28 fps with it.

## Quick install

1. Click Code, then Download ZIP (and unpack it), or `git clone` this repository.
2. Double-click `Install.cmd`. It checks the PC, builds the runtime, asks for the
   folder of your own copy and translates all shaders once (about two minutes).
3. Double-click `Play.cmd`, or `Play-VR.cmd` for the headset.

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
3. Дважды щёлкнуть `Play.cmd`. Для шлема: `Play-VR.cmd` (без шлема: `Play-VR.cmd -VREmu`).

Нужны Windows 10/11 x64, видеокарта с Direct3D 12 и 12-16 ГБ видеопамяти, лучше 32 ГБ ОЗУ и файл подкачки 32-48 ГБ, около 3 ГБ на диске для сборки и ваша собственная копия игры.

## Where the runtime stands (2026-11-27)

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

## Known problems

- VR: see "Where the mod stands" above.
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
  - Own work: the SteamVR mod (`crates/victory-gpu/src/vr/`); the fix for flickering light and black patches (a present waits for its own frame), the cause of trees flickering on shores (steps done on the card, now off), mouse look, a rays-miss mode for runs without ray tracing, objects of a command list released on a thread of their own, scripted keys for automated runs, warm refusals, the RX 9070 XT findings (clean-up pass 4, Series X with the 4K mode for a right-sized interface), per-part timing of list reset/close, constants of small shader-filled buffers read on the card instead of waiting for it (four waits a frame gone).

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
