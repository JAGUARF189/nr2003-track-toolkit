pub mod install;
pub mod models;
pub mod pack;
pub mod stalls;
pub mod textures;
pub mod workspace;

use crate::config::{Settings, TrackOpts};
use crate::logger::Logger;
use anyhow::{bail, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    Workspace,
    Textures,
    Models,
    Pack,
    Install,
    Stalls,
}

impl Step {
    pub const ALL: [Step; 6] = [
        Step::Workspace,
        Step::Textures,
        Step::Models,
        Step::Pack,
        Step::Install,
        Step::Stalls,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn title(self) -> &'static str {
        match self {
            Step::Workspace => "1. Workspace",
            Step::Textures => "2. Textures -> MIP",
            Step::Models => "3. PSG -> 3DO",
            Step::Pack => "4. Pack .dat",
            Step::Install => "5. Install into NR2003",
            Step::Stalls => "6. Pit stalls / grid",
        }
    }

    pub fn cli_name(self) -> &'static str {
        match self {
            Step::Workspace => "workspace",
            Step::Textures => "textures",
            Step::Models => "models",
            Step::Pack => "pack",
            Step::Install => "install",
            Step::Stalls => "stalls",
        }
    }

    pub fn from_cli(s: &str) -> Option<Step> {
        Step::ALL
            .into_iter()
            .find(|st| st.cli_name().eq_ignore_ascii_case(s))
    }

    pub fn run(self, ctx: &Ctx) -> Result<()> {
        match self {
            Step::Workspace => workspace::run(ctx),
            Step::Textures => textures::run(ctx),
            Step::Models => models::run(ctx),
            Step::Pack => pack::run(ctx),
            Step::Install => install::run(ctx),
            Step::Stalls => stalls::run(ctx),
        }
    }
}

/// The steps "Run all" executes, in order.
pub fn run_all_steps(opts: &TrackOpts) -> Vec<Step> {
    let mut v = vec![
        Step::Workspace,
        Step::Textures,
        Step::Models,
        Step::Pack,
        Step::Install,
    ];
    if opts.stalls.enabled {
        v.push(Step::Stalls);
    }
    v
}

pub struct Ctx<'a> {
    pub settings: &'a Settings,
    pub name: &'a str,
    pub opts: &'a TrackOpts,
    pub log: &'a Logger,
    pub cancel: &'a AtomicBool,
}

impl Ctx<'_> {
    pub fn track_dir(&self) -> PathBuf {
        self.settings.track_dir(self.name)
    }

    pub fn game_track_dir(&self) -> PathBuf {
        self.settings.game_tracks().join(self.name)
    }

    pub fn dat_name(&self) -> String {
        format!("{}.dat", self.name)
    }

    /// `tracks\<name>\_build\<sub>` scratch folder.
    pub fn build_dir(&self, sub: &str) -> Result<PathBuf> {
        let d = self.track_dir().join("_build").join(sub);
        std::fs::create_dir_all(&d)?;
        Ok(d)
    }

    pub fn tool(&self, exe: &str) -> PathBuf {
        self.settings.sandbox_dir.join(exe)
    }

    pub fn check_cancel(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        Ok(())
    }
}

pub fn validate_track_name(name: &str) -> Result<()> {
    let n = name.trim();
    if n.is_empty() {
        bail!("no track selected");
    }
    if n != name {
        bail!("track name has leading/trailing spaces");
    }
    if n.chars()
        .any(|c| r#"\/:*?"<>|"#.contains(c) || c.is_control())
    {
        bail!("track name '{n}' contains characters Windows doesn't allow in folder names");
    }
    if n.eq_ignore_ascii_case("shared") {
        bail!("'shared' is reserved for the unpacked shared.dat");
    }
    if n.starts_with('.') || n.starts_with('_') {
        bail!("track name shouldn't start with '.' or '_'");
    }
    Ok(())
}

/// Run `steps` in order, stopping at the first failure.
/// `on_event(step, None)` fires when a step starts, `Some(result)` when it ends.
pub fn run_steps(
    steps: &[Step],
    ctx: &Ctx,
    mut on_event: impl FnMut(Step, Option<&Result<()>>),
) -> Result<()> {
    for &s in steps {
        on_event(s, None);
        ctx.log.step(format!("== {} ==", s.title()));
        let r = s.run(ctx);
        if let Err(e) = &r {
            ctx.log.error(format!("{e:#}"));
        }
        on_event(s, Some(&r));
        r?;
    }
    Ok(())
}
