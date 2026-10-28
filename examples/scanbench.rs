//! Headless scan timing: `cargo run --release --example scanbench -- /usr`
//! Compare with `du -sx /usr`.

use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use acreage::model::Tree;
use acreage::scan::mounts::MountTable;
use acreage::scan::{ScanEvent, ScanOpts, Scanner};

fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "/usr".into()))
        .canonicalize()
        .unwrap();
    let t0 = Instant::now();
    let s = Scanner::start(
        root.clone(),
        ScanOpts::default(),
        Arc::new(MountTable::read()),
        Arc::new(|| {}),
    );
    let mut tree = Tree::new(root.as_os_str().as_bytes());
    let mut errors = 0;
    loop {
        match s
            .events
            .recv_timeout(Duration::from_secs(60))
            .expect("stalled")
        {
            ScanEvent::Batch { parent, entries } => {
                tree.insert_batch(parent, entries);
            }
            ScanEvent::DirDone { id } => tree.set_scanned(id),
            ScanEvent::DirError { id, .. } => {
                tree.set_denied(id);
                errors += 1;
            }
            ScanEvent::Done { .. } => break,
        }
    }
    let n = tree.get(0);
    println!(
        "{}: {} nodes, {} files, {:.1} MiB on disk, {} errors, {:.2}s",
        root.display(),
        tree.len(),
        n.items,
        n.size_disk as f64 / 1048576.0,
        errors,
        t0.elapsed().as_secs_f64()
    );
}
