//! Settings remembered between runs: `$XDG_CONFIG_HOME/canopy/config.toml`
//! (default `~/.config/canopy/config.toml`). Human-editable; unknown keys are ignored and
//! missing ones take defaults, so an old file never stops canopy from starting.

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub filter: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Detail {
    /// Fewer, bigger blocks.
    Coarse,
    Balanced,
    /// Every file that fits.
    Fine,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// "dark" or "light".
    pub theme: String,
    /// "disk" (like du) or "apparent" (file length).
    pub size: String,
    pub one_filesystem: bool,
    pub hide_dotfiles: bool,
    pub detail: Detail,
    pub animations: bool,
    pub live_updates: bool,
    pub presets: Vec<Preset>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "dark".into(),
            size: "disk".into(),
            one_filesystem: true,
            hide_dotfiles: false,
            detail: Detail::Balanced,
            animations: true,
            live_updates: true,
            presets: vec![
                Preset {
                    name: "Big files".into(),
                    filter: ">1gb".into(),
                },
                Preset {
                    name: "Old downloads".into(),
                    filter: ">6months".into(),
                },
                Preset {
                    name: "Videos".into(),
                    filter: "*.mkv;*.mp4;*.webm;*.mov;*.avi".into(),
                },
            ],
        }
    }
}

pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("canopy").join("config.toml")
}

impl Config {
    /// Load the config. A missing file gives defaults; a broken one gives defaults plus the
    /// error text so the UI can say so (the broken file is left alone until the next save).
    pub fn load() -> (Config, Option<String>) {
        let p = path();
        match std::fs::read_to_string(&p) {
            Ok(text) => match toml::from_str(&text) {
                Ok(c) => (c, None),
                Err(e) => (
                    Config::default(),
                    Some(format!("{}: {}", p.display(), e.message())),
                ),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Config::default(), None),
            Err(e) => (Config::default(), Some(format!("{}: {e}", p.display()))),
        }
    }

    /// Write atomically (temp file + rename) so a crash can't leave half a config.
    pub fn save(&self) -> io::Result<()> {
        let p = path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(
            &tmp,
            format!(
                "# canopy settings. Edit freely; canopy rewrites this file when you change settings.\n\n{text}"
            ),
        )?;
        std::fs::rename(tmp, p)
    }

    pub fn apparent(&self) -> bool {
        self.size == "apparent"
    }

    pub fn dark(&self) -> bool {
        self.theme != "light"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_partial_files() {
        let mut c = Config {
            theme: "light".into(),
            detail: Detail::Fine,
            ..Default::default()
        };
        c.presets.push(Preset {
            name: "x".into(),
            filter: "*.iso;>1gb".into(),
        });
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back, c);
        // Old or hand-written file with one key: everything else defaults.
        let partial: Config = toml::from_str("theme = \"light\"\nunknown = 3\n").unwrap();
        assert_eq!(partial.theme, "light");
        assert!(partial.live_updates);
        assert!(toml::from_str::<Config>("detail = \"huge\"").is_err());
    }
}
