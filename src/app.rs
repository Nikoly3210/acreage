//! The app shell: which screen is showing, switching between them, and the shared settings.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Theme, ViewportCommand};

use crate::config::Config;
use crate::scan::mounts::MountTable;
use crate::screens::browse::{Browse, BrowseOut};
use crate::screens::picker::Picker;
use crate::testhook::{Hook, Obj};
use crate::ui::style;

enum Screen {
    Picker(Picker),
    Browse(Box<Browse>),
}

pub struct App {
    cfg: Config,
    /// Why the settings file couldn't be read, shown once on the first browse screen.
    cfg_error: Option<String>,
    mounts: Arc<MountTable>,
    initial_filter: Option<String>,
    screen: Screen,
    hook: Hook,
}

fn theme_of(cfg: &Config) -> Theme {
    if cfg.dark() {
        Theme::Dark
    } else {
        Theme::Light
    }
}

impl App {
    pub fn new(
        ctx: &egui::Context,
        cfg: Config,
        cfg_error: Option<String>,
        path: Option<PathBuf>,
        filter: Option<String>,
    ) -> App {
        let mounts = Arc::new(MountTable::read());
        let mut app = App {
            cfg,
            cfg_error,
            mounts: mounts.clone(),
            initial_filter: filter,
            screen: Screen::Picker(Picker::new(&mounts)),
            hook: Hook::from_env(),
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
            self.cfg.clone(),
            self.mounts.clone(),
            self.initial_filter.take(),
            self.cfg_error.take(),
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
        // Ctrl+Shift+T flips the theme (and remembers it).
        if ui.input_mut(|i| {
            i.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::T,
            )
        }) {
            self.cfg.theme = if self.cfg.dark() {
                "light".into()
            } else {
                "dark".into()
            };
            if let Screen::Browse(b) = &mut self.screen {
                b.cfg.theme = self.cfg.theme.clone();
            }
            let _ = self.cfg.save();
        }
        let theme = theme_of(&self.cfg);
        if ctx.theme() != theme {
            ctx.set_theme(theme);
        }
        let p = style::palette(theme);
        match &mut self.screen {
            Screen::Picker(picker) => {
                if let Some(path) = picker.ui(ui, p) {
                    self.open(&ctx, path);
                }
            }
            Screen::Browse(b) => {
                let out = b.ui(ui, p);
                // Settings changed on the browse screen are the app's settings now.
                if b.cfg != self.cfg {
                    self.cfg = b.cfg.clone();
                }
                if let BrowseOut::OpenPicker = out {
                    self.picker(&ctx);
                }
            }
        }
        if self.hook.enabled() {
            let state = match &self.screen {
                Screen::Picker(picker) => Obj::default()
                    .str("screen", "picker")
                    .opt("error", picker.error.as_deref())
                    .finish(),
                Screen::Browse(b) => b
                    .test_state(Obj::default().str("screen", "browse"))
                    .finish(),
            };
            self.hook.write(state);
        }
    }
}
