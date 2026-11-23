//! Command-line front end for the track pipeline.
//!
//!   ttk [--settings FILE] [--track NAME] <step>... | all
//!   ttk settings          show the settings in use
//!   ttk tracks            list workspace tracks
//!
//! Steps: workspace textures models pack install stalls

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use ttk::config::{Settings, TrackOpts};
use ttk::logger::{Level, Logger};
use ttk::mip::{self, Mip};
use ttk::steps::{run_all_steps, run_steps, Ctx, Step};

fn usage() -> ExitCode {
    eprintln!(
        "usage: ttk [--settings FILE] [--track NAME] <step>... | all\n       ttk settings | tracks\n\
         steps: {}",
        Step::ALL.iter().map(|s| s.cli_name()).collect::<Vec<_>>().join(" ")
    );
    ExitCode::from(2)
}

/// `ttk mip <file.mip> [out.png]`
fn mip_info(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("usage: ttk mip <file.mip> [out.png]");
        return ExitCode::from(2);
    };
    let m = match Mip::open(std::path::Path::new(path)) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("ERROR {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let i = &m.info;
    println!("{path}");
    println!(
        "  {}x{}  type {} ({})  mapping {} ({})  priority {:.2}  avg BGRA {:02X?}",
        i.width,
        i.height,
        i.kind,
        mip::type_name(i.kind),
        i.mapping,
        mip::mapping_name(i.mapping),
        i.priority,
        i.average_bgra
    );
    for (n, l) in m.levels.iter().enumerate() {
        println!(
            "  level {n}: {}x{} type {} pitch {} {} ({} bytes stored)",
            l.width,
            l.height,
            l.kind,
            l.pitch,
            if l.compressed { "DCL" } else { "raw" },
            l.data_len
        );
    }
    if let Some(out) = args.get(1) {
        match m
            .decode_largest()
            .and_then(|img| img.save(out).map_err(Into::into))
        {
            Ok(()) => println!("  saved {out}"),
            Err(e) => {
                eprintln!("ERROR {e:#}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

/// `ttk mipcheck <folder>...` — decode every level of every .mip.
fn mip_check(args: &[String]) -> ExitCode {
    let mut files = Vec::new();
    for a in args {
        let p = std::path::Path::new(a);
        if p.is_dir() {
            files.extend(
                ttk::util::list_files(p, &["mip".to_string(), "stp".to_string()])
                    .unwrap_or_default(),
            );
        } else {
            files.push(p.to_path_buf());
        }
    }
    let (mut ok, mut bad) = (0, 0);
    let mut kinds = std::collections::BTreeMap::<u8, usize>::new();
    let mut levels = 0usize;
    let mut dcl = 0usize;
    for f in &files {
        let r = Mip::open(f).and_then(|m| {
            *kinds.entry(m.info.kind).or_default() += 1;
            if m.levels.len() as u32 != m.info.levels_declared {
                anyhow::bail!(
                    "header says {} levels, file has {}",
                    m.info.levels_declared,
                    m.levels.len()
                );
            }
            let big = m.levels[m.largest_index()].clone();
            if (big.width, big.height) != (m.info.width, m.info.height) {
                anyhow::bail!(
                    "largest level {}x{} != header {}x{}",
                    big.width,
                    big.height,
                    m.info.width,
                    m.info.height
                );
            }
            for n in 0..m.levels.len() {
                m.decode_level(n)?;
                levels += 1;
                dcl += m.levels[n].compressed as usize;
            }
            Ok(())
        });
        match r {
            Ok(()) => ok += 1,
            Err(e) => {
                bad += 1;
                println!("FAIL {}: {e:#}", f.display());
            }
        }
    }
    println!(
        "{ok} ok, {bad} failed, {levels} levels decoded ({dcl} DCL-compressed); types: {kinds:?}"
    );
    if bad > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// `ttk mipdiff <file.mip> <reference image>` — per-channel error of the largest level.
/// Ignores color where the reference is fully transparent (DXT/1555 don't keep it).
fn mip_diff(args: &[String]) -> ExitCode {
    let (Some(mp), Some(rp)) = (args.first(), args.get(1)) else {
        eprintln!("usage: ttk mipdiff <file.mip> <reference.tga|png>");
        return ExitCode::from(2);
    };
    let r = (|| -> anyhow::Result<()> {
        let m = Mip::open(std::path::Path::new(mp))?;
        let got = m.decode_largest()?;
        let want = image::open(rp)?.to_rgba8();
        anyhow::ensure!(
            got.dimensions() == want.dimensions(),
            "size {:?} vs {:?}",
            got.dimensions(),
            want.dimensions()
        );
        let mut max = [0u32; 4];
        let mut sum = [0u64; 4];
        let mut n = 0u64;
        for (a, b) in got.pixels().zip(want.pixels()) {
            n += 1;
            let channels = if b.0[3] == 0 { 3..4 } else { 0..4 };
            for c in channels {
                let d = (a.0[c] as i32 - b.0[c] as i32).unsigned_abs();
                max[c] = max[c].max(d);
                sum[c] += d as u64;
            }
        }
        let mean: Vec<String> = sum
            .iter()
            .map(|s| format!("{:.2}", *s as f64 / n as f64))
            .collect();
        println!(
            "type {:2} {:<38} max err R{} G{} B{} A{}  mean {}",
            m.info.kind,
            mip::type_name(m.info.kind),
            max[0],
            max[1],
            max[2],
            max[3],
            mean.join("/")
        );
        Ok(())
    })();
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ERROR {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// `ttk mipbuild <picture> <target.mip> [--surface] [--best]`
/// If target exists, its settings are kept (replace); otherwise new-texture defaults.
fn mip_build(settings: &Settings, args: &[String]) -> ExitCode {
    use ttk::mip::build::{build, Recipe};
    let (Some(pic), Some(target)) = (args.first(), args.get(1)) else {
        eprintln!("usage: ttk mipbuild <picture> <target.mip> [--surface] [--best]");
        return ExitCode::from(2);
    };
    let target = std::path::Path::new(target);
    let surface = args.iter().any(|a| a == "--surface");
    let best = args.iter().any(|a| a == "--best");
    let r = (|| -> anyhow::Result<()> {
        let recipe = if target.is_file() {
            Recipe::matching(&Mip::open(target)?)
        } else {
            Recipe::new_texture(surface, best)
        };
        println!("recipe: {recipe:?}");
        let work = std::env::temp_dir().join(format!("ttk_mipbuild_{}", std::process::id()));
        let b = build(
            &settings.sandbox_dir,
            std::path::Path::new(pic),
            target,
            &recipe,
            &work,
        );
        let _ = std::fs::remove_dir_all(&work);
        let b = b?;
        println!(
            "built {}: {}x{} type {} mapping {} priority {} levels {}",
            b.target.display(),
            b.info.width,
            b.info.height,
            b.info.kind,
            b.info.mapping,
            b.info.priority,
            b.info.levels_declared
        );
        if let Some(bk) = &b.backup {
            println!("backup: {}", bk.display());
        }
        for n in &b.notes {
            println!("note: {n}");
        }
        Ok(())
    })();
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ERROR {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// `ttk dat <file.dat>`                      list
/// `ttk dat <file.dat> --check <unpackdir>`  compare every entry with mklib -u output
/// `ttk dat <file.dat> --textures`           decode every .mip/.stp inside
fn dat_cmd(args: &[String]) -> ExitCode {
    use ttk::dat::Dat;
    let Some(path) = args.first() else {
        eprintln!("usage: ttk dat <file.dat> [--check <unpackdir> | --textures]");
        return ExitCode::from(2);
    };
    let r = (|| -> anyhow::Result<bool> {
        let d = Dat::open(std::path::Path::new(path))?;
        let comp = d.entries.iter().filter(|e| e.compressed).count();
        println!("{path}: {} entries ({comp} compressed)", d.entries.len());
        match args.get(1).map(|s| s.as_str()) {
            Some("--check") => {
                let dir = std::path::Path::new(args.get(2).map(|s| s.as_str()).unwrap_or("."));
                let mut bad = 0;
                for e in &d.entries {
                    let want = std::fs::read(dir.join(e.relative_path()?));
                    match (d.read(e), want) {
                        (Ok(a), Ok(b)) if a == b => {}
                        (Ok(_), Ok(_)) => {
                            bad += 1;
                            println!("DIFFERS {}", e.name)
                        }
                        (Err(x), _) => {
                            bad += 1;
                            println!("FAIL {}: {x:#}", e.name)
                        }
                        (_, Err(x)) => {
                            bad += 1;
                            println!("MISSING {}: {x}", e.name)
                        }
                    }
                }
                println!(
                    "{} of {} entries byte-identical to the unpacked files",
                    d.entries.len() - bad,
                    d.entries.len()
                );
                Ok(bad == 0)
            }
            Some("--extract") => {
                let dir = std::path::Path::new(args.get(2).map(|s| s.as_str()).unwrap_or("."));
                let mut n = 0;
                for e in &d.entries {
                    d.extract(e, dir)?;
                    n += 1;
                }
                println!("extracted {n} files to {}", dir.display());
                Ok(true)
            }
            Some("--textures") => {
                let (mut ok, mut bad) = (0, 0);
                for e in d
                    .entries
                    .iter()
                    .filter(|e| matches!(e.ext_lower().as_str(), "mip" | "stp"))
                {
                    match d
                        .read(e)
                        .and_then(Mip::parse)
                        .and_then(|m| m.decode_largest())
                    {
                        Ok(_) => ok += 1,
                        Err(x) => {
                            bad += 1;
                            println!("FAIL {}: {x:#}", e.name)
                        }
                    }
                }
                println!("{ok} textures decoded, {bad} failed");
                Ok(bad == 0)
            }
            _ => {
                for e in &d.entries {
                    println!(
                        "  {:>9} {}{}",
                        e.size,
                        e.name,
                        if e.compressed { "  (compressed)" } else { "" }
                    );
                }
                Ok(true)
            }
        }
    })();
    match r {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("ERROR {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut settings_path: Option<PathBuf> = None;
    let mut track: Option<String> = None;
    let mut words: Vec<String> = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--settings" => settings_path = args.next().map(PathBuf::from),
            "--track" => track = args.next(),
            "-h" | "--help" => return usage(),
            _ => words.push(a),
        }
    }
    let settings_file = settings_path.unwrap_or_else(Settings::default_path);
    let settings = Settings::load_from(&settings_file);

    match words.first().map(|s| s.as_str()) {
        None => return usage(),
        Some("settings") => {
            println!("# {}", settings_file.display());
            println!("{}", serde_json::to_string_pretty(&settings).unwrap());
            return ExitCode::SUCCESS;
        }
        Some("tracks") => {
            for t in settings.list_tracks() {
                println!("{t}");
            }
            return ExitCode::SUCCESS;
        }
        Some("mip") => return mip_info(&words[1..]),
        Some("mipcheck") => return mip_check(&words[1..]),
        Some("mipdiff") => return mip_diff(&words[1..]),
        Some("mipbuild") => return mip_build(&settings, &words[1..]),
        Some("dat") => return dat_cmd(&words[1..]),
        _ => {}
    }

    let name = match track
        .or_else(|| (!settings.last_track.is_empty()).then(|| settings.last_track.clone()))
    {
        Some(n) => n,
        None => {
            eprintln!("no track given (use --track NAME)");
            return ExitCode::from(2);
        }
    };
    let opts = TrackOpts::load(&settings.track_dir(&name));

    let steps: Vec<Step> = if words.len() == 1 && words[0].eq_ignore_ascii_case("all") {
        run_all_steps(&opts)
    } else {
        let mut v = Vec::new();
        for w in &words {
            match Step::from_cli(w) {
                Some(s) => v.push(s),
                None => {
                    eprintln!("unknown step '{w}'");
                    return usage();
                }
            }
        }
        v
    };

    let log = Logger::new(|lvl, msg| {
        let tag = match lvl {
            Level::Step => "",
            Level::Info => "  ",
            Level::Ok => "OK ",
            Level::Warn => "WARN ",
            Level::Error => "ERROR ",
            Level::Tool => "  | ",
        };
        if lvl == Level::Error {
            eprintln!("{tag}{msg}");
        } else {
            println!("{tag}{msg}");
        }
    });
    let cancel = AtomicBool::new(false);
    let ctx = Ctx {
        settings: &settings,
        name: &name,
        opts: &opts,
        log: &log,
        cancel: &cancel,
    };
    match run_steps(&steps, &ctx, |_, _| {}) {
        Ok(()) => {
            println!("Done.");
            ExitCode::SUCCESS
        }
        Err(_) => ExitCode::FAILURE,
    }
}
