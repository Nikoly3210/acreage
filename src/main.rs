use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;

use canopy::app::App;
use canopy::config::Config;
use canopy::model::Tree;
use canopy::report::{self, Format, Header, ReportOpts};
use canopy::scan::mounts::MountTable;
use canopy::scan::{ScanEvent, ScanOpts, Scanner};
use canopy::screens::picker::resolve;
use canopy::ui::{fonts, style, theme};
use canopy::view::{SizeMode, filtered_sizes, node_size};

/// See where your disk space went: a live, zoomable treemap.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Folder to scan. Without it, canopy shows the disk picker.
    path: Option<PathBuf>,
    /// Also descend into other filesystems mounted below PATH (default: stay on one).
    #[arg(long)]
    cross_fs: bool,
    /// Measure file sizes in bytes instead of disk usage.
    #[arg(long)]
    apparent_size: bool,
    /// Start with this filter, e.g. '*.iso;>1gb'.
    #[arg(short, long)]
    filter: Option<String>,
    /// Colour theme (default: the saved setting).
    #[arg(long, value_parser = ["dark", "light"])]
    theme: Option<String>,
    /// Don't open a window: scan PATH, write a report to FILE ('-' for stdout) and exit.
    #[arg(long, value_name = "FILE")]
    report: Option<String>,
    /// Report: levels below PATH to list.
    #[arg(long, default_value_t = 3, requires = "report")]
    depth: usize,
    /// Report: list files too, not just folder totals.
    #[arg(long, requires = "report")]
    files: bool,
    /// Report: at most this many entries per folder.
    #[arg(long, default_value_t = 50, requires = "report")]
    top: usize,
    /// Report format.
    #[arg(long, value_parser = ["text", "csv", "json"], default_value = "text", requires = "report")]
    format: String,
}

fn fail(msg: String) -> ! {
    eprintln!("canopy: {msg}");
    std::process::exit(2);
}

/// `--report`: the same scanner and report as the GUI, no window.
fn headless(root: PathBuf, cli: &Cli, cfg: &Config) -> ! {
    let filter = match cli.filter.as_deref().map(canopy::filter::parse).transpose() {
        Ok(f) => f.flatten(),
        Err(e) => fail(format!("bad filter: {e}")),
    };
    let s = Scanner::start(
        root.clone(),
        ScanOpts {
            one_fs: cfg.one_filesystem,
        },
        Arc::new(MountTable::read().for_scan(&root)),
        Arc::new(|| {}),
    );
    let mut tree = Tree::new(root.as_os_str().as_bytes());
    let mut errors = 0;
    loop {
        match s.events.recv_timeout(Duration::from_secs(600)) {
            Ok(ScanEvent::Batch { parent, entries }) => {
                tree.insert_batch(parent, entries);
            }
            Ok(ScanEvent::DirDone { id }) => tree.set_scanned(id),
            Ok(ScanEvent::DirError { id, .. }) => {
                tree.set_denied(id);
                errors += 1;
            }
            Ok(ScanEvent::Done { .. }) => break,
            Err(_) => fail("scan stalled for 10 minutes".into()),
        }
    }
    let mode = if cfg.apparent() {
        SizeMode::Apparent
    } else {
        SizeMode::Disk
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let sizes = (filter.is_some() || cfg.hide_dotfiles).then(|| {
        filtered_sizes(
            &tree,
            0,
            filter.as_ref(),
            cfg.hide_dotfiles,
            &Default::default(),
            mode,
            now,
        )
    });
    let size = |id: u32| match &sizes {
        Some(s) => s[id as usize],
        None => node_size(&tree, id, mode),
    };
    let opts = ReportOpts {
        depth: cli.depth.max(1),
        files: cli.files,
        top: cli.top.max(1),
        format: Format::parse(&cli.format).unwrap_or(Format::Text),
    };
    let header = Header {
        when: report::utc_stamp(now),
        size_label: if mode == SizeMode::Apparent {
            "apparent size"
        } else {
            "disk usage"
        },
        filter: cli.filter.as_deref(),
    };
    let text = report::render(&tree, 0, &size, &opts, &header);
    let target = cli.report.as_deref().unwrap_or("-");
    let res = if target == "-" {
        std::io::stdout().write_all(text.as_bytes())
    } else {
        std::fs::write(target, &text)
    };
    if let Err(e) = res {
        fail(format!("couldn't write {target}: {e}"));
    }
    if errors > 0 {
        eprintln!("canopy: {errors} folders couldn't be read; their size isn't counted");
    }
    std::process::exit(0);
}

fn main() -> eframe::Result {
    let cli = Cli::parse();
    let path = cli
        .path
        .as_ref()
        .map(|p| resolve(&p.to_string_lossy()).unwrap_or_else(|e| fail(e)));

    // Saved settings, with command-line flags on top (flags are not saved).
    let (mut cfg, cfg_error) = Config::load();
    if let Some(t) = &cli.theme {
        cfg.theme = t.clone();
    }
    if cli.apparent_size {
        cfg.size = "apparent".into();
    }
    if cli.cross_fs {
        cfg.one_filesystem = false;
    }

    if cli.report.is_some() {
        let Some(root) = path else {
            fail("--report needs a PATH to scan".into())
        };
        headless(root, &cli, &cfg);
    }

    let theme = if cfg.dark() {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("canopy")
            .with_app_id("canopy")
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size(theme::layout::MIN_WINDOW),
        ..Default::default()
    };
    eframe::run_native(
        "canopy",
        native,
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            style::apply(&cc.egui_ctx, theme);
            Ok(Box::new(App::new(
                &cc.egui_ctx,
                cfg,
                cfg_error,
                path,
                cli.filter,
            )))
        }),
    )
}
