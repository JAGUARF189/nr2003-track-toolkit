//! The .ptf object graph, as produced by the C++ `ptfjson` tool (cpp/tools/ptfjson.cpp).
//! Every object is a class name plus its fields in file order; see docs/ptf/classes.md.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
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

#[derive(Serialize, Deserialize, Clone, Debug)]
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
    /// Change a field's value; returns the old value. Fails if the field doesn't exist.
    pub fn set(&mut self, name: &str, v: Value) -> Result<Value> {
        let class = self.class.clone();
        let f = self
            .fields
            .iter_mut()
            .find(|f| f.0 == name)
            .with_context(|| format!("{class} has no field {name}"))?;
        // Store exactly what the file can hold, so the editor and the saved file agree.
        let v = match (f.1.as_str(), v.as_f64()) {
            ("f32", Some(x)) => serde_json::json!(x as f32 as f64),
            ("i32" | "u8" | "size", Some(x)) if v.as_i64().is_none() => serde_json::json!(x.round() as i64),
            _ => v,
        };
        Ok(std::mem::replace(&mut f.2, v))
    }
}

/// Same value, comparing numbers by value (so 2 == 2.0) and everything else exactly.
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64()) {
            (Some(i), Some(j)) => i == j,
            _ => x.as_f64().map(f64::to_bits) == y.as_f64().map(f64::to_bits),
        },
        _ => a == b,
    }
}

#[derive(Clone)]
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

    pub fn node_mut(&mut self, id: i32) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    /// The ptfjson text form (objects in id order), which `ptfjson write` turns into a .ptf.
    pub fn to_json(&self) -> Result<String> {
        let mut ids: Vec<i32> = self.nodes.keys().copied().collect();
        ids.sort_unstable();
        #[derive(Serialize)]
        struct Out<'a> {
            format: &'static str,
            version: u32,
            root: i32,
            trailer: &'a str,
            objects: Vec<&'a Node>,
        }
        let out = Out {
            format: "ptfjson",
            version: 1,
            root: self.root,
            trailer: &self.trailer,
            objects: ids.iter().map(|id| &self.nodes[id]).collect(),
        };
        Ok(serde_json::to_string(&out)?)
    }

    /// Explain the first difference from `other`, or None if both hold the same track.
    pub fn difference(&self, other: &Graph) -> Option<String> {
        if self.root != other.root || self.trailer != other.trailer {
            return Some("the file header differs".into());
        }
        if self.nodes.len() != other.nodes.len() {
            return Some(format!("{} objects instead of {}", other.nodes.len(), self.nodes.len()));
        }
        for (id, a) in &self.nodes {
            let Some(b) = other.nodes.get(id) else { return Some(format!("object {id} is missing")) };
            if a.class != b.class || a.fields.len() != b.fields.len() {
                return Some(format!("object {id} ({}) differs", a.class));
            }
            for (fa, fb) in a.fields.iter().zip(&b.fields) {
                let same = if fa.1 == "f32" {
                    // stored as a 32-bit float: compare at that precision
                    match (fa.2.as_f64(), fb.2.as_f64()) {
                        (Some(x), Some(y)) => (x as f32).to_bits() == (y as f32).to_bits(),
                        _ => fa.2 == fb.2,
                    }
                } else {
                    same_value(&fa.2, &fb.2)
                };
                if fa.0 != fb.0 || fa.1 != fb.1 || !same {
                    return Some(format!(
                        "object {id} ({}) field {}: {} in the editor, {} in the file",
                        a.class, fa.0, fa.2, fb.2
                    ));
                }
            }
        }
        None
    }

    /// An id no object uses yet.
    pub fn next_id(&self) -> i32 {
        self.nodes.keys().copied().max().unwrap_or(0) + 1
    }

    /// The same track numbered the way the writer numbers it: objects in the order they are
    /// written (depth first, following object fields in order from the root), one counter
    /// shared with the class ids, and objects nothing refers to left out. A file read back
    /// from disk is already in this form, so after objects are added or removed this is what
    /// it should be compared with.
    pub fn canonical(&self) -> Graph {
        struct Walk<'a> {
            g: &'a Graph,
            map: HashMap<i32, i32>,
            order: Vec<i32>,
            classes: std::collections::HashSet<&'a str>,
            counter: i32,
        }
        impl<'a> Walk<'a> {
            fn visit(&mut self, id: i32) {
                if id == 0 || self.map.contains_key(&id) {
                    return;
                }
                let Some(n) = self.g.nodes.get(&id) else { return };
                self.map.insert(id, self.counter);
                self.counter += 1;
                if self.classes.insert(n.class.as_str()) {
                    self.counter += 1;
                }
                self.order.push(id);
                for f in &n.fields {
                    if f.1 == "obj" {
                        if let Some(c) = f.2.as_i64() {
                            self.visit(c as i32);
                        }
                    }
                }
            }
        }
        let mut w = Walk { g: self, map: HashMap::new(), order: Vec::new(), classes: Default::default(), counter: 1 };
        w.visit(self.root);
        let mut nodes = HashMap::with_capacity(w.order.len());
        for id in &w.order {
            let mut n = self.nodes[id].clone();
            n.id = w.map[id];
            for f in &mut n.fields {
                if f.1 == "obj" {
                    if let Some(c) = f.2.as_i64().filter(|&c| c != 0) {
                        f.2 = Value::from(w.map.get(&(c as i32)).copied().unwrap_or(0));
                    }
                }
            }
            nodes.insert(n.id, n);
        }
        Graph { root: w.map.get(&self.root).copied().unwrap_or(1), trailer: self.trailer.clone(), nodes }
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
    let out = run_ptfjson(&tool, &["read".as_ref(), tmp.as_os_str(), "-".as_ref()]);
    let _ = std::fs::remove_file(&tmp);
    let out = out.map_err(|e| anyhow::anyhow!("couldn't read the track file: {e}"))?;
    let text = String::from_utf8(out).context("ptfjson output is not text")?;
    Graph::from_json(&text)
}

/// Read a .ptf file into a graph.
pub fn read_ptf_file(path: &Path) -> Result<Graph> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    parse_ptf(&bytes)
}

/// Write a graph to `dest` as a .ptf (via ptfjson). Doesn't check the result; see save.rs.
pub fn write_ptf(g: &Graph, dest: &Path) -> Result<()> {
    write_ptf_json(&g.to_json()?, dest)
}

/// Write ptfjson text to `dest` as a .ptf.
pub fn write_ptf_json(json: &str, dest: &Path) -> Result<()> {
    let tool = find_ptfjson().context("ptfjson.exe was not found next to track-toolkit.exe")?;
    let json_path = dest.with_extension("ptfjson.tmp");
    std::fs::write(&json_path, json).with_context(|| format!("writing {}", json_path.display()))?;
    let r = run_ptfjson(&tool, &["write".as_ref(), json_path.as_os_str(), dest.as_os_str()]);
    let _ = std::fs::remove_file(&json_path);
    r.map(|_| ()).map_err(|e| anyhow::anyhow!("couldn't write the track: {e}"))
}

/// Run ptfjson without a console window; Ok(stdout) or Err(its error message).
fn run_ptfjson(tool: &Path, args: &[&std::ffi::OsStr]) -> std::result::Result<Vec<u8>, String> {
    let mut cmd = Command::new(tool);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().map_err(|e| format!("running {}: {e}", tool.display()))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        return Err(msg.trim().trim_start_matches("error: ").to_string());
    }
    Ok(out.stdout)
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
