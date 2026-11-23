//! Runs the 1999-era Papyrus command-line tools and streams their output.

use crate::logger::Logger;
use anyhow::{bail, Context, Result};
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;

pub struct ToolOutput {
    pub code: i32,
    pub lines: Vec<String>,
}

impl ToolOutput {
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    /// Lines that mention an error (the tools don't always set an exit code).
    pub fn error_lines(&self) -> Vec<&str> {
        self.lines
            .iter()
            .map(|s| s.as_str())
            .filter(|l| l.to_ascii_lowercase().contains("error"))
            .collect()
    }

    pub fn last_line(&self) -> &str {
        self.lines.last().map(|s| s.trim()).unwrap_or("(no output)")
    }
}

/// Run `exe args...` in `cwd`. Output lines are logged as they arrive.
///
/// The tools are given short relative paths wherever possible: they predate
/// long file names with spaces and some of them mis-parse their arguments.
pub fn run_tool<S: AsRef<OsStr>>(
    log: &Logger,
    exe: &Path,
    args: &[S],
    cwd: &Path,
) -> Result<ToolOutput> {
    if !exe.is_file() {
        bail!(
            "Papyrus tool not found: {} (check the Sandbox folder in Settings)",
            exe.display()
        );
    }
    let shown: Vec<String> = args
        .iter()
        .map(|a| {
            let s = a.as_ref().to_string_lossy();
            if s.contains(' ') {
                format!("\"{s}\"")
            } else {
                s.into_owned()
            }
        })
        .collect();
    log.tool(format!(
        "> {} {}    [in {}]",
        crate::util::file_name(exe),
        shown.join(" "),
        cwd.display()
    ));

    let mut cmd = Command::new(exe);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("could not start {}", exe.display()))?;

    let (tx, rx) = mpsc::channel::<String>();
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(spawn_reader(out, tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(spawn_reader(err, tx.clone()));
    }
    drop(tx);

    let mut lines = Vec::new();
    for line in rx {
        log.tool(format!("  {line}"));
        lines.push(line);
    }
    for r in readers {
        let _ = r.join();
    }
    let status = child.wait()?;
    Ok(ToolOutput {
        code: status.code().unwrap_or(-1),
        lines,
    })
}

fn spawn_reader<R: Read + Send + 'static>(r: R, tx: mpsc::Sender<String>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut rd = BufReader::new(r);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match rd.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let s = String::from_utf8_lossy(&buf);
                    // Progress output sometimes uses bare '\r'.
                    for part in s.split(['\r', '\n']) {
                        let p = part.trim_end();
                        if !p.is_empty() && tx.send(p.to_string()).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    })
}
