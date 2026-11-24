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

`track-toolkit.exe <file.ptf|.dat> [--3d] [--piece N] [--objects]` opens straight into this tab.

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
