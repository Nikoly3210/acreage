//! Things canopy does to the outside world: open with the default app, open a terminal,
//! move to Trash. Long-running work happens on a thread and reports back on a channel.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use crossbeam_channel::Sender;

use crate::model::NodeId;

#[derive(Debug)]
pub enum ActionResult {
    Trashed {
        ok: Vec<NodeId>,
        failed: Vec<(PathBuf, String)>,
    },
}

/// Spawn detached and reap it on a thread so no zombies pile up.
fn spawn(mut cmd: Command) -> io::Result<()> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn open(path: &Path) -> io::Result<()> {
    let mut c = Command::new("xdg-open");
    c.arg(path);
    spawn(c)
}

pub fn open_containing(path: &Path) -> io::Result<()> {
    open(path.parent().unwrap_or(path))
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

/// `$TERMINAL`, else the first common terminal found on `$PATH`.
pub fn terminal_command() -> Option<String> {
    if let Ok(t) = std::env::var("TERMINAL")
        && !t.trim().is_empty()
    {
        return Some(t);
    }
    [
        "alacritty",
        "kitty",
        "foot",
        "wezterm",
        "ghostty",
        "konsole",
        "gnome-terminal",
        "xfce4-terminal",
        "urxvt",
        "xterm",
    ]
    .into_iter()
    .find(|b| on_path(b))
    .map(str::to_owned)
}

/// Open a terminal in `dir` (or the folder containing it, for files).
pub fn terminal_here(path: &Path) -> io::Result<()> {
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let term = terminal_command().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "no terminal found; set $TERMINAL")
    })?;
    let mut parts = term.split_whitespace();
    let mut c = Command::new(parts.next().unwrap_or("xterm"));
    c.args(parts).current_dir(dir);
    spawn(c)
}

/// Move items to the freedesktop Trash on a thread. Already-gone paths count as success.
pub fn trash(
    items: Vec<(NodeId, PathBuf)>,
    tx: Sender<ActionResult>,
    wake: impl Fn() + Send + 'static,
) {
    thread::spawn(move || {
        let mut ok = vec![];
        let mut failed = vec![];
        for (id, path) in items {
            if std::fs::symlink_metadata(&path).is_err() {
                ok.push(id);
                continue;
            }
            match trash::delete(&path) {
                Ok(()) => ok.push(id),
                Err(e) => failed.push((path, e.to_string())),
            }
        }
        tx.send(ActionResult::Trashed { ok, failed }).ok();
        wake();
    });
}
