//! Saving: Rust graph -> JSON -> ptfjson -> .ptf must reproduce files byte for byte, and an
//! edit must change only what it should. Skipped when the test tracks aren't on this machine.
use serde_json::json;
use std::path::{Path, PathBuf};
use ttk::track::{graph, save};

fn test_file(rel: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    (p.is_file() && graph::find_ptfjson().is_some()).then_some(p)
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ttk_save_test_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn unchanged_tracks_save_byte_identical() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("_test/ptfs");
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let out = scratch("same");
    let mut n = 0;
    for e in entries.flatten() {
        for f in std::fs::read_dir(e.path()).into_iter().flatten().flatten() {
            let p = f.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ptf")) && graph::find_ptfjson().is_some() {
                let original = std::fs::read(&p).unwrap();
                let g = graph::parse_ptf(&original).unwrap();
                let dest = out.join(p.file_name().unwrap());
                save::save_graph(&g, &dest).unwrap();
                assert!(std::fs::read(&dest).unwrap() == original, "{} changed when saved", p.display());
                n += 1;
            }
        }
    }
    eprintln!("{n} tracks saved byte-identical");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn night_edit_changes_only_the_lighting_and_keeps_a_backup() {
    let Some(src) = test_file("_test/ptfs/martinsville/martinsville.ptf") else { return };
    let out = scratch("night");
    let dest = out.join("martinsville.ptf");
    std::fs::copy(&src, &dest).unwrap();
    let original = std::fs::read(&dest).unwrap();

    let mut g = graph::read_ptf_file(&dest).unwrap();
    let root = g.root;
    let r = g.node_mut(root).unwrap();
    r.set("night", json!(1)).unwrap();
    for k in 0..3 {
        r.set(&format!("light{k}"), json!(0.5)).unwrap();
    }
    let saved = save::save_graph(&g, &dest).unwrap();

    // The old version went to _ttk_backup and is the original file.
    let backup = saved.backup.expect("a backup of the previous version");
    assert_eq!(std::fs::read(&backup).unwrap(), original);
    assert_eq!(save::backups(&dest).len(), 1);

    // Same size, and only the night flag (1 byte) and three doubles (24 bytes) differ,
    // all inside the root object's header.
    let now = std::fs::read(&dest).unwrap();
    assert_eq!(now.len(), original.len());
    let diffs: Vec<usize> = (0..now.len()).filter(|&i| now[i] != original[i]).collect();
    assert!(!diffs.is_empty() && diffs.len() <= 25, "{} bytes differ", diffs.len());
    assert!(diffs.iter().all(|&i| i < 0x50), "a change landed outside the header: {diffs:?}");

    let back = graph::read_ptf_file(&dest).unwrap();
    assert_eq!(back.root().i("night"), 1);
    assert_eq!(back.root().f("light1"), 0.5);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn a_failed_save_leaves_the_file_alone() {
    let Some(src) = test_file("_test/ptfs/martinsville/martinsville.ptf") else { return };
    let out = scratch("fail");
    let dest = out.join("martinsville.ptf");
    std::fs::copy(&src, &dest).unwrap();
    let original = std::fs::read(&dest).unwrap();

    // Break the graph: a field with the wrong name can't be written.
    let mut g = graph::read_ptf_file(&dest).unwrap();
    let root = g.root;
    g.node_mut(root).unwrap().fields[3].0 = "not_a_field".into();
    let err = save::save_graph(&g, &dest).unwrap_err();
    assert!(format!("{err:#}").contains("not changed"), "{err:#}");
    assert_eq!(std::fs::read(&dest).unwrap(), original);
    assert!(!dest.with_extension("ptf.saving").exists());
    assert!(save::backups(&dest).is_empty());
    let _ = std::fs::remove_dir_all(&out);
}

/// Every .ptf under the NR2003 tracks folder: `cargo test --release -- --ignored all_game_tracks`
#[test]
#[ignore]
fn all_game_tracks_save_byte_identical() {
    let root = Path::new(r"C:\Papyrus\NASCAR Racing 2003 Season\tracks");
    let mut stack = vec![root.to_path_buf()];
    let out = scratch("all");
    let (mut ok, mut bad) = (0, Vec::new());
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ptf")) {
                let original = std::fs::read(&p).unwrap();
                let dest = out.join("t.ptf");
                let r = graph::parse_ptf(&original).and_then(|g| save::save_graph(&g, &dest));
                match r {
                    Ok(_) if std::fs::read(&dest).unwrap() == original => ok += 1,
                    Ok(_) => bad.push(format!("{} (bytes differ)", p.display())),
                    Err(e) => bad.push(format!("{}: {e:#}", p.display())),
                }
                let _ = std::fs::remove_file(&dest);
            }
        }
    }
    eprintln!("{ok} game tracks saved byte-identical");
    assert!(bad.is_empty(), "{bad:#?}");
    let _ = std::fs::remove_dir_all(&out);
}
