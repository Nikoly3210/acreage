use std::path::PathBuf;

use clap::Parser;
use egui::Theme;

use canopy::app::App;
use canopy::scan::ScanOpts;
use canopy::screens::picker::resolve;
use canopy::ui::{fonts, style, theme};
use canopy::view::SizeMode;

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
    /// Colour theme.
    #[arg(long, value_parser = ["dark", "light"], default_value = "dark")]
    theme: String,
}

fn main() -> eframe::Result {
    let cli = Cli::parse();
    let path = match &cli.path {
        Some(p) => match resolve(&p.to_string_lossy()) {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("canopy: {e}");
                std::process::exit(2);
            }
        },
        None => None,
    };
    let theme = if cli.theme == "light" {
        Theme::Light
    } else {
        Theme::Dark
    };
    let opts = ScanOpts {
        one_fs: !cli.cross_fs,
    };
    let mode = if cli.apparent_size {
        SizeMode::Apparent
    } else {
        SizeMode::Disk
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
                theme,
                path,
                opts,
                mode,
                cli.filter,
            )))
        }),
    )
}
