//! Parallel directory scanner. Workers never touch the tree: they send `ScanEvent`s that the
//! UI thread splices in, so rendering never sees a half-updated tree and needs no locks.

pub mod mounts;

use std::ffi::{CStr, OsStr};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, StatxFlags};

use crate::model::{Entry, Kind, NodeId};
use mounts::{FsClass, MountTable};

pub use crate::model::Entry as RawEntry;

/// Max entries per batch, so a 1M-entry directory still streams in.
const BATCH: usize = 4096;

#[derive(Clone, Copy, Debug)]
pub struct ScanOpts {
    /// Don't descend into other mounted filesystems.
    pub one_fs: bool,
}

impl Default for ScanOpts {
    fn default() -> Self {
        ScanOpts { one_fs: true }
    }
}

#[derive(Debug)]
pub enum ScanEvent {
    Batch {
        parent: NodeId,
        entries: Vec<Entry>,
    },
    /// All entries of this directory were sent.
    DirDone {
        id: NodeId,
    },
    DirError {
        id: NodeId,
        err: io::ErrorKind,
    },
    Done {
        cancelled: bool,
        elapsed: Duration,
    },
}

struct Ctl {
    paused: AtomicBool,
    cancel: AtomicBool,
    finished: AtomicBool,
    ids: Arc<AtomicU32>,
    pending: AtomicUsize,
    /// Test hook (ACREAGE_TEST_PAUSE_AFTER): pause after this many folders, so e2e tests can
    /// act while a scan is genuinely in progress. usize::MAX = never.
    pause_after: AtomicUsize,
    done: AtomicUsize,
}

struct Work {
    id: NodeId,
    path: PathBuf,
}

pub struct Scanner {
    pub events: Receiver<ScanEvent>,
    ctl: Arc<Ctl>,
    ev_tx: Sender<ScanEvent>,
    started: Instant,
}

/// Number of workers: fewer on spinning disks, where parallel seeks hurt.
fn worker_count(root: &Path) -> usize {
    let cpus = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8);
    if is_rotational(root) { 2 } else { cpus }
}

fn is_rotational(path: &Path) -> bool {
    let Ok(st) = rustix::fs::stat(path) else {
        return false;
    };
    let (maj, min) = (rustix::fs::major(st.st_dev), rustix::fs::minor(st.st_dev));
    // Partitions point at their parent disk through ../queue.
    for p in [
        format!("/sys/dev/block/{maj}:{min}/queue/rotational"),
        format!("/sys/dev/block/{maj}:{min}/../queue/rotational"),
    ] {
        if let Ok(s) = std::fs::read_to_string(p) {
            return s.trim() == "1";
        }
    }
    false
}

impl Scanner {
    /// Scan `root` (should be canonical). Node 0 is the root; the scanner hands out ids from 1.
    /// `wake` is called when new events are available (e.g. `ctx.request_repaint`).
    pub fn start(
        root: PathBuf,
        opts: ScanOpts,
        mounts: Arc<MountTable>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Scanner {
        Self::start_at(root, 0, Arc::new(AtomicU32::new(1)), opts, mounts, wake)
    }

    /// Scan `root` as existing node `root_id`. `ids` is shared with every other scan of
    /// the same tree, so a sub-scan can run next to the main one without id clashes.
    pub fn start_at(
        root: PathBuf,
        root_id: NodeId,
        ids: Arc<AtomicU32>,
        opts: ScanOpts,
        mounts: Arc<MountTable>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Scanner {
        let ctl = Arc::new(Ctl {
            paused: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            ids,
            pending: AtomicUsize::new(1),
            pause_after: AtomicUsize::new(if root_id == 0 {
                std::env::var("ACREAGE_TEST_PAUSE_AFTER")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(usize::MAX)
            } else {
                usize::MAX
            }),
            done: AtomicUsize::new(0),
        });
        let (ev_tx, events) = unbounded();
        let (work_tx, work_rx) = unbounded::<Work>();
        let started = Instant::now();
        let n = worker_count(&root);
        work_tx
            .send(Work {
                id: root_id,
                path: root,
            })
            .ok();

        for i in 0..n {
            let (ctl, ev_tx, work_tx, work_rx, mounts, wake) = (
                ctl.clone(),
                ev_tx.clone(),
                work_tx.clone(),
                work_rx.clone(),
                mounts.clone(),
                wake.clone(),
            );
            thread::Builder::new()
                .name(format!("scan-{i}"))
                .spawn(move || {
                    worker(
                        &ctl, &ev_tx, &work_tx, &work_rx, &mounts, opts, &*wake, started,
                    )
                })
                .expect("spawn scanner thread");
        }
        Scanner {
            events,
            ctl,
            ev_tx,
            started,
        }
    }

    pub fn set_paused(&self, paused: bool) {
        self.ctl.paused.store(paused, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.ctl.paused.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        if !self.ctl.cancel.swap(true, Ordering::Relaxed)
            && !self.ctl.finished.load(Ordering::Relaxed)
        {
            self.ev_tx
                .send(ScanEvent::Done {
                    cancelled: true,
                    elapsed: self.started.elapsed(),
                })
                .ok();
        }
    }

    /// The id allocator of this tree, for sub-scans and live updates.
    pub fn ids(&self) -> Arc<AtomicU32> {
        self.ctl.ids.clone()
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        self.ctl.cancel.store(true, Ordering::Relaxed);
    }
}

#[allow(clippy::too_many_arguments)]
fn worker(
    ctl: &Ctl,
    ev_tx: &Sender<ScanEvent>,
    work_tx: &Sender<Work>,
    work_rx: &Receiver<Work>,
    mounts: &MountTable,
    opts: ScanOpts,
    wake: &(dyn Fn() + Send + Sync),
    started: Instant,
) {
    loop {
        if ctl.cancel.load(Ordering::Relaxed) || ctl.finished.load(Ordering::Relaxed) {
            return;
        }
        if ctl.paused.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(50));
            continue;
        }
        match work_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(w) => {
                scan_dir(ctl, ev_tx, work_tx, mounts, opts, w);
                if ctl.done.fetch_add(1, Ordering::Relaxed) + 1
                    == ctl.pause_after.load(Ordering::Relaxed)
                {
                    ctl.paused.store(true, Ordering::Relaxed);
                }
                wake();
                if ctl.pending.fetch_sub(1, Ordering::AcqRel) == 1
                    && !ctl.cancel.load(Ordering::Relaxed)
                {
                    ctl.finished.store(true, Ordering::Relaxed);
                    ev_tx
                        .send(ScanEvent::Done {
                            cancelled: false,
                            elapsed: started.elapsed(),
                        })
                        .ok();
                    wake();
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn scan_dir(
    ctl: &Ctl,
    ev_tx: &Sender<ScanEvent>,
    work_tx: &Sender<Work>,
    mounts: &MountTable,
    opts: ScanOpts,
    w: Work,
) {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let fd = match rustix::fs::open(&w.path, flags, Mode::empty()) {
        Ok(fd) => fd,
        Err(e) => {
            ev_tx
                .send(ScanEvent::DirError {
                    id: w.id,
                    err: io::Error::from(e).kind(),
                })
                .ok();
            return;
        }
    };
    let dir = match Dir::read_from(&fd) {
        Ok(d) => d,
        Err(e) => {
            ev_tx
                .send(ScanEvent::DirError {
                    id: w.id,
                    err: io::Error::from(e).kind(),
                })
                .ok();
            return;
        }
    };

    let mut batch = Vec::new();
    let mut subdirs = Vec::new();
    for ent in dir {
        if ctl.cancel.load(Ordering::Relaxed) {
            return;
        }
        // A read error mid-directory (e.g. EIO): keep what we have.
        let Ok(ent) = ent else { break };
        let name = ent.file_name();
        let bytes = name.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        let Some((mut entry, sub)) = stat_entry(&fd, name, &w.path, mounts, opts) else {
            continue;
        };
        entry.id = ctl.ids.fetch_add(1, Ordering::Relaxed);
        if let Some(path) = sub {
            subdirs.push(Work { id: entry.id, path });
        }
        batch.push(entry);
        if batch.len() >= BATCH {
            ev_tx
                .send(ScanEvent::Batch {
                    parent: w.id,
                    entries: std::mem::take(&mut batch),
                })
                .ok();
        }
    }
    if !batch.is_empty() {
        ev_tx
            .send(ScanEvent::Batch {
                parent: w.id,
                entries: batch,
            })
            .ok();
    }
    ev_tx.send(ScanEvent::DirDone { id: w.id }).ok();
    // Enqueue children only after their entries were sent, so their own batches arrive later.
    ctl.pending.fetch_add(subdirs.len(), Ordering::AcqRel);
    for s in subdirs {
        work_tx.send(s).ok();
    }
}

/// stat one directory entry relative to its directory fd. Returns the entry (id 0) and, for
/// folders the scan should descend into, the folder's path. `None` if it vanished.
fn stat_entry(
    fd: &OwnedFd,
    name: &CStr,
    dir: &Path,
    mounts: &MountTable,
    opts: ScanOpts,
) -> Option<(Entry, Option<PathBuf>)> {
    let bytes = name.to_bytes();
    let st =
        rustix::fs::statx(fd, name, AtFlags::SYMLINK_NOFOLLOW, StatxFlags::BASIC_STATS).ok()?;
    let mut kind = match FileType::from_raw_mode(st.stx_mode as _) {
        FileType::Directory => Kind::Dir,
        FileType::RegularFile => Kind::File,
        FileType::Symlink => Kind::Symlink,
        _ => Kind::Other,
    };
    let mut sub = None;
    if kind == Kind::Dir {
        let child = dir.join(OsStr::from_bytes(bytes));
        if mounts.is_boundary(&child) {
            let class = mounts.class_at(&child).unwrap_or(FsClass::Local);
            // proc, sysfs, tmpfs, cgroup…: nothing on disk to measure, so not even a block.
            if class == FsClass::Pseudo {
                return None;
            }
            if opts.one_fs || class != FsClass::Local {
                kind = Kind::MountPoint;
            }
        }
        if kind == Kind::Dir {
            sub = Some(child);
        }
    }
    let mount = kind == Kind::MountPoint;
    Some((
        Entry {
            id: 0,
            name: bytes.to_vec(),
            kind,
            size_apparent: if mount { 0 } else { st.stx_size },
            size_disk: if mount { 0 } else { st.stx_blocks * 512 },
            mtime: st.stx_mtime.tv_sec,
            nlink: st.stx_nlink,
            dev: rustix::fs::makedev(st.stx_dev_major, st.stx_dev_minor),
            ino: st.stx_ino,
        },
        sub,
    ))
}

/// One non-recursive listing of `dir` (ids left at 0), for live updates.
pub fn list_dir(dir: &Path, mounts: &MountTable, opts: ScanOpts) -> io::Result<Vec<Entry>> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let fd = rustix::fs::open(dir, flags, Mode::empty())?;
    let mut out = vec![];
    for ent in Dir::read_from(&fd)? {
        let Ok(ent) = ent else { break };
        let name = ent.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        if let Some((e, _)) = stat_entry(&fd, name, dir, mounts, opts) {
            out.push(e);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Tree;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn run(root: &Path) -> (Tree, Vec<io::ErrorKind>) {
        let mounts = Arc::new(MountTable::read());
        let s = Scanner::start(
            root.to_path_buf(),
            ScanOpts::default(),
            mounts,
            Arc::new(|| {}),
        );
        let mut tree = Tree::new(root.as_os_str().as_bytes());
        let mut errors = vec![];
        loop {
            match s
                .events
                .recv_timeout(Duration::from_secs(10))
                .expect("scan stalled")
            {
                ScanEvent::Batch { parent, entries } => {
                    tree.insert_batch(parent, entries);
                }
                ScanEvent::DirDone { id } => tree.set_scanned(id),
                ScanEvent::DirError { id, err } => {
                    tree.set_denied(id);
                    errors.push(err);
                }
                ScanEvent::Done { cancelled, .. } => {
                    assert!(!cancelled);
                    break;
                }
            }
        }
        (tree, errors)
    }

    fn find(t: &Tree, parent: u32, name: &str) -> u32 {
        t.children(parent)
            .find(|c| t.name(*c) == name.as_bytes())
            .unwrap_or_else(|| panic!("{name} missing"))
    }

    #[test]
    fn scans_a_fixture_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        std::fs::create_dir_all(r.join("a/b/c")).unwrap();
        std::fs::write(r.join("a/one.bin"), vec![1u8; 10_000]).unwrap();
        std::fs::write(r.join("a/b/c/deep.txt"), b"hello").unwrap();
        std::fs::hard_link(r.join("a/one.bin"), r.join("a/b/link.bin")).unwrap();
        symlink(r, r.join("a/loop")).unwrap(); // must not be followed
        std::fs::write(r.join(OsStr::from_bytes(b"bad-\xff-name")), b"x").unwrap();
        // Sparse file: 50 MB apparent, almost nothing on disk.
        let f = std::fs::File::create(r.join("sparse.img")).unwrap();
        f.set_len(50 << 20).unwrap();
        std::fs::create_dir(r.join("locked")).unwrap();
        std::fs::write(r.join("locked/secret"), b"s").unwrap();
        std::fs::set_permissions(r.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();

        let (t, errors) = run(r);
        std::fs::set_permissions(r.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();

        let a = find(&t, 0, "a");
        assert_eq!(t.get(find(&t, a, "loop")).kind, Kind::Symlink);
        assert!(
            t.is_hardlink_dup(find(&t, a, "one.bin"))
                || t.is_hardlink_dup(find(&t, find(&t, a, "b"), "link.bin"))
        );
        let sparse = t.get(find(&t, 0, "sparse.img"));
        assert_eq!(sparse.size_apparent, 50 << 20);
        assert!(sparse.size_disk < 1 << 20);
        assert!(t.children(0).any(|c| t.name(c) == b"bad-\xff-name"));
        // Running as root can read 000 dirs; only assert denial for normal users.
        if !rustix::process::geteuid().is_root() {
            assert_eq!(t.get(find(&t, 0, "locked")).kind, Kind::Denied);
            assert_eq!(errors, vec![io::ErrorKind::PermissionDenied]);
        }
        // Every directory finished.
        assert!(t.is_scanned(find(&t, find(&t, a, "b"), "c")));
        // Root items: one.bin, link.bin (dup still a file), deep.txt, loop, bad name, sparse.img.
        assert_eq!(t.get(0).items, 6);
    }
}
