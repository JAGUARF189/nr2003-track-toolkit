//! The .ptf object graph, as produced by the C++ `ptfjson` tool (cpp/tools/ptfjson.cpp).
//! Every object is a class name plus its fields in file order; see docs/ptf/classes.md.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::dat::Dat;

#[derive(Deserialize)]
struct FileJson {
    root: i32,
    #[serde(default)]
    trailer: String,
    objects: Vec<Node>,
}

#[derive(Deserialize, Clone)]
pub struct Node {
    pub id: i32,
    pub class: String,
    /// (name, kind, value) in file order
    pub fields: Vec<(String, String, Value)>,
}

impl Node {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.iter().find(|f| f.0 == name).map(|f| &f.2)
    }
    /// Number field (ints too). Non-finite floats, stored as "bits:..", read as NaN.
    pub fn f(&self, name: &str) -> f64 {
        match self.get(name) {
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
            Some(Value::String(s)) if s.starts_with("bits:") => f64::NAN,
            _ => 0.0,
        }
    }
    pub fn i(&self, name: &str) -> i64 {
        self.get(name).and_then(Value::as_i64).unwrap_or(0)
    }
    /// Object reference (None for null).
    pub fn r(&self, name: &str) -> Option<i32> {
        self.get(name).and_then(Value::as_i64).map(|v| v as i32).filter(|&v| v != 0)
    }
    pub fn s(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(Value::as_str)
    }
}

pub struct Graph {
    pub root: i32,
    pub trailer: String,
    pub nodes: HashMap<i32, Node>,
}

impl Graph {
    pub fn from_json(text: &str) -> Result<Self> {
        let f: FileJson = serde_json::from_str(text).context("reading ptfjson output")?;
        let nodes: HashMap<i32, Node> = f.objects.into_iter().map(|n| (n.id, n)).collect();
        if !nodes.contains_key(&f.root) {
            bail!("the track file has no root object");
        }
        Ok(Self { root: f.root, trailer: f.trailer, nodes })
    }

    pub fn node(&self, id: i32) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn root(&self) -> &Node {
        &self.nodes[&self.root]
    }

    /// Follow a reference field.
    pub fn child(&self, n: &Node, field: &str) -> Option<&Node> {
        n.r(field).and_then(|id| self.node(id))
    }

    /// Objects of a list field `stem[0]`, `stem[1]`, ...
    pub fn list<'a>(&'a self, n: &'a Node, stem: &str, count: i64) -> Vec<&'a Node> {
        (0..count.max(0)).filter_map(|k| self.child(n, &format!("{stem}[{k}]"))).collect()
    }
}

/// Where ptfjson.exe lives: next to this program, or the C++ build folder when running from
/// a source checkout (target\release\ -> cpp\build\). TTK_PTFJSON overrides both.
pub fn find_ptfjson() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TTK_PTFJSON").map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut candidates = vec![dir.join("ptfjson.exe")];
    for up in dir.ancestors().take(8) {
        candidates.push(up.join("cpp").join("build").join("ptfjson.exe"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Decode .ptf bytes into a graph using ptfjson.
pub fn parse_ptf(bytes: &[u8]) -> Result<Graph> {
    let tool = find_ptfjson().context(
        "ptfjson.exe was not found. It must sit next to track-toolkit.exe (build it from the cpp folder).",
    )?;
    let tmp = std::env::temp_dir().join(format!(
        "ttk_view_{}_{}.ptf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    let mut cmd = Command::new(&tool);
    cmd.arg("read").arg(&tmp).arg("-").stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output();
    let _ = std::fs::remove_file(&tmp);
    let out = out.with_context(|| format!("running {}", tool.display()))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        bail!("couldn't read the track file: {}", msg.trim().trim_start_matches("error: "));
    }
    let text = String::from_utf8(out.stdout).context("ptfjson output is not text")?;
    Graph::from_json(&text)
}

/// The .ptf bytes for a track: a loose `.ptf` file, or the `.ptf` inside a `.dat`.
/// Returns the bytes and a description of where they came from.
pub fn load_ptf_bytes(path: &Path) -> Result<(Vec<u8>, String)> {
    let ext = crate::util::ext_lower(path);
    if ext == "ptf" {
        let b = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        return Ok((b, path.display().to_string()));
    }
    if ext == "dat" {
        let dat = Dat::open(path)?;
        let entry = dat
            .entries
            .iter()
            .find(|e| e.ext_lower() == "ptf")
            .with_context(|| format!("{} has no .ptf track file inside", path.display()))?;
        let b = dat.read(entry)?;
        return Ok((b, format!("{} (inside {})", entry.file_name(), path.display())));
    }
    bail!("pick a .ptf or .dat file")
}

/// Best file to show for a game track folder: `<name>.ptf` if it's there, else `<name>.dat`.
pub fn track_file_in(folder: &Path) -> Option<PathBuf> {
    let name = folder.file_name()?.to_string_lossy().to_string();
    let ptf = folder.join(format!("{name}.ptf"));
    if ptf.is_file() {
        return Some(ptf);
    }
    let dat = folder.join(format!("{name}.dat"));
    if dat.is_file() {
        return Some(dat);
    }
    // any .ptf, then any .dat
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder).ok()?.flatten().map(|e| e.path()).collect();
    files.sort();
    files
        .iter()
        .find(|p| crate::util::ext_lower(p) == "ptf")
        .or_else(|| files.iter().find(|p| crate::util::ext_lower(p) == "dat"))
        .cloned()
}
