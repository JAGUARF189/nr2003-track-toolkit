//! Byte-preserving editor for NR2003 `track.ini` files.
//!
//! track.ini files are Windows-1252 (e.g. "banked 36°") with `;` comments and
//! section headers written as `[ name ]`. Lines we don't touch are written back
//! byte-for-byte.

pub struct IniDoc {
    lines: Vec<Vec<u8>>,
    crlf: bool,
    trailing_eol: bool,
}

fn text(line: &[u8]) -> String {
    String::from_utf8_lossy(line).into_owned()
}

/// Lowercase section name if `line` is a `[ section ]` header.
pub fn section_of_line(line: &[u8]) -> Option<String> {
    let t = text(line);
    let t = t.trim();
    if !t.starts_with('[') {
        return None;
    }
    let end = t.find(']')?;
    Some(t[1..end].trim().to_ascii_lowercase())
}

/// Lowercase key if `line` is a `key = value` line.
pub fn key_of_line(line: &[u8]) -> Option<String> {
    let t = text(line);
    let t = t.trim();
    if t.is_empty() || t.starts_with(';') || t.starts_with('[') {
        return None;
    }
    let i = t.find('=')?;
    Some(t[..i].trim().to_ascii_lowercase())
}

fn is_stall_key(k: &str) -> bool {
    k.strip_prefix("stall_")
        .map(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or(false)
}

impl IniDoc {
    pub fn parse(bytes: &[u8]) -> Self {
        let crlf = bytes.windows(2).any(|w| w == b"\r\n");
        let trailing_eol = bytes.ends_with(b"\n");
        let mut lines: Vec<Vec<u8>> = if bytes.is_empty() {
            Vec::new()
        } else {
            bytes
                .split(|&b| b == b'\n')
                .map(|l| l.strip_suffix(b"\r").unwrap_or(l).to_vec())
                .collect()
        };
        if trailing_eol {
            lines.pop();
        }
        Self {
            lines,
            crlf,
            trailing_eol,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let eol: &[u8] = if self.crlf { b"\r\n" } else { b"\n" };
        let mut out = Vec::new();
        for (i, l) in self.lines.iter().enumerate() {
            if i > 0 {
                out.extend_from_slice(eol);
            }
            out.extend_from_slice(l);
        }
        if self.trailing_eol && !self.lines.is_empty() {
            out.extend_from_slice(eol);
        }
        out
    }

    /// (header index, end index exclusive) of a section.
    pub fn section_range(&self, name: &str) -> Option<(usize, usize)> {
        let name = name.trim().to_ascii_lowercase();
        let start = self
            .lines
            .iter()
            .position(|l| section_of_line(l).as_deref() == Some(name.as_str()))?;
        let end = (start + 1..self.lines.len())
            .find(|&i| section_of_line(&self.lines[i]).is_some())
            .unwrap_or(self.lines.len());
        Some((start, end))
    }

    /// Value of `key` in `section`, without any trailing `;` comment.
    pub fn get(&self, section: &str, key: &str) -> Option<String> {
        let (s, e) = self.section_range(section)?;
        let key = key.to_ascii_lowercase();
        (s + 1..e).find_map(|i| {
            if key_of_line(&self.lines[i]).as_deref() == Some(key.as_str()) {
                let t = text(&self.lines[i]);
                let v = &t[t.find('=').unwrap() + 1..];
                let v = v.split(';').next().unwrap_or("");
                Some(v.trim().to_string())
            } else {
                None
            }
        })
    }

    /// Set `key = value` in `section`, adding the key or section if needed.
    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        let line = format!("{key} = {value}").into_bytes();
        let lkey = key.to_ascii_lowercase();
        match self.section_range(section) {
            Some((s, e)) => {
                for i in s + 1..e {
                    if key_of_line(&self.lines[i]).as_deref() == Some(lkey.as_str()) {
                        self.lines[i] = line;
                        return;
                    }
                }
                self.lines.insert(s + 1, line);
            }
            None => {
                if !self.lines.is_empty() {
                    self.lines.push(Vec::new());
                }
                self.lines.push(format!("[ {section} ]").into_bytes());
                self.lines.push(line);
            }
        }
    }

    /// Section names (lowercase), in file order.
    pub fn section_names(&self) -> Vec<String> {
        self.lines.iter().filter_map(|l| section_of_line(l)).collect()
    }

    /// How many `stall_N = ...` lines `section` has.
    pub fn stall_count(&self, section: &str) -> usize {
        self.section_range(section).map_or(0, |(s, e)| {
            (s + 1..e).filter(|&i| key_of_line(&self.lines[i]).is_some_and(|k| is_stall_key(&k))).count()
        })
    }

    /// Replace every `stall_N = ...` line in `section` with `new_lines`.
    /// `stall_pace` and all other keys are kept. Returns how many lines were removed.
    pub fn replace_stalls(&mut self, section: &str, new_lines: &[String]) -> usize {
        let Some((s, e)) = self.section_range(section) else {
            if !self.lines.is_empty() {
                self.lines.push(Vec::new());
            }
            self.lines.push(format!("[ {section} ]").into_bytes());
            self.lines
                .extend(new_lines.iter().map(|l| l.clone().into_bytes()));
            return 0;
        };
        let idxs: Vec<usize> = (s + 1..e)
            .filter(|&i| {
                key_of_line(&self.lines[i])
                    .map(|k| is_stall_key(&k))
                    .unwrap_or(false)
            })
            .collect();
        let insert_at = match idxs.first() {
            Some(&first) => first,
            None => {
                let pace = (s + 1..e)
                    .rev()
                    .find(|&i| key_of_line(&self.lines[i]).as_deref() == Some("stall_pace"));
                match pace {
                    Some(i) => i + 1,
                    None => {
                        let mut i = e;
                        while i > s + 1 && text(&self.lines[i - 1]).trim().is_empty() {
                            i -= 1;
                        }
                        i
                    }
                }
            }
        };
        for &i in idxs.iter().rev() {
            self.lines.remove(i);
        }
        for (n, l) in new_lines.iter().enumerate() {
            self.lines.insert(insert_at + n, l.clone().into_bytes());
        }
        idxs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = b"[ track ]\r\ntrack_name = OLD ; c\r\ninfo = banked 36\xb0\r\n\r\n[ pit_lane_0 ]\r\nfoo = 1\r\nstall_pace = 675.00 35.15 0.00\r\nstall_0 = 1 2 3\r\nstall_1 = 4 5 6\r\n; tail\r\n[ next ]\r\nx = 1\r\n";

    #[test]
    fn untouched_roundtrip_is_identical() {
        assert_eq!(IniDoc::parse(SAMPLE).to_bytes(), SAMPLE);
    }

    #[test]
    fn set_and_get() {
        let mut d = IniDoc::parse(SAMPLE);
        assert_eq!(d.get("TRACK", "track_name").as_deref(), Some("OLD"));
        d.set("track", "track_name", "NEW");
        d.set("track", "track_name_short", "new");
        assert_eq!(d.get("track", "track_name").as_deref(), Some("NEW"));
        assert_eq!(d.get("track", "track_name_short").as_deref(), Some("new"));
        // non-UTF8 byte preserved
        assert!(d.to_bytes().windows(2).any(|w| w == b"6\xb0"));
    }

    #[test]
    fn stalls_replaced_in_place() {
        let mut d = IniDoc::parse(SAMPLE);
        let n = d.replace_stalls(
            "pit_lane_0",
            &[
                "stall_0 = 9 9 9".into(),
                "stall_1 = 8 8 8".into(),
                "stall_2 = 7 7 7".into(),
            ],
        );
        assert_eq!(n, 2);
        let s = String::from_utf8_lossy(&d.to_bytes()).into_owned();
        assert!(s.contains("stall_pace = 675.00 35.15 0.00\r\nstall_0 = 9 9 9\r\nstall_1 = 8 8 8\r\nstall_2 = 7 7 7\r\n; tail"));
        assert!(!s.contains("stall_0 = 1 2 3"));
    }

    #[test]
    fn stalls_into_new_section() {
        let mut d = IniDoc::parse(SAMPLE);
        d.replace_stalls("starting_grid_0", &["stall_0 = 1 1 1".into()]);
        assert!(d.section_range("starting_grid_0").is_some());
        assert_eq!(
            d.get("starting_grid_0", "stall_0").as_deref(),
            Some("1 1 1")
        );
    }
}
