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
