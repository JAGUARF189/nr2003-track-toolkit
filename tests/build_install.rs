//! "Build and install" (the fully automated build) into a fake NR2003 folder in temp: the real
//! game folder is never touched. Needs the Sandbox tools (mklib) and the test track.
use std::path::Path;
use ttk::config::Settings;
use ttk::logger::{Level, Logger};
use ttk::steps;

#[test]
fn builds_and_installs_into_the_game_folder() {
    let real = Settings::default();
    let ptf = Path::new(env!("CARGO_MANIFEST_DIR")).join("_test/ptfs/martinsville/martinsville.ptf");
    if !real.sandbox_dir.join("mklib.exe").is_file() || !ptf.is_file() {
        eprintln!("skipping: mklib.exe or the test track missing");
        return;
    }
    let root = std::env::temp_dir().join(format!("ttk_build_install_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let game = root.join("NR2003");
    let ws = root.join("workspace");
    // a template track with support files, as NR2003 has them
    let tpl = game.join("tracks").join("tpl");
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("track.ini"), "[track]\ntrack_name = TEMPLATE\ntrack_name_short = tpl\n").unwrap();
    std::fs::write(tpl.join("tpl.cam"), b"cam").unwrap();
    std::fs::write(tpl.join("race.lp"), b"lp").unwrap();
    // the shared textures already unpacked in the workspace
    std::fs::create_dir_all(ws.join("shared")).unwrap();
    std::fs::write(ws.join("shared").join(".ttk_unpacked"), b"").unwrap();
    // the track to build: a saved .ptf in its workspace folder
    let name = "autotest";
    std::fs::create_dir_all(ws.join(name)).unwrap();
    std::fs::copy(&ptf, ws.join(name).join(format!("{name}.ptf"))).unwrap();

    let mut settings = real.clone();
    settings.nr2003_dir = game.clone();
    settings.workspace_dir = ws.clone();
    settings.template_track = "tpl".into();
    let warnings = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let w2 = warnings.clone();
    let log = Logger::new(move |l, m| {
        if l == Level::Warn {
            w2.lock().unwrap().push(m)
        }
    });

    steps::build_and_install(&settings, name, &log).unwrap();
    let inst = game.join("tracks").join(name);
    assert!(inst.join(format!("{name}.dat")).is_file(), "the .dat is installed");
    assert!(inst.join(format!("{name}.cam")).is_file() && inst.join("race.lp").is_file(), "support files from the template");
    let ini = std::fs::read_to_string(inst.join("track.ini")).unwrap();
    assert!(ini.contains("AUTOTEST"), "track.ini names the new track: {ini}");
    assert!(warnings.lock().unwrap().iter().any(|w| w.contains(".lp")), "it says the .lp files are placeholders");

    // again: the installed .dat is backed up, not lost
    steps::build_and_install(&settings, name, &log).unwrap();
    assert!(inst.join("_ttk_backup").is_dir(), "the previous .dat was backed up");

    // with a game program running it refuses (cargo.exe runs these tests, so a game folder
    // holding a cargo.exe looks like one is running)
    std::fs::write(game.join("cargo.exe"), b"").unwrap();
    assert_eq!(steps::game_programs_running(&settings), vec!["cargo.exe".to_string()]);
    let e = steps::build_and_install(&settings, name, &log).unwrap_err();
    assert!(format!("{e}").contains("is running"), "{e}");
    let _ = std::fs::remove_dir_all(&root);
}
