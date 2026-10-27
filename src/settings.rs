//! Settings (ADR-0014): kept in `$XDG_CONFIG_HOME/omamv/settings.toml`,
//! omamv's own file, never in a folder it reads (ADR-0002).

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    /// About 66 characters, the Quiet page measure.
    Narrow,
    Wide,
    Full,
}

impl Width {
    pub const ALL: [Width; 3] = [Width::Narrow, Width::Wide, Width::Full];

    pub fn name(self) -> &'static str {
        match self {
            Width::Narrow => "narrow",
            Width::Wide => "wide",
            Width::Full => "full",
        }
    }

    /// The page's max-width in CSS.
    pub fn css(self) -> &'static str {
        match self {
            Width::Narrow => "704px",
            Width::Wide => "944px",
            Width::Full => "none",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Percent.
    pub text_size: u32,
    pub width: Width,
    pub tree_left: bool,
    pub key_hints: bool,
    pub hidden: bool,
    pub gitignore: bool,
    pub web_pictures: bool,
    /// When a folder opens: show its first file, or ask for one by name.
    pub ask_on_open: bool,
    /// HTML pages listed and shown, without their scripts (ADR-0016).
    pub html: bool,
}

/// The text sizes offered, in percent.
pub const TEXT_SIZES: [u32; 6] = [80, 90, 100, 110, 125, 150];

impl Default for Settings {
    fn default() -> Self {
        Settings {
            text_size: 100,
            width: Width::Narrow,
            tree_left: false,
            key_hints: true,
            hidden: false,
            gitignore: true,
            web_pictures: true,
            ask_on_open: false,
            html: false,
        }
    }
}

impl Settings {
    pub fn zoom(&self) -> f64 {
        f64::from(self.text_size) / 100.0
    }

    pub fn parse(text: &str) -> Settings {
        let table: toml::Table = text.parse().unwrap_or_default();
        let d = Settings::default();
        let flag = |k: &str, or: bool| table.get(k).and_then(toml::Value::as_bool).unwrap_or(or);
        Settings {
            text_size: table
                .get("text_size")
                .and_then(toml::Value::as_integer)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| TEXT_SIZES.contains(n))
                .unwrap_or(d.text_size),
            width: table
                .get("page_width")
                .and_then(toml::Value::as_str)
                .and_then(|w| Width::ALL.into_iter().find(|x| x.name() == w))
                .unwrap_or(d.width),
            tree_left: table.get("tree_side").and_then(toml::Value::as_str).map_or(d.tree_left, |s| s == "left"),
            key_hints: flag("key_hints", d.key_hints),
            hidden: flag("show_hidden", d.hidden),
            gitignore: flag("respect_gitignore", d.gitignore),
            web_pictures: flag("web_pictures", d.web_pictures),
            ask_on_open: table.get("on_open").and_then(toml::Value::as_str).map_or(d.ask_on_open, |s| s == "open-by-name"),
            html: flag("show_html", d.html),
        }
    }

    pub fn to_toml(&self) -> String {
        format!(
            "# omamv settings (Ctrl+, in the window)\n\
             text_size = {}\npage_width = \"{}\"\ntree_side = \"{}\"\nkey_hints = {}\n\
             show_hidden = {}\nrespect_gitignore = {}\nweb_pictures = {}\non_open = \"{}\"\nshow_html = {}\n",
            self.text_size,
            self.width.name(),
            if self.tree_left { "left" } else { "right" },
            self.key_hints,
            self.hidden,
            self.gitignore,
            self.web_pictures,
            if self.ask_on_open { "open-by-name" } else { "first-file" },
            self.html,
        )
    }
}

fn file() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute())
        .unwrap_or_else(|| std::env::home_dir().unwrap_or_default().join(".config"))
        .join("omamv/settings.toml")
}

fn cell() -> &'static Mutex<Settings> {
    static CELL: OnceLock<Mutex<Settings>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(Settings::parse(&std::fs::read_to_string(file()).unwrap_or_default())))
}

pub fn get() -> Settings {
    cell().lock().map(|s| s.clone()).unwrap_or_default()
}

/// Keep `s` and write it down. A failure to write is reported, not fatal.
pub fn set(s: Settings) {
    if let Ok(mut c) = cell().lock() {
        *c = s.clone();
    }
    let f = file();
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = f.with_extension("toml.tmp");
    let written = std::fs::write(&tmp, s.to_toml()).and_then(|()| std::fs::rename(&tmp, &f));
    if let Err(e) = written {
        eprintln!("omamv: settings not saved to {}: {e}", f.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = Settings {
            text_size: 125,
            width: Width::Wide,
            tree_left: true,
            key_hints: false,
            hidden: true,
            gitignore: false,
            web_pictures: false,
            ask_on_open: true,
            html: true,
        };
        assert_eq!(Settings::parse(&s.to_toml()), s);
    }

    #[test]
    fn nonsense_falls_back_to_defaults() {
        assert_eq!(Settings::parse("text_size = 7\npage_width = \"huge\"\nthis is not toml"), Settings::default());
        assert_eq!(Settings::parse("text_size = 110").text_size, 110);
    }
}
