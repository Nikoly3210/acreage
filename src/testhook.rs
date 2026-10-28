//! Test hook: with `CANOPY_STATE_FILE=/path.json`, the app writes what the user can see
//! (screen, zoom, selection, filter, dialogs, toasts) whenever it changes. The e2e suite
//! reads it to assert behaviour instead of comparing pixels. Off unless the variable is set.

use std::fmt::Write;
use std::path::PathBuf;

pub struct Hook {
    path: Option<PathBuf>,
    last: String,
}

impl Hook {
    pub fn from_env() -> Hook {
        Hook {
            path: std::env::var_os("CANOPY_STATE_FILE").map(PathBuf::from),
            last: String::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.path.is_some()
    }

    pub fn write(&mut self, state: String) {
        let Some(path) = &self.path else { return };
        if state == self.last {
            return;
        }
        // Write-then-rename so a reader never sees half a file.
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, &state).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
        self.last = state;
    }
}

/// Minimal JSON object builder (no serde dependency for a test hook).
#[derive(Default)]
pub struct Obj(String);

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

impl Obj {
    fn sep(&mut self) {
        if !self.0.is_empty() {
            self.0.push(',');
        }
    }
    pub fn str(mut self, k: &str, v: &str) -> Self {
        self.sep();
        let _ = write!(self.0, "{}:{}", esc(k), esc(v));
        self
    }
    pub fn opt(mut self, k: &str, v: Option<&str>) -> Self {
        self.sep();
        match v {
            Some(v) => {
                let _ = write!(self.0, "{}:{}", esc(k), esc(v));
            }
            None => {
                let _ = write!(self.0, "{}:null", esc(k));
            }
        }
        self
    }
    pub fn num(mut self, k: &str, v: u64) -> Self {
        self.sep();
        let _ = write!(self.0, "{}:{v}", esc(k));
        self
    }
    pub fn bool(mut self, k: &str, v: bool) -> Self {
        self.sep();
        let _ = write!(self.0, "{}:{v}", esc(k));
        self
    }
    pub fn list(mut self, k: &str, v: &[String]) -> Self {
        self.sep();
        let items: Vec<String> = v.iter().map(|s| esc(s)).collect();
        let _ = write!(self.0, "{}:[{}]", esc(k), items.join(","));
        self
    }
    pub fn finish(self) -> String {
        format!("{{{}}}", self.0)
    }
}
