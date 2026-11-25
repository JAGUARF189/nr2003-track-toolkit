//! Track Toolkit core: the NR2003 track build pipeline, independent of any UI.
//!
//! Steps (see [`steps::Step`]):
//! 1. Workspace — `tracks\<name>\trackmat`, unpacked `tracks\shared`, flagger textures
//! 2. Textures  — PNG/JPG/BMP/TGA -> TGA -> MIP (`makemip`)
//! 3. Models    — `.psg` -> `.3do` (`make3do -s`)
//! 4. Pack      — generated `.def` -> `<name>.dat` (`mklib`)
//! 5. Install   — copy into NR2003 `tracks\<name>` with template ini/cam/sim/lp files
//! 6. Stalls    — `mkstalls` on an LP file, merged into track.ini

pub mod ac;
pub mod acconv;
pub mod acscenery;
pub mod acsupport;
pub mod config;
pub mod dat;
pub mod inifile;
pub mod logger;
pub mod mip;
pub mod runner;
pub mod steps;
pub mod tga;
pub mod track;
pub mod util;
