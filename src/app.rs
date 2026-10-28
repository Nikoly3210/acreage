//! The app shell: which screen is showing, and switching between them.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Theme, ViewportCommand};

use crate::scan::ScanOpts;
use crate::scan::mounts::MountTable;
use crate::screens::browse::{Browse, BrowseOut};
use crate::screens::picker::Picker;
use crate::ui::style;
use crate::view::SizeMode;

enum Screen {
    Picker(Picker),
    Browse(Box<Browse>),
}

pub struct App {
    theme: Theme,
    mounts: Arc<MountTable>,
    opts: ScanOpts,
    mode: SizeMode,
    initial_filter: Option<String>,
    screen: Screen,
}

impl App {
    pub fn new(
        ctx: &egui::Context,
        theme: Theme,
        path: Option<PathBuf>,
        opts: ScanOpts,
        mode: SizeMode,
        filter: Option<String>,
    ) -> App {
        let mounts = Arc::new(MountTable::read());
        let mut app = App {
            theme,
            mounts: mounts.clone(),
            opts,
            mode,
            initial_filter: filter,
            screen: Screen::Picker(Picker::new(&mounts)),
        };
        if let Some(p) = path {
            app.open(ctx, p);
        }
        app
    }

    fn open(&mut self, ctx: &egui::Context, path: PathBuf) {
        ctx.send_viewport_cmd(ViewportCommand::Title(format!(
            "canopy — {}",
            path.display()
        )));
        let b = Browse::new(
            ctx,
            path,
            self.opts,
            self.mode,
            self.mounts.clone(),
            self.initial_filter.take(),
        );
        self.screen = Screen::Browse(Box::new(b));
    }

    fn picker(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(ViewportCommand::Title("canopy".into()));
        self.mounts = Arc::new(MountTable::read());
        self.screen = Screen::Picker(Picker::new(&self.mounts));
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Ctrl+Shift+T flips the theme (no settings screen yet).
        if ui.input_mut(|i| {
            i.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::T,
            )
        }) {
            self.theme = if self.theme == Theme::Dark {
                Theme::Light
            } else {
                Theme::Dark
            };
            ctx.set_theme(self.theme);
        }
        let p = style::palette(self.theme);
        match &mut self.screen {
            Screen::Picker(picker) => {
                if let Some(path) = picker.ui(ui, p) {
                    self.open(&ctx, path);
                }
            }
            Screen::Browse(b) => {
                if let BrowseOut::OpenPicker = b.ui(ui, p) {
                    self.picker(&ctx);
                }
            }
        }
    }
}
