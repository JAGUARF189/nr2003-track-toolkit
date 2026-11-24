# NR2003 Track Toolkit

One-click build pipeline for Papyrus Sandbox tracks, written in Rust.
It drives the original 2003 tools (`mklib`, `makemip`, `make3do`, `mkstalls`) and leaves them unmodified.

- `target\release\track-toolkit.exe` is the GUI.
- `target\release\ttk.exe` is the command line. It runs the same pipeline.

## Steps

| # | Step | What it does |
|---|------|--------------|
| 1 | Workspace | Creates `tracks\<name>\trackmat` and `src_textures\` folders. Copies `shared.dat` from the game, unpacks it into `tracks\shared` (once), and copies the flagger textures. It can also seed an empty track from an existing game track's `.dat`, renaming its `.ptf` to `<name>.ptf`. |
| 2 | Textures | Converts PNG/JPG/BMP/TGA to TGA and then MIP. Can resize to powers of two (makemip rejects any other size). Supports makemip's `-tile/-dxt/-trueColor/-trans` options. Unchanged images are skipped. |
| 3 | Models | Runs `make3do -s` on each `.psg` and fails if the output contains `ERROR`. |
| 4 | Pack | Writes `<name>.def` listing the files in the track folder and `trackmat\`, builds `<name>.dat`, then checks the listing against the `.def`. |
| 5 | Install | Copies the `.dat` to `NR2003\tracks\<name>`, backing up the old one to `_ttk_backup\` (the newest 5 are kept). Copies missing `track.ini`/`.cam`/`.sim`/`.acd`/`.lp` files from the template track and never overwrites existing ones. |
| 6 | Stalls | Runs `mkstalls` on an LP file and replaces the `stall_N` lines in one track.ini section, keeping `stall_pace` and preserving bytes and line endings. A backup is made first. |

## Look at a track (the Sandbox rebuild)

The **Look at a track** tab opens any installed track, or a `.ptf` / `.dat` file, and shows it:
- **Map (from above):** pan and zoom. Click a piece of track to select it.
- **3D:** drag to turn, right-drag to move, scroll to zoom.
- **Pieces list:** every straight and curve, with length, turn angle, radius and banking. The selected piece also shows its height, the ground across it (right to left) and its walls in plain words.

`track-toolkit.exe <file.ptf|.dat> [--3d] [--piece N] [--wall N | --strip N] [--objects]` opens straight into this tab. `--wall`/`--strip` pick the Nth wall or ground strip, counting right to left, of that piece. `--dist M --yaw D --pitch D` set the 3D camera (for screenshots).

### Editing the shape (phase 3)

Select a piece (in the list or on the map) to edit it:
- **Shape:** straight or curve. **Length** in metres; for curves also **Turns** (degrees, left/right) and **Radius**.
  - Changing a curve's length keeps its radius and changes the angle.
- **Banking** at the start and end, in degrees. Positive = the outside of the turn is higher (the right side on straights).
- **Changing the shape:** every piece after it moves with it, so the track stays joined. With "Move objects with the track" on, objects beside those pieces move too.
  - A gap can open at the start/finish line. The checker shows how big it is.
- **Changing banking:** the road tilts about the inside edge, and everything further out (walls, apron, run-off) moves with it. The pieces either side are adjusted, so there's no step or kink at the joins. Banking changes evenly from start to end.
- **Undo:** dragging or typing a number makes one Undo step.
- **After a shape change:** the AI lines (.lp), cameras and pit stalls are measured along the old track and need making again. The tab says so.

**Check** lists problems, worst first. Click one to go to that piece:
- **Won't load:**
  - Sandbox's own load limits (sandbox.exe): headings within 0..2π, a straight bending more than 0.5°, cross-section limits, walls over 100 m tall.
  - Anything on a curve reaching past the turn's centre.
- **Warnings:**
  - Gaps, kinks or steps between pieces, and a track that doesn't close.
  - Curves over 90° (the 2003 track makers split them).
  - Steep road (over 10% grade), and ground strips or walls off the edge of the ground.
  - Texture/object files that can't be found.
- **Calibration:** all 47 installed game tracks give no "won't load" findings.

### Real textures

The map and 3D view draw the track's own textures. Untick **Textures** for plain colours by surface and wall type.
- **Exact placement:** each strip and wall face uses the texture coordinates stored in the track file, so pictures sit exactly where the game puts them. Signs read the right way round, and fences are see-through between the posts.
- **Where textures are found:** the track folder and its `trackmat` folder, the track's `.dat` files, then `tracks\shared`. A texture that can't be found is drawn in its plain colour.
- **Speed:** textures load at about 256 pixels in the background with the track. Watkins Glen's 75 textures take 0.05 s and about 6 MB.

### Walls, ground and textures (phase 5)

Click the track on the map: that picks the piece and the wall or ground strip under the mouse, outlined in blue. You can also pick from the lists under the piece editor.
- **Wall:**
  - **Type:** in plain words (hard wall, tire wall, fence…).
  - **Where:** distance from the centre line at the start and end. Type e.g. `12 left` or `8.5 right`.
  - **Height** and **Thickness**.
  - **Texture.**
  - **Add a wall on the right / on the left:** copies the nearest wall on that side onto the outer edge of the ground. **Delete this wall** removes one.
- **Ground strip:**
  - **Surface:** asphalt, concrete, grass, sand…
  - **Right edge:** where the strip starts. It can't pass the strips beside it.
  - **Texture.**
  - **Split in two** makes two strips side by side, so half can be changed. **Remove** lets the strip to the right fill the gap.
- **Textures:**
  - **Picking:** choose any texture the track already uses, or **Another .mip file…**, which copies the file into the track's `trackmat` folder and never overwrites anything.
  - **Only this wall or strip changes:** each face has its own texture settings, and the texture files stay shared.
- **Copy to pieces A to B:**
  - **A wall** gives the wall on the same side of each of those pieces its type, height, thickness and texture.
  - **A strip** gives the strip in the same place its surface and texture.
- **Order:** every installed track lists strips and walls right to left. New and moved ones keep that order.

### Objects (phase 4)

Click **Work on: Objects**. Every placed object shows as a dot on the map.
- **Picking objects (map view):**
  - Click one to select it. Click the same spot again to get the one underneath.
  - Ctrl+click adds or removes one. Drag a box around several.
  - Click empty ground, or press Esc, to select nothing.
- **Moving and turning:**
  - Drag selected objects to move them.
  - Drag the round blue handle to turn them about their middle.
  - Right-drag moves the map. Each drag is one Undo step.
- **Add objects…** picks `.3do` files and places them in a row in the middle of the view, sitting on the ground.
  - A file not already in the track folder is copied in, along with the textures and models it names (found by reading the `.3do`).
  - A file already in the track folder is never overwritten. If a texture can't be found, the tab says so.
- **Make a copy** (Ctrl+D) copies the selection 5 m along. **Delete** (Delete key) removes it. Object files nothing uses any more are dropped from the track.
- **Lock / Unlock:** locked objects show grey and can't be picked on the map, so they can't be moved or deleted by accident.
  - Unlock them from the list.
  - Locks are kept next to the track in `<track>.ptf.ttk.json`; the game ignores it.
- **Put on the ground** sets each selected object's height to the ground under it.
- **One object selected:** type its position, height and direction.
- **The list** shows every object. Search it by name; click one to jump to it.
- **Saving:** the save check compares in the file's own numbering, so adding and deleting objects saves safely. Tested by saving, reading back and comparing.

### Saving and autosave

- **Save / Save as… / Undo / Redo** (Ctrl+S, Ctrl+Z, Ctrl+Y). Undo covers lighting, shape edits and object edits.
- **Every save is checked before it counts.** The new file is written as `<name>.ptf.saving` and read back. Only if it matches does the old file go to `_ttk_backup\` (the newest 20 are kept), and then the new file takes its place. A failed save never touches your track.
- **Autosave** runs only when there are unsaved changes. It defaults to **every 5 minutes**; set any time from 10 seconds to a week, in seconds, minutes or hours. The setting is stored in `settings.json` (`autosave`, `autosave_secs`).
  - The countdown starts at your first unsaved change and restarts after every save.
  - A track opened from inside a `.dat` has nowhere to save yet. Autosave keeps a safety copy in `%APPDATA%\TrackToolkit\autosave\` until you pick a place with Save.
- **Earlier versions** lists the backups. Open one, then Save to put it back.
- **Closing the app, or opening another track, with unsaved changes** asks first: Save / Don't save / Keep working.

Tracks are read by `cpp\build\ptfjson.exe` (C++). It must sit next to `track-toolkit.exe`, or in `cpp\build\` of a source checkout. ptfjson decodes all 11 .ptf classes (see `docs\ptf\classes.md`), and writes them back byte-identical: all 55 .ptf files tested so far round-trip exactly.

```
ptfjson read  <in.ptf> <out.json>
ptfjson write <in.json> <out.ptf>
ptfjson check <in.ptf>...          (re-encode and byte-compare)
```

### Testing the editors without a mouse

`cargo test --release --bin track-toolkit` runs the Track tab's real panels in memory (`src\track_ui\ui_tests.rs`).
- **How it works:** widgets are found by name through egui's accessibility tree, and clicks and typing are fed in as input events. No window opens and no real input is sent, so it's safe while someone is using the PC.
- **What it covers:**
  - Shape editor: typing a length, Undo, picking from the piece list, typing banking and radius.
  - Wall editor: type, height, add and delete.
  - Strip editor: pick, surface, split and remove, texture, copy to the next piece.
  - Map picking of walls and strips.
  - Every test saves and reads the file back.
- **Real clicks:** `re\gui_shot.ps1` refuses to send input if the PC was used in the last minute, or if anything but the toolkit is in front.

## Assetto Corsa tracks (converting to NR2003, phases 1-3 of 5)

The **Assetto Corsa** tab opens an AC track and shows it in the map and 3D views, with its textures.
- **Finding tracks:** it finds AC through Steam's library list, or you can pick the `content\tracks` folder yourself.
- **Picking:** choose a track and, for tracks with several layouts, the layout.
- **What it shows:**
  - The AI line, with the track edges stored alongside it.
  - The pit boxes and grid spots (AC's `AC_PIT_n` / `AC_START_n` markers).
  - What cars drive on, listed in plain words: road, kerbs, grass, walls, pit lane. Tick **Colour what cars drive on** to see them in plain colours, including the hidden ones AC uses only for driving.
- **Command line:** `track-toolkit.exe --ac <AC track folder> [--layout name] [--3d]`.
- **How it works:** the reading is done in C++ by `acdump.exe` (`cpp\src\kn5.cpp`, `dds.cpp`, `actrack.cpp`):
  - `.kn5` models, versions 5 and 6.
  - DDS textures: DXT1/3/5, BC7 and uncompressed, with the old or the DX10 header. PNG/JPEG pictures stored in a model are decoded by the app.
  - Layouts from `models.ini` / `models_<layout>.ini` and `ui\<layout>\`, the surfaces from the track's and AC's own `surfaces.ini`, and the AI line from `fast_lane.ai`.
- **Tested on:** Mugello (Kunos) and Monaco 2020 with its F1 2022–2026 layouts (a mod). `tests\ac_scene.rs` runs against the AC install when it's there.
- **Not yet:**
  - AC's multi-layer road shaders, which show their main layer only.
  - Encrypted `.kn5` files, which are refused, and that's on purpose.
- **Make NR2003 track** (phase 2) fits NR2003 straights and curves to the AC track and saves it as `tracks\<name>\<name>.ptf` in your Sandbox workspace, with the textures it uses.
  - **What it follows:** AC's shape, width, hills and banking.
  - **The steps:**
    - **Centerline:** the middle between the track edges stored with AC's AI line, smoothed.
    - **Shape:** a continuous fit of the direction around the lap gives exact straights and arcs. Least squares then adjusts every arc so the lap stays within 0.3 m of AC and closes exactly.
    - **Height:** road heights come from AC's driving-surface meshes. Pieces are split wherever NR2003's per-piece height curve or width can't follow.
    - **Ground and walls (phase 3):** the ground is read across every piece from AC's driving surfaces, at its start, middle and end in 25 cm steps.
      - **Surfaces:** road, kerbs, grass, sand/gravel and concrete/pit become NR2003 strips with the matching surface (rumble strip, sand, grass…), so cars feel the right thing. Pieces are split where the layout changes along them.
      - **AC's game-wide surfaces:** the list in `system\data\surfaces.ini` (ROAD, GRASS, KERB, SAND) is included.
      - **Walls:** AC's walls become NR2003 walls at the same place and height. A safety wall goes at the edge of the ground only where AC has none.
      - **Unreadable ground** before a wall becomes grass, or concrete on tracks without grass.
      - **Textures** come from a template track, Watkins Glen by default (any installed track can be chosen), with texture coordinates re-scaled.
      - **Overlap and joins:** the ground stops halfway to any other stretch of the track, so parallel stretches never overlap, and neighbouring pieces meet within 5 cm.
      - **Fallback:** where AC's ground can't be read, the piece gets plain asphalt and grass.
  - **Results:**
    - Mugello: 404 pieces, within 0.22 m of AC (0.04 m on average), road height within 0.09 m. AC ground on every piece, 614 walls from AC, 241 kerb and 305 gravel strips.
    - Monaco F1 2026: 283 pieces, within 0.11 m.
    - Both pass the checker, and each takes about 1 s.
  - **After converting:** **Look at it** opens the result in the Look at a track tab, and an orange line shows it over the AC track. To try it in NR2003, use Build track with the same name.
  - **Tested across your AC tracks:** `examples\acbatch.rs` converts every installed AC track and layout. It fixed:
    - layouts kept only under `ui\<layout>`
    - lower-case `1_track` names
    - track edges missing from, or nonsense in, the AI file (now measured on AC's road)
    - kinked centerlines
    - turns tighter than the road is wide (the inside is clipped)
    - an endless loop
  - **Point-to-point roads** can't be converted (NR2003 tracks are loops), and the converter says so.
  - **Command line:** `cargo run --release --example acconvert -- <AC folder> <layout|-> <template folder> <out folder> <name>`.
    - `examples\acplot.rs` draws a top-down comparison picture.
    - `examples\acprobe.rs` lists AC's surfaces across the road.
- **Next phases:**
  4. Scenery and textures as `.3do`/`.mip`.
  5. AI lines, cameras and pit stalls.

## Where things are stored

- **Settings:** `%APPDATA%\TrackToolkit\settings.json`. Holds the Sandbox folder, the NR2003 folder, the workspace folder and the template track.
- **Per-track options:** `tracks\<name>\toolkit.json`.
- **Scratch files:** `tracks\<name>\_build\`.

## CLI

```
ttk [--settings FILE] [--track NAME] all
ttk --track mytrack textures pack install
ttk settings | tracks
```

## mklib quirks found while building this

- `mklib -u x.dat` unpacks into the *current directory* and keeps `trackmat\` paths.
- To build, the working form is `mklib [-c] [-xext] <path\to\file.def>`. The output name comes from `LIBNAME=`, and adding a second positional argument makes it fail with "Couldn't open .DEF file".
- `mklib -l` prints a header, a dashed line, `name size` rows, a blank line, and `Total Size: N bytes`.

## PSG and texture notes

- PSG definitions look like `name: EXPR`, and `OUTPUT` takes a defined name:

  ```
  ball: SCENE "ball76.3do"
  moved: TRANSFORM POS (0, 0, 2) ROT (45, 0, 0) ball
  OUTPUT moved "out.3do"
  ```
- makemip accepts RLE and uncompressed TGAs. It refuses sizes that aren't powers of two.

## Building

```
cargo build --release
cargo test
```

The C++ part (`ptfjson`, `ptfcheck`) builds with CMake and Ninja from a VS developer prompt:
`cmake -S cpp -B cpp\build -G Ninja -DCMAKE_BUILD_TYPE=Release && cmake --build cpp\build`
