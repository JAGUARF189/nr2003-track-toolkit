//! "Textures" tab: browse textures (.mip) and menu pictures (.stp) in a folder
//! or inside a .dat, look at one big, save as PNG, replace with your own
//! picture, drop pictures in to make new ones, or take files out of a .dat.
//! Designed to need no knowledge of mip types: everything is in plain words.

use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Id, LayerId, Order, Rect, RichText, Sense,
    TextureHandle, TextureOptions, Vec2,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use ttk::dat::Dat;
use ttk::mip::build::{self, Recipe};
use ttk::mip::{self, Mip, MipInfo};

const THUMB: u32 = 128;
const TILE: Vec2 = Vec2::new(144.0, 168.0);
const GOOD: Color32 = Color32::from_rgb(70, 180, 90);
const BAD: Color32 = Color32::from_rgb(230, 80, 70);

/// What the tab needs from the rest of the app.
pub struct TexturesEnv<'a> {
    /// Folder shortcuts: (button label, folder).
    pub places: &'a [(String, PathBuf)],
    /// Installed tracks: (name, path of its .dat).
    pub game_dats: &'a [(String, PathBuf)],
    /// Suggested parent folder for "Take out everything".
    pub unpack_root: &'a Path,
    pub sandbox: &'a Path,
}

#[derive(Clone)]
enum Source {
    Folder(PathBuf),
    Dat(Arc<Dat>),
}

#[derive(Clone, PartialEq)]
enum Origin {
    File(PathBuf),
    /// Index into the open .dat's entries.
    Entry(usize),
}

struct Thumb {
    origin: Origin,
    /// Shown under the thumbnail (for .dat entries, without the folder).
    name: String,
    /// Full name (for .dat entries, e.g. trackmat\apron.mip).
    full_name: String,
    info: Option<MipInfo>,
    tex: Option<TextureHandle>,
    error: Option<String>,
}

#[derive(Clone)]
struct Job {
    picture: PathBuf,
    target: PathBuf,
    recipe: Recipe,
}

struct JobResult {
    job: Job,
    outcome: Result<(Option<PathBuf>, Vec<String>), String>, // (backup, notes)
}

struct UndoItem {
    target: PathBuf,
    /// The previous file, or None if the texture was brand new.
    backup: Option<PathBuf>,
}

/// Work waiting for a yes/no from the user.
enum Pending {
    Build {
        jobs: Vec<Job>,
        replacing: Vec<String>,
    },
    Extract {
        dat: Arc<Dat>,
        dir: PathBuf,
        existing: usize,
    },
}

enum Msg {
    Thumb {
        gen: u64,
        index: usize,
        image: Option<ColorImage>,
        info: Option<MipInfo>,
        error: Option<String>,
    },
    ThumbsDone {
        gen: u64,
    },
    Export {
        done: usize,
        total: usize,
        failed: usize,
        folder: PathBuf,
        finished: bool,
    },
    Progress(String),
    Built(Vec<JobResult>),
    Extracted {
        dir: PathBuf,
        written: usize,
        failed: Vec<String>,
    },
}

pub struct TexturesView {
    source: Option<Source>,
    thumbs: Vec<Thumb>,
    selected: Option<usize>,
    big: Option<(usize, TextureHandle)>,
    big_error: Option<String>,
    big_alpha: mip::Transparency,
    gen: Arc<AtomicU64>,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    loading: bool,
    /// Exporting, building or extracting.
    working: bool,
    status: String,
    report: Vec<(bool, String)>,
    filter: String,
    use_picture_size: bool,
    best_quality: bool,
    undo: Vec<UndoItem>,
    pending: Option<Pending>,
    select_after: Option<PathBuf>,
    track_pick: String,
}

impl Default for TexturesView {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            source: None,
            thumbs: Vec::new(),
            selected: None,
            big: None,
            big_error: None,
            big_alpha: mip::Transparency::None,
            gen: Arc::new(AtomicU64::new(0)),
            tx,
            rx,
            loading: false,
            working: false,
            status: String::new(),
            report: Vec::new(),
            filter: String::new(),
            use_picture_size: false,
            best_quality: false,
            undo: Vec::new(),
            pending: None,
            select_after: None,
            track_pick: String::new(),
        }
    }
}

// ---------- helpers ----------

fn pictures_dir(name: &str) -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Pictures")
        .join("NR2003 Textures")
        .join(name)
}

fn work_dir() -> PathBuf {
    std::env::temp_dir()
        .join("TrackToolkit")
        .join(format!("mipbuild_{}", std::process::id()))
}

fn to_color_image(img: &image::RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

fn paint_checker(painter: &egui::Painter, rect: Rect) {
    painter.rect_filled(rect, 0.0, Color32::from_gray(200));
    let s = 8.0;
    let cols = (rect.width() / s).ceil() as i32;
    let rows = (rect.height() / s).ceil() as i32;
    for y in 0..rows {
        for x in 0..cols {
            if (x + y) % 2 == 0 {
                let r = Rect::from_min_size(
                    rect.min + Vec2::new(x as f32 * s, y as f32 * s),
                    Vec2::splat(s),
                )
                .intersect(rect);
                painter.rect_filled(r, 0.0, Color32::from_gray(150));
            }
        }
    }
}

fn fit(size: Vec2, max: Vec2) -> Vec2 {
    let k = (max.x / size.x).min(max.y / size.y);
    size * k
}

fn friendly_priority(p: f32) -> String {
    let n = (p * 10.0).round() as i32;
    match n {
        ..=0 => "normal".into(),
        1..=2 => format!("{n} (scenery)"),
        3..=7 => format!("{n} (track surface)"),
        _ => format!("{n} (stays sharp longest)"),
    }
}

fn stem_of(name: &str) -> String {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    file.rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(file)
        .to_string()
}

fn pick_pictures(title: &str, many: bool) -> Vec<PathBuf> {
    let d = rfd::FileDialog::new()
        .set_title(title)
        .add_filter("Pictures", &["png", "jpg", "jpeg", "bmp", "tga"]);
    if many {
        d.pick_files().unwrap_or_default()
    } else {
        d.pick_file().into_iter().collect()
    }
}

fn big_button(ui: &egui::Ui, text: &str, size: f32, height: f32) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).size(size).strong())
        .min_size(Vec2::new(ui.available_width(), height))
}

fn load(source: &Source, origin: &Origin) -> anyhow::Result<Mip> {
    match (source, origin) {
        (_, Origin::File(p)) => Mip::open(p),
        (Source::Dat(d), Origin::Entry(i)) => {
            let e = d
                .entries
                .get(*i)
                .ok_or_else(|| anyhow::anyhow!("entry {i} is missing"))?;
            d.read(e).and_then(Mip::parse)
        }
        (Source::Folder(_), Origin::Entry(_)) => anyhow::bail!("not inside a .dat"),
    }
}

fn thumbnail(m: &Mip) -> anyhow::Result<ColorImage> {
    let img = m.decode_level(m.level_for(THUMB))?;
    let img = if img.width() > THUMB || img.height() > THUMB {
        let s = fit(
            Vec2::new(img.width() as f32, img.height() as f32),
            Vec2::splat(THUMB as f32),
        );
        image::imageops::thumbnail(&img, (s.x as u32).max(1), (s.y as u32).max(1))
    } else {
        img
    };
    Ok(to_color_image(&img))
}

impl TexturesView {
    fn folder(&self) -> Option<&Path> {
        match &self.source {
            Some(Source::Folder(p)) => Some(p),
            _ => None,
        }
    }

    fn dat(&self) -> Option<&Arc<Dat>> {
        match &self.source {
            Some(Source::Dat(d)) => Some(d),
            _ => None,
        }
    }

    fn source_label(&self) -> String {
        match &self.source {
            Some(Source::Folder(p)) => p.display().to_string(),
            Some(Source::Dat(d)) => format!("Inside {} (look only)", ttk::util::file_name(&d.path)),
            None => String::new(),
        }
    }

    fn source_short_name(&self) -> String {
        match &self.source {
            Some(Source::Folder(p)) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Some(Source::Dat(d)) => stem_of(&d.path.to_string_lossy()),
            None => "textures".into(),
        }
    }

    // ---------- opening ----------

    pub fn open_folder(&mut self, ctx: &egui::Context, folder: PathBuf) {
        let files = ttk::util::list_files(&folder, &["mip".to_string(), "stp".to_string()])
            .unwrap_or_default();
        let thumbs = files
            .iter()
            .map(|p| {
                let n = ttk::util::file_name(p);
                Thumb {
                    origin: Origin::File(p.clone()),
                    name: n.clone(),
                    full_name: n,
                    info: None,
                    tex: None,
                    error: None,
                }
            })
            .collect();
        let changed = self.folder() != Some(folder.as_path());
        self.start_loading(ctx, Source::Folder(folder), thumbs, changed);
        if self.thumbs.is_empty() {
            self.status =
                "No textures here yet. Drag pictures onto this window to make some.".into();
        }
    }

    pub fn open_dat(&mut self, ctx: &egui::Context, path: &Path) {
        match Dat::open(path) {
            Ok(d) => {
                let thumbs = d
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| build::is_texture_ext(&e.ext_lower()))
                    .map(|(i, e)| Thumb {
                        origin: Origin::Entry(i),
                        name: e.file_name().to_string(),
                        full_name: e.name.clone(),
                        info: None,
                        tex: None,
                        error: None,
                    })
                    .collect();
                let total = d.entries.len();
                self.start_loading(ctx, Source::Dat(Arc::new(d)), thumbs, true);
                self.status = format!("{} pictures ({} files in all)", self.thumbs.len(), total);
            }
            Err(e) => self.report = vec![(false, format!("Couldn't open that .dat: {e:#}"))],
        }
    }

    fn start_loading(
        &mut self,
        ctx: &egui::Context,
        source: Source,
        thumbs: Vec<Thumb>,
        new_place: bool,
    ) {
        let gen = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
        self.thumbs = thumbs;
        self.selected = None;
        self.big = None;
        self.big_error = None;
        self.status = format!("{} pictures", self.thumbs.len());
        if new_place {
            self.report.clear();
            self.undo.clear();
            self.filter.clear();
        }
        self.source = Some(source.clone());
        self.loading = !self.thumbs.is_empty();

        let origins: Vec<Origin> = self.thumbs.iter().map(|t| t.origin.clone()).collect();
        let tx = self.tx.clone();
        let gen_now = self.gen.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for (index, o) in origins.iter().enumerate() {
                if gen_now.load(Ordering::SeqCst) != gen {
                    return; // a different place was opened
                }
                let msg = match load(&source, o).and_then(|m| Ok((thumbnail(&m)?, m.info.clone())))
                {
                    Ok((image, info)) => Msg::Thumb {
                        gen,
                        index,
                        image: Some(image),
                        info: Some(info),
                        error: None,
                    },
                    Err(e) => Msg::Thumb {
                        gen,
                        index,
                        image: None,
                        info: None,
                        error: Some(format!("{e:#}")),
                    },
                };
                if tx.send(msg).is_err() {
                    return;
                }
                if index % 8 == 0 {
                    ctx.request_repaint();
                }
            }
            let _ = tx.send(Msg::ThumbsDone { gen });
            ctx.request_repaint();
        });
    }

    fn refresh(&mut self, ctx: &egui::Context, select: Option<PathBuf>) {
        if let Some(f) = self.folder().map(Path::to_path_buf) {
            self.open_folder(ctx, f);
        }
        if let Some(p) = select {
            if let Some(i) = self
                .thumbs
                .iter()
                .position(|t| t.origin == Origin::File(p.clone()))
            {
                self.select(ctx, i);
            }
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let cur = self.gen.load(Ordering::SeqCst);
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Thumb {
                    gen,
                    index,
                    image,
                    info,
                    error,
                } if gen == cur => {
                    if let Some(t) = self.thumbs.get_mut(index) {
                        t.info = info;
                        t.error = error;
                        if let Some(img) = image {
                            t.tex = Some(ctx.load_texture(
                                format!("thumb-{gen}-{index}"),
                                img,
                                TextureOptions::LINEAR,
                            ));
                        }
                    }
                }
                Msg::ThumbsDone { gen } if gen == cur => self.loading = false,
                Msg::Export {
                    done,
                    total,
                    failed,
                    folder,
                    finished,
                } => {
                    self.status = if finished {
                        self.working = false;
                        if failed == 0 {
                            format!("Saved {done} pictures to {}", folder.display())
                        } else {
                            format!(
                                "Saved {} pictures to {} ({failed} could not be read)",
                                done - failed,
                                folder.display()
                            )
                        }
                    } else {
                        format!("Saving pictures… {done} of {total}")
                    };
                }
                Msg::Progress(s) => self.status = s,
                Msg::Built(results) => self.finish_build(ctx, results),
                Msg::Extracted {
                    dir,
                    written,
                    failed,
                } => {
                    self.working = false;
                    self.open_folder(ctx, dir.clone());
                    self.status = format!("Took out {written} files.");
                    let mut report = vec![
                        (true, format!("They're in {}", dir.display())),
                        (
                            true,
                            "You're now looking at them, so you can replace any texture here."
                                .into(),
                        ),
                    ];
                    report.extend(failed.into_iter().map(|f| (false, f)));
                    self.report = report;
                }
                _ => {} // stale message from a previous place
            }
        }
    }

    fn select(&mut self, ctx: &egui::Context, i: usize) {
        self.selected = Some(i);
        self.big = None;
        self.big_error = None;
        let Some(source) = self.source.clone() else {
            return;
        };
        match load(&source, &self.thumbs[i].origin).and_then(|m| m.decode_largest()) {
            Ok(img) => {
                self.big_alpha = mip::Transparency::of(&img);
                let tex = ctx.load_texture(
                    format!(
                        "big-{}-{}",
                        self.thumbs[i].full_name,
                        self.gen.load(Ordering::SeqCst)
                    ),
                    to_color_image(&img),
                    TextureOptions::LINEAR,
                );
                self.big = Some((i, tex));
            }
            Err(e) => self.big_error = Some(format!("{e:#}")),
        }
    }

    // ---------- saving pictures ----------

    fn save_one(&mut self, i: usize) {
        let Some(source) = self.source.clone() else {
            return;
        };
        let stem = stem_of(&self.thumbs[i].full_name);
        let dir = pictures_dir(&self.source_short_name());
        let _ = std::fs::create_dir_all(&dir);
        let Some(out) = rfd::FileDialog::new()
            .set_title("Save this picture")
            .set_directory(&dir)
            .set_file_name(format!("{stem}.png"))
            .add_filter("PNG picture", &["png"])
            .save_file()
        else {
            return;
        };
        self.status = match load(&source, &self.thumbs[i].origin)
            .and_then(|m| m.decode_largest())
            .and_then(|img| Ok(img.save(&out)?))
        {
            Ok(()) => format!("Saved {}", out.display()),
            Err(e) => format!("Couldn't save: {e:#}"),
        };
    }

    fn save_all(&mut self, ctx: &egui::Context) {
        let Some(source) = self.source.clone() else {
            return;
        };
        let start = pictures_dir(&self.source_short_name());
        let _ = std::fs::create_dir_all(&start);
        let Some(out_dir) = rfd::FileDialog::new()
            .set_title("Choose where to put the pictures")
            .set_directory(&start)
            .pick_folder()
        else {
            return;
        };
        let items: Vec<(Origin, String)> = self
            .thumbs
            .iter()
            .map(|t| (t.origin.clone(), stem_of(&t.full_name)))
            .collect();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.working = true;
        std::thread::spawn(move || {
            let total = items.len();
            let mut failed = 0;
            for (n, (o, stem)) in items.iter().enumerate() {
                let r = load(&source, o)
                    .and_then(|m| m.decode_largest())
                    .and_then(|img| Ok(img.save(out_dir.join(format!("{stem}.png")))?));
                if r.is_err() {
                    failed += 1;
                }
                let _ = tx.send(Msg::Export {
                    done: n + 1,
                    total,
                    failed,
                    folder: out_dir.clone(),
                    finished: false,
                });
                ctx.request_repaint();
            }
            let _ = tx.send(Msg::Export {
                done: total,
                total,
                failed,
                folder: out_dir,
                finished: true,
            });
            ctx.request_repaint();
        });
    }

    // ---------- taking files out of a .dat ----------

    fn take_out_one(&mut self, i: usize) {
        let (Some(d), Origin::Entry(ei)) = (self.dat().cloned(), self.thumbs[i].origin.clone())
        else {
            return;
        };
        let e = &d.entries[ei];
        let Some(out) = rfd::FileDialog::new()
            .set_title("Save this file")
            .set_file_name(e.file_name())
            .save_file()
        else {
            return;
        };
        self.report = vec![match d.read(e).and_then(|b| Ok(std::fs::write(&out, b)?)) {
            Ok(()) => (true, format!("Saved {}", out.display())),
            Err(x) => (false, format!("Couldn't take it out: {x:#}")),
        }];
    }

    fn take_out_all(&mut self, ctx: &egui::Context, unpack_root: &Path) {
        let Some(d) = self.dat().cloned() else { return };
        let suggested = unpack_root.join(stem_of(&d.path.to_string_lossy()));
        let _ = std::fs::create_dir_all(&suggested);
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Choose a folder for the files")
            .set_directory(&suggested)
            .pick_folder()
        else {
            return;
        };
        let existing = d
            .entries
            .iter()
            .filter(|e| {
                e.relative_path()
                    .map(|r| dir.join(r).exists())
                    .unwrap_or(false)
            })
            .count();
        if existing > 0 {
            self.pending = Some(Pending::Extract {
                dat: d,
                dir,
                existing,
            });
        } else {
            self.run_extract(ctx, d, dir);
        }
    }

    fn run_extract(&mut self, ctx: &egui::Context, d: Arc<Dat>, dir: PathBuf) {
        self.working = true;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let total = d.entries.len();
            let (mut written, mut failed) = (0, Vec::new());
            for (n, e) in d.entries.iter().enumerate() {
                match d.extract(e, &dir) {
                    Ok(_) => written += 1,
                    Err(x) => failed.push(format!("{}: {x:#}", e.name)),
                }
                if n % 20 == 0 {
                    let _ = tx.send(Msg::Progress(format!(
                        "Taking files out… {} of {total}",
                        n + 1
                    )));
                    ctx.request_repaint();
                }
            }
            let _ = tx.send(Msg::Extracted {
                dir,
                written,
                failed,
            });
            ctx.request_repaint();
        });
    }

    // ---------- making / replacing textures ----------

    fn replace_selected(&mut self, ctx: &egui::Context, sandbox: &Path, i: usize) {
        let Origin::File(target) = self.thumbs[i].origin.clone() else {
            return;
        };
        let Some(picture) = pick_pictures("Pick the picture to use instead", false).pop() else {
            return;
        };
        match Mip::open(&target) {
            Ok(m) => {
                let mut recipe = Recipe::matching(&m);
                if self.use_picture_size {
                    recipe.size = None;
                }
                self.run_jobs(
                    ctx,
                    sandbox,
                    vec![Job {
                        picture,
                        target: target.clone(),
                        recipe,
                    }],
                    Some(target),
                );
            }
            Err(e) => {
                self.report = vec![(
                    false,
                    format!("The old picture can't be read, so its settings are unknown: {e:#}"),
                )]
            }
        }
    }

    /// Files dropped on the window or chosen with "Add pictures…".
    fn add_files(&mut self, ctx: &egui::Context, sandbox: &Path, files: Vec<PathBuf>) {
        // A dropped .dat opens it.
        if let Some(d) = files.iter().find(|p| ttk::util::ext_lower(p) == "dat") {
            let d = d.clone();
            self.open_dat(ctx, &d);
            return;
        }
        let Some(folder) = self.folder().map(Path::to_path_buf) else {
            self.report = vec![(
                false,
                if self.dat().is_some() {
                    "This is inside a .dat, which can't be changed here. Take the files out first."
                        .into()
                } else {
                    "Pick a place first, then add pictures.".into()
                },
            )];
            return;
        };
        let (pics, other): (Vec<PathBuf>, Vec<PathBuf>) =
            files.into_iter().partition(|p| build::is_picture(p));
        let report: Vec<(bool, String)> = other
            .iter()
            .map(|o| {
                (
                    false,
                    format!(
                        "{} isn't a picture I can use (PNG, JPG, BMP or TGA).",
                        ttk::util::file_name(o)
                    ),
                )
            })
            .collect();
        let track_surface = folder
            .file_name()
            .map(|n| n.to_string_lossy().eq_ignore_ascii_case("trackmat"))
            .unwrap_or(false);
        let mut jobs = Vec::new();
        let mut replacing = Vec::new();
        for p in pics {
            let target = folder.join(format!(
                "{}.mip",
                build::safe_stem(&stem_of(&p.to_string_lossy()))
            ));
            let recipe = match target.is_file().then(|| Mip::open(&target)) {
                Some(Ok(m)) => {
                    replacing.push(ttk::util::file_name(&target));
                    let mut r = Recipe::matching(&m);
                    if self.use_picture_size {
                        r.size = None;
                    }
                    r
                }
                Some(Err(_)) => {
                    replacing.push(ttk::util::file_name(&target));
                    Recipe::new_texture(track_surface, self.best_quality)
                }
                None => Recipe::new_texture(track_surface, self.best_quality),
            };
            jobs.push(Job {
                picture: p,
                target,
                recipe,
            });
        }
        if jobs.is_empty() {
            self.report = report;
            return;
        }
        self.report = report;
        if replacing.is_empty() {
            let last = jobs.last().map(|j| j.target.clone());
            self.run_jobs(ctx, sandbox, jobs, last);
        } else {
            self.pending = Some(Pending::Build { jobs, replacing });
        }
    }

    fn run_jobs(
        &mut self,
        ctx: &egui::Context,
        sandbox: &Path,
        jobs: Vec<Job>,
        select: Option<PathBuf>,
    ) {
        self.working = true;
        self.select_after = select;
        self.status = "Working…".into();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let sandbox = sandbox.to_path_buf();
        std::thread::spawn(move || {
            let work = work_dir();
            let total = jobs.len();
            let mut results = Vec::new();
            for (n, job) in jobs.into_iter().enumerate() {
                let _ = tx.send(Msg::Progress(format!(
                    "Making {} ({} of {total})…",
                    ttk::util::file_name(&job.target),
                    n + 1
                )));
                ctx.request_repaint();
                let outcome = build::build(&sandbox, &job.picture, &job.target, &job.recipe, &work)
                    .map(|b| (b.backup, b.notes))
                    .map_err(|e| format!("{e:#}"));
                results.push(JobResult { job, outcome });
            }
            let _ = std::fs::remove_dir_all(&work);
            let _ = tx.send(Msg::Built(results));
            ctx.request_repaint();
        });
    }

    fn finish_build(&mut self, ctx: &egui::Context, results: Vec<JobResult>) {
        self.working = false;
        // Keep messages about skipped files from add_files.
        let mut report = std::mem::take(&mut self.report);
        let mut undo = Vec::new();
        let (mut made, mut replaced) = (0, 0);
        for r in results {
            let name = ttk::util::file_name(&r.job.target);
            let pic = ttk::util::file_name(&r.job.picture);
            match r.outcome {
                Ok((backup, notes)) => {
                    if backup.is_some() {
                        replaced += 1;
                        report.push((true, format!("Replaced {name} with {pic}.")));
                    } else {
                        made += 1;
                        report.push((true, format!("Made {name} from {pic}.")));
                    }
                    report.extend(notes.into_iter().map(|n| (true, format!("   {n}"))));
                    undo.push(UndoItem {
                        target: r.job.target,
                        backup,
                    });
                }
                Err(e) => report.push((false, format!("Couldn't use {pic}: {e}"))),
            }
        }
        if made + replaced > 0 {
            report.push((
                true,
                "To see it in the game: Build track tab, then Run all.".into(),
            ));
        }
        self.undo = undo;
        let select = self.select_after.take();
        self.refresh(ctx, select);
        self.status = match (made, replaced) {
            (0, 0) => "Nothing was changed.".into(),
            (m, 0) => format!("Done! {m} new picture{}.", if m == 1 { "" } else { "s" }),
            (0, r) => format!(
                "Done! {r} picture{} replaced.",
                if r == 1 { "" } else { "s" }
            ),
            (m, r) => format!("Done! {m} new, {r} replaced."),
        };
        self.report = report;
    }

    fn undo_last(&mut self, ctx: &egui::Context) {
        let mut report = Vec::new();
        for u in std::mem::take(&mut self.undo) {
            let name = ttk::util::file_name(&u.target);
            let r = match &u.backup {
                Some(b) => std::fs::copy(b, &u.target).map(|_| format!("Put the old {name} back.")),
                None => std::fs::remove_file(&u.target).map(|_| format!("Removed the new {name}.")),
            };
            report.push(match r {
                Ok(s) => (true, s),
                Err(e) => (false, format!("Couldn't undo {name}: {e}")),
            });
        }
        let sel = match self
            .selected
            .and_then(|i| self.thumbs.get(i))
            .map(|t| t.origin.clone())
        {
            Some(Origin::File(p)) if p.is_file() => Some(p),
            _ => None,
        };
        self.refresh(ctx, sel);
        self.status = "Undone.".into();
        self.report = report;
    }

    // ---------- drawing ----------

    /// Draws the whole tab (call in place of the CentralPanel).
    pub fn show(&mut self, ctx: &egui::Context, env: &TexturesEnv) {
        self.poll(ctx);
        if self.source.is_none() {
            if let Some((_, p)) = env.places.iter().find(|(_, p)| p.is_dir()) {
                self.open_folder(ctx, p.clone());
            }
        }

        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() && !self.working && self.pending.is_none() {
            self.add_files(ctx, env.sandbox, dropped);
        }

        egui::SidePanel::right("tex_details")
            .resizable(true)
            .default_width(380.0)
            .min_width(300.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.details(ui, env);
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.places_bar(ui, ctx, env);
            if self.dat().is_none() {
                self.add_bar(ui, ctx, env.sandbox);
            } else {
                ui.horizontal(|ui| {
                    let b = egui::Button::new(RichText::new("Take out everything…").size(16.0).strong())
                        .min_size(Vec2::new(0.0, 32.0));
                    if ui
                        .add_enabled(!self.working, b)
                        .on_hover_text("Copies every file in this .dat into a folder, so you can change them")
                        .clicked()
                    {
                        self.take_out_all(ctx, env.unpack_root);
                    }
                    ui.label(RichText::new("Textures inside a .dat can only be looked at. Take them out to change them.").weak());
                });
            }
            ui.separator();
            self.grid(ui, ctx);
        });

        if hovering {
            let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("ttk_drop")));
            let r = ctx.screen_rect();
            painter.rect_filled(r, 0.0, Color32::from_black_alpha(170));
            let text = if self.folder().is_some() {
                format!("Drop your pictures here\nto make textures in “{}”\n\n(or drop a .dat to look inside it)", self.source_short_name())
            } else {
                "Drop a .dat to look inside it".to_string()
            };
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                text,
                FontId::proportional(30.0),
                Color32::WHITE,
            );
        }

        self.confirm_window(ctx, env.sandbox);
    }

    fn places_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, env: &TexturesEnv) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Look in:").size(16.0));
            let mut go: Option<PathBuf> = None;
            let mut go_dat: Option<PathBuf> = None;
            for (label, p) in env.places {
                let current = self.folder() == Some(p.as_path());
                let b = egui::Button::new(RichText::new(label).size(16.0))
                    .selected(current)
                    .min_size(Vec2::new(0.0, 32.0));
                if ui
                    .add_enabled(p.is_dir() && !self.working, b)
                    .on_hover_text(p.display().to_string())
                    .clicked()
                {
                    go = Some(p.clone());
                }
            }
            ui.add_enabled_ui(!self.working, |ui| {
                let shown = if let Some(d) = self.dat() {
                    stem_of(&d.path.to_string_lossy())
                } else {
                    "Look inside a track…".into()
                };
                egui::ComboBox::from_id_salt("tex_track_pick")
                    .width(200.0)
                    .selected_text(RichText::new(shown).size(16.0))
                    .show_ui(ui, |ui| {
                        for (name, dat) in env.game_dats {
                            if ui
                                .selectable_label(self.track_pick == *name, name)
                                .clicked()
                            {
                                self.track_pick = name.clone();
                                go_dat = Some(dat.clone());
                            }
                        }
                    });
                if ui
                    .add(
                        egui::Button::new(RichText::new("Open a .dat…").size(16.0))
                            .min_size(Vec2::new(0.0, 32.0)),
                    )
                    .clicked()
                {
                    go_dat = rfd::FileDialog::new()
                        .set_title("Open a .dat file")
                        .add_filter("NR2003 .dat", &["dat"])
                        .pick_file();
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new("Pick a folder…").size(16.0))
                            .min_size(Vec2::new(0.0, 32.0)),
                    )
                    .clicked()
                {
                    go = rfd::FileDialog::new()
                        .set_title("Pick a folder with textures")
                        .pick_folder();
                }
            });
            if let Some(p) = go {
                self.open_folder(ctx, p);
            }
            if let Some(d) = go_dat {
                self.open_dat(ctx, &d);
            }
        });
        ui.horizontal(|ui| {
            let full = match &self.source {
                Some(Source::Dat(d)) => d.path.display().to_string(),
                _ => self.source_label(),
            };
            // A folder with track-surface textures one level down: offer a way in.
            let trackmat = self
                .folder()
                .map(|f| f.join("trackmat"))
                .filter(|t| t.is_dir());
            if let Some(tm) = trackmat {
                if ui
                    .button(RichText::new("Track surfaces in here").strong())
                    .on_hover_text(tm.display().to_string())
                    .clicked()
                {
                    self.open_folder(ctx, tm);
                }
            }
            let w = (ui.available_width() - 200.0).max(100.0);
            ui.allocate_ui(Vec2::new(w, 20.0), |ui| {
                ui.add(egui::Label::new(RichText::new(self.source_label()).weak()).truncate())
                    .on_hover_text(full);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("find by name")
                        .desired_width(160.0),
                );
                if self.loading || self.working {
                    ui.spinner();
                }
            });
        });
        ui.separator();
    }

    fn add_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, sandbox: &Path) {
        ui.horizontal(|ui| {
            let add = egui::Button::new(RichText::new("Add pictures…").size(16.0).strong())
                .min_size(Vec2::new(0.0, 32.0));
            if ui
                .add_enabled(!self.working && self.folder().is_some(), add)
                .on_hover_text("Turn your own pictures into textures in this folder")
                .clicked()
            {
                let pics = pick_pictures("Pick pictures to turn into textures", true);
                if !pics.is_empty() {
                    self.add_files(ctx, sandbox, pics);
                }
            }
            ui.label(RichText::new("or drag pictures onto this window").weak());
            ui.separator();
            ui.label("New textures:");
            ui.selectable_value(&mut self.best_quality, false, "Normal")
                .on_hover_text("Compressed like the game's own textures. Smaller and faster.");
            ui.selectable_value(&mut self.best_quality, true, "Best quality")
                .on_hover_text("Not compressed. Sharper colors, bigger files.");
        });
    }

    fn grid(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let filter = self.filter.to_ascii_lowercase();
        let mut clicked: Option<usize> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                    for (i, t) in self.thumbs.iter().enumerate() {
                        if !filter.is_empty() && !t.full_name.to_ascii_lowercase().contains(&filter)
                        {
                            continue;
                        }
                        let (rect, resp) = ui.allocate_exact_size(TILE, Sense::click());
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let painter = ui.painter_at(rect);
                        let bg = if self.selected == Some(i) {
                            ui.visuals().selection.bg_fill
                        } else if resp.hovered() {
                            ui.visuals().widgets.hovered.bg_fill
                        } else {
                            ui.visuals().faint_bg_color
                        };
                        painter.rect_filled(rect, 6.0, bg);
                        let img_area = Rect::from_min_size(
                            rect.min + Vec2::new(8.0, 8.0),
                            Vec2::splat(THUMB as f32),
                        );
                        match (&t.tex, &t.error) {
                            (Some(tex), _) => {
                                let s = fit(tex.size_vec2(), img_area.size());
                                let r = Rect::from_center_size(img_area.center(), s);
                                paint_checker(&painter, r);
                                egui::Image::new(tex).paint_at(ui, r);
                            }
                            (None, Some(_)) => {
                                painter.text(
                                    img_area.center(),
                                    Align2::CENTER_CENTER,
                                    "can't read",
                                    FontId::proportional(14.0),
                                    BAD,
                                );
                            }
                            (None, None) => {
                                painter.text(
                                    img_area.center(),
                                    Align2::CENTER_CENTER,
                                    "…",
                                    FontId::proportional(24.0),
                                    Color32::GRAY,
                                );
                            }
                        }
                        let label = match t.name.rsplit_once('.') {
                            Some((s, "stp")) => format!("{s} (menu)"),
                            Some((s, _)) => s.to_string(),
                            None => t.name.clone(),
                        };
                        painter.text(
                            egui::pos2(rect.center().x, img_area.bottom() + 6.0),
                            Align2::CENTER_TOP,
                            label,
                            FontId::proportional(13.0),
                            ui.visuals().text_color(),
                        );
                        let tip = match (&t.info, &t.error) {
                            (Some(info), _) => format!(
                                "{}\n{}×{} · {}",
                                t.full_name,
                                info.width,
                                info.height,
                                mip::type_name(info.kind)
                            ),
                            (None, Some(e)) => format!("{}\n{e}", t.full_name),
                            _ => t.full_name.clone(),
                        };
                        if resp.on_hover_text(tip).clicked() {
                            clicked = Some(i);
                        }
                    }
                });
            });
        if let Some(i) = clicked {
            self.select(ctx, i);
        }
    }

    fn details(&mut self, ui: &mut egui::Ui, env: &TexturesEnv) {
        ui.add_space(6.0);
        let Some(i) = self.selected else {
            ui.heading("Click a picture");
            ui.label(if self.dat().is_some() {
                "Pick any picture on the left to see it big and save it."
            } else {
                "Pick any picture on the left to see it big, save it, or swap in your own."
            });
            ui.add_space(16.0);
            self.save_all_button(ui);
            self.results(ui);
            return;
        };
        let t = &self.thumbs[i];
        ui.heading(stem_of(&t.full_name));
        if t.full_name != t.name {
            ui.label(RichText::new(&t.full_name).weak());
        }

        if let Some((_, tex)) = &self.big {
            let s = fit(tex.size_vec2(), Vec2::new(ui.available_width(), 300.0));
            let (r, _) = ui.allocate_exact_size(s, Sense::hover());
            paint_checker(ui.painter(), r);
            egui::Image::new(tex).paint_at(ui, r);
        } else if let Some(e) = &self.big_error {
            ui.colored_label(BAD, format!("This picture can't be read:\n{e}"));
        }
        ui.add_space(8.0);

        if let Some(info) = &self.thumbs[i].info {
            let info = info.clone();
            egui::Grid::new("tex_facts")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Size").strong());
                    ui.label(format!("{} × {} pixels", info.width, info.height));
                    ui.end_row();
                    ui.label(RichText::new("Kind").strong());
                    ui.label(if info.stp {
                        format!("Menu picture · {}", mip::type_name(info.kind))
                    } else {
                        mip::type_name(info.kind).to_string()
                    });
                    ui.end_row();
                    ui.label(RichText::new("See-through").strong());
                    ui.label(self.big_alpha.describe());
                    ui.end_row();
                    if !info.stp {
                        ui.label(RichText::new("On the track").strong());
                        ui.label(mip::mapping_name(info.mapping));
                        ui.end_row();
                        ui.label(RichText::new("Detail priority").strong());
                        ui.label(friendly_priority(info.priority));
                        ui.end_row();
                    }
                });
        }
        ui.add_space(12.0);

        if self.dat().is_some() {
            if ui
                .add(big_button(ui, "Save as picture", 18.0, 44.0))
                .on_hover_text("Saves a PNG you can open in any paint program")
                .clicked()
            {
                self.save_one(i);
            }
            ui.add_space(4.0);
            if ui
                .add(big_button(ui, "Take out this file…", 16.0, 36.0))
                .on_hover_text("Copies the original file out of the .dat")
                .clicked()
            {
                self.take_out_one(i);
            }
            self.results(ui);
            ui.add_space(10.0);
            self.save_all_button(ui);
            let ctx = ui.ctx().clone();
            if ui
                .add_enabled(
                    !self.working,
                    big_button(ui, "Take out everything…", 16.0, 36.0),
                )
                .clicked()
            {
                self.take_out_all(&ctx, env.unpack_root);
            }
            return;
        }

        let can_build = !self.working && self.big.is_some();
        if ui
            .add_enabled(
                can_build,
                big_button(ui, "Replace with my picture…", 18.0, 44.0),
            )
            .on_hover_text("Pick a picture. It takes this one's place, keeping all its settings.")
            .clicked()
        {
            let ctx = ui.ctx().clone();
            self.replace_selected(&ctx, env.sandbox, i);
        }
        ui.checkbox(
            &mut self.use_picture_size,
            "Use my picture's size (sharper if it's bigger)",
        );
        self.results(ui);
        ui.add_space(10.0);
        if ui
            .add(big_button(ui, "Save as picture", 16.0, 36.0))
            .on_hover_text("Saves a PNG you can open in any paint program")
            .clicked()
        {
            self.save_one(i);
        }
        ui.add_space(4.0);
        self.save_all_button(ui);
    }

    fn save_all_button(&mut self, ui: &mut egui::Ui) {
        let enabled = !self.thumbs.is_empty() && !self.working;
        if ui
            .add_enabled(enabled, big_button(ui, "Save all as pictures", 16.0, 36.0))
            .on_hover_text("Saves every picture here as a PNG")
            .clicked()
        {
            let ctx = ui.ctx().clone();
            self.save_all(&ctx);
        }
    }

    fn results(&mut self, ui: &mut egui::Ui) {
        if !self.status.is_empty() {
            ui.add_space(10.0);
            ui.label(RichText::new(&self.status).size(15.0).strong());
        }
        for (ok, line) in &self.report {
            ui.label(RichText::new(line).color(if *ok { GOOD } else { BAD }));
        }
        if !self.undo.is_empty() && !self.working {
            ui.add_space(6.0);
            if ui
                .add(big_button(ui, "Undo", 16.0, 34.0))
                .on_hover_text("Put things back the way they were")
                .clicked()
            {
                let ctx = ui.ctx().clone();
                self.undo_last(&ctx);
            }
        }
    }

    fn confirm_window(&mut self, ctx: &egui::Context, sandbox: &Path) {
        let Some(p) = &self.pending else { return };
        // 1 = do it, 2 = only new ones / skip existing, 0 = cancel
        let mut choice: Option<u8> = None;
        let (title, lines, yes, partial): (&str, Vec<String>, &str, &str) = match p {
            Pending::Build { replacing, .. } => (
                "Replace pictures?",
                {
                    let mut v = vec!["These already exist:".to_string()];
                    v.extend(replacing.iter().take(12).map(|n| format!("   • {n}")));
                    if replacing.len() > 12 {
                        v.push(format!("   … and {} more", replacing.len() - 12));
                    }
                    v.push("Replacing keeps their settings, and the old ones are saved so you can undo.".into());
                    v
                },
                "Replace them",
                "Only add the new ones",
            ),
            Pending::Extract { existing, dir, .. } => (
                "Files already there",
                vec![
                    format!(
                        "{existing} of these files are already in {}.",
                        dir.display()
                    ),
                    "If you changed any of them, replacing would lose your changes.".into(),
                ],
                "Replace them",
                "Keep mine, add the rest",
            ),
        };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                for (n, l) in lines.iter().enumerate() {
                    if n == 0 {
                        ui.label(RichText::new(l).size(16.0));
                    } else {
                        ui.label(l);
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(RichText::new(yes).size(16.0).strong()))
                        .clicked()
                    {
                        choice = Some(1);
                    }
                    if ui
                        .add(egui::Button::new(RichText::new(partial).size(16.0)))
                        .clicked()
                    {
                        choice = Some(2);
                    }
                    if ui
                        .add(egui::Button::new(RichText::new("Cancel").size(16.0)))
                        .clicked()
                    {
                        choice = Some(0);
                    }
                });
            });
        let Some(c) = choice else { return };
        let p = self.pending.take().unwrap();
        if c == 0 {
            return;
        }
        match p {
            Pending::Build { jobs, .. } => {
                let jobs: Vec<Job> = if c == 1 {
                    jobs
                } else {
                    jobs.into_iter().filter(|j| !j.target.is_file()).collect()
                };
                if jobs.is_empty() {
                    self.status = "Nothing to add.".into();
                } else {
                    let last = jobs.last().map(|j| j.target.clone());
                    self.run_jobs(ctx, sandbox, jobs, last);
                }
            }
            Pending::Extract { dat, dir, .. } => {
                if c == 1 {
                    self.run_extract(ctx, dat, dir);
                } else {
                    // Keep existing files: extract only the missing ones.
                    let keep: Vec<_> = dat
                        .entries
                        .iter()
                        .filter(|e| {
                            e.relative_path()
                                .map(|r| !dir.join(r).exists())
                                .unwrap_or(false)
                        })
                        .cloned()
                        .collect();
                    self.run_extract_some(ctx, dat, dir, keep);
                }
            }
        }
    }

    fn run_extract_some(
        &mut self,
        ctx: &egui::Context,
        d: Arc<Dat>,
        dir: PathBuf,
        entries: Vec<ttk::dat::DatEntry>,
    ) {
        self.working = true;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let (mut written, mut failed) = (0, Vec::new());
            for e in &entries {
                match d.extract(e, &dir) {
                    Ok(_) => written += 1,
                    Err(x) => failed.push(format!("{}: {x:#}", e.name)),
                }
            }
            let _ = tx.send(Msg::Extracted {
                dir,
                written,
                failed,
            });
            ctx.request_repaint();
        });
    }
}
