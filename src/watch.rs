//! Live updates: one thread owns an inotify instance, collects events per directory,
//! waits until a directory has been quiet briefly, re-lists it and sends the listing to
//! the UI, which diffs it into the tree.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};

use crate::model::Entry;
use crate::scan::mounts::MountTable;
use crate::scan::{ScanOpts, list_dir};

/// Wait this long after the last event in a directory before re-listing it…
const QUIET: Duration = Duration::from_millis(250);
/// …but never longer than this, so a constantly busy directory still updates.
const MAX_DELAY: Duration = Duration::from_millis(1500);

enum Cmd {
    /// Watch these folders. With `relist`, list each once right away too: anything created
    /// between its scan and the watch being added would otherwise never be seen.
    Watch(Vec<PathBuf>, bool),
}

pub struct Listing {
    pub dir: PathBuf,
    pub entries: io::Result<Vec<Entry>>,
}

pub struct Watcher {
    tx: Sender<Cmd>,
    pub listings: Receiver<Listing>,
    watched: Arc<AtomicUsize>,
    limited: Arc<AtomicBool>,
    pub budget: usize,
}

/// Half the system's inotify watch limit, at most 65 536: leave room for other programs.
pub fn watch_budget() -> usize {
    let max = std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(8192);
    (max / 2).clamp(1, 65_536)
}

impl Watcher {
    pub fn start(
        mounts: Arc<MountTable>,
        opts: ScanOpts,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<Watcher> {
        let mut ino = Inotify::init()?;
        let (tx, cmd_rx) = unbounded::<Cmd>();
        let (list_tx, listings) = unbounded();
        let watched = Arc::new(AtomicUsize::new(0));
        let limited = Arc::new(AtomicBool::new(false));
        let budget = watch_budget();
        let (w2, l2) = (watched.clone(), limited.clone());
        thread::Builder::new().name("watch".into()).spawn(move || {
            let mask = WatchMask::CREATE
                | WatchMask::DELETE
                | WatchMask::MOVED_FROM
                | WatchMask::MOVED_TO
                | WatchMask::CLOSE_WRITE
                | WatchMask::MODIFY
                | WatchMask::ATTRIB
                | WatchMask::ONLYDIR
                | WatchMask::DONT_FOLLOW
                | WatchMask::EXCL_UNLINK;
            let mut dirs: HashMap<WatchDescriptor, PathBuf> = HashMap::new();
            // path -> (first event, last event)
            let mut dirty: HashMap<PathBuf, (Instant, Instant)> = HashMap::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                loop {
                    match cmd_rx.try_recv() {
                        Ok(Cmd::Watch(paths, relist)) => {
                            for p in paths {
                                if dirs.len() >= budget {
                                    l2.store(true, Ordering::Relaxed);
                                    break;
                                }
                                match ino.watches().add(&p, mask) {
                                    Ok(wd) => {
                                        if relist {
                                            let t = Instant::now() - QUIET;
                                            dirty.insert(p.clone(), (t, t));
                                        }
                                        dirs.insert(wd, p);
                                    }
                                    // Out of watches system-wide (ENOSPC): stop and report it.
                                    Err(e) if e.raw_os_error() == Some(28) => {
                                        l2.store(true, Ordering::Relaxed);
                                        break;
                                    }
                                    Err(_) => {}
                                }
                            }
                            w2.store(dirs.len(), Ordering::Relaxed);
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                match ino.read_events(&mut buf) {
                    Ok(events) => {
                        let now = Instant::now();
                        for ev in events {
                            if ev.mask.contains(EventMask::IGNORED) {
                                dirs.remove(&ev.wd);
                                continue;
                            }
                            if let Some(p) = dirs.get(&ev.wd) {
                                dirty
                                    .entry(p.clone())
                                    .and_modify(|e| e.1 = now)
                                    .or_insert((now, now));
                            }
                        }
                        w2.store(dirs.len(), Ordering::Relaxed);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(40))
                    }
                    Err(_) => thread::sleep(Duration::from_millis(200)),
                }
                let now = Instant::now();
                let ready: Vec<PathBuf> = dirty
                    .iter()
                    .filter(|(_, (first, last))| now - *last >= QUIET || now - *first >= MAX_DELAY)
                    .map(|(p, _)| p.clone())
                    .collect();
                for dir in ready {
                    dirty.remove(&dir);
                    let entries = list_dir(&dir, &mounts, opts);
                    if list_tx.send(Listing { dir, entries }).is_err() {
                        return;
                    }
                    wake();
                }
            }
        })?;
        Ok(Watcher {
            tx,
            listings,
            watched,
            limited,
            budget,
        })
    }

    pub fn watch(&self, paths: Vec<PathBuf>) {
        if !paths.is_empty() {
            self.tx.send(Cmd::Watch(paths, false)).ok();
        }
    }

    /// Watch folders that just appeared, and re-list them once to close the scan→watch gap.
    pub fn watch_new(&self, paths: Vec<PathBuf>) {
        if !paths.is_empty() {
            self.tx.send(Cmd::Watch(paths, true)).ok();
        }
    }

    pub fn watched(&self) -> usize {
        self.watched.load(Ordering::Relaxed)
    }

    /// True once a watch could not be added (budget or system limit reached).
    pub fn limited(&self) -> bool {
        self.limited.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_changes_in_a_watched_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let w = Watcher::start(
            Arc::new(MountTable::read()),
            ScanOpts::default(),
            Arc::new(|| {}),
        )
        .unwrap();
        w.watch(vec![tmp.path().to_path_buf()]);
        thread::sleep(Duration::from_millis(150));
        std::fs::write(tmp.path().join("new.txt"), b"hello").unwrap();
        let l = w
            .listings
            .recv_timeout(Duration::from_secs(3))
            .expect("no listing");
        assert_eq!(l.dir, tmp.path());
        let names: Vec<Vec<u8>> = l.entries.unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec![b"new.txt".to_vec()]);
        assert_eq!(w.watched(), 1);
    }
}
