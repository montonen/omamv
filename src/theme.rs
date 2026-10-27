//! The active Omarchy theme, followed live.
//!
//! Omarchy keeps the active theme behind `~/.local/state/omarchy/current/theme`
//! (palette in `colors.toml`, name in the sibling `theme.name`). A switch moves
//! `current/next-theme` to `current/theme`, then writes `current/theme.name`:
//! watch the `current` directory, not files inside `theme/`.
//!
//! The same palette colours two things: the GTK chrome (libadwaita's named
//! colours, redefined) and the document page (CSS variables, `page_vars`).
//! Omarchy's conventions: background for the page, lighter_background for what
//! stands apart (the tree, dialogs, code), muted for lines, accent for focus and
//! links, red only for what demands attention. Muted text is the foreground at
//! 70%, not a palette key.

use std::path::{Path, PathBuf};

/// The colours omamv uses, each `#rrggbb`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub dark: bool,
    pub background: String,
    pub lighter_background: String,
    pub foreground: String,
    pub bright_foreground: String,
    pub dim_foreground: String,
    pub accent: String,
    pub muted: String,
    pub red: String,
    pub yellow: String,
    pub orange: String,
    pub green: String,
    pub blue: String,
    pub cyan: String,
    pub magenta: String,
}

/// Tokyo Night, for a desktop with no Omarchy theme and a dark preference.
const FALLBACK_DARK: &str = r##"
mode = "dark"
accent = "#7aa2f7"
muted = "#414868"
background = "#1a1b26"
lighter_background = "#24283b"
foreground = "#a9b1d6"
dark_foreground = "#565f89"
bright_foreground = "#c0caf5"
red = "#f7768e"
yellow = "#e0af68"
orange = "#eb927b"
green = "#9ece6a"
cyan = "#449dab"
blue = "#7aa2f7"
magenta = "#ad8ee6"
"##;

/// Catppuccin Latte, for a desktop with no Omarchy theme and a light preference.
const FALLBACK_LIGHT: &str = r##"
mode = "light"
accent = "#1e66f5"
muted = "#acb0be"
background = "#eff1f5"
lighter_background = "#dce0e8"
foreground = "#4c4f69"
dark_foreground = "#9ca0b0"
bright_foreground = "#4c4f69"
red = "#d20f39"
yellow = "#df8e1d"
orange = "#d84e2b"
green = "#40a02b"
cyan = "#179299"
blue = "#1e66f5"
magenta = "#ea76cb"
"##;

/// `$XDG_STATE_HOME/omarchy/current` (default `~/.local/state/omarchy/current`).
pub fn current_dir() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| {
            std::env::home_dir()
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".local/state")
        });
    state.join("omarchy/current")
}

/// The active palette, or `None` when there is no Omarchy theme.
pub fn load() -> Option<Palette> {
    load_from(&current_dir())
}

fn load_from(current: &Path) -> Option<Palette> {
    let theme = current.join("theme");
    let text = std::fs::read_to_string(theme.join("colors.toml")).ok()?;
    resolve(&text, theme.join("light.mode").is_file())
}

/// The palette for a desktop without an Omarchy theme.
pub fn fallback(dark: bool) -> Palette {
    resolve(if dark { FALLBACK_DARK } else { FALLBACK_LIGHT }, false)
        .expect("the built-in palettes parse")
}

#[cfg(test)]
/// Parse `colors.toml` text by the rules of `omarchy-theme-color`: each key's
/// fallbacks in its order, values that are not hex colours ignored.
pub fn parse(colors_toml: &str) -> Option<Palette> {
    resolve(colors_toml, false)
}

fn resolve(colors_toml: &str, marked_light: bool) -> Option<Palette> {
    let table = read_table(colors_toml);
    let word = |key: &str| {
        table
            .get(key)
            .and_then(toml::Value::as_str)
            .map(|value| value.trim())
    };
    let colour = |keys: &[&str]| {
        keys.iter()
            .filter_map(|key| word(key))
            .find(|value| is_hex(value))
            .map(|value| hex(rgb(value)))
    };
    let background = colour(&["background", "bg", "color0"])?;
    let foreground = colour(&["foreground", "fg", "color7"])?;
    let dark = match ["mode", "theme_type"]
        .into_iter()
        .filter_map(word)
        .find(|m| !m.is_empty())
    {
        Some(mode) => mode != "light",
        None if marked_light => false,
        None => !is_light(rgb(&background)),
    };
    let accent = colour(&["accent", "blue", "color4"]).unwrap_or_else(|| "#3584e4".into());
    Some(Palette {
        dark,
        lighter_background: colour(&["lighter_background", "lighter_bg"])
            .unwrap_or_else(|| hex(mix(rgb(&background), rgb(&foreground), 0.92))),
        muted: colour(&["muted", "color8", "dark_foreground", "dark_fg"])
            .unwrap_or_else(|| hex(mix(rgb(&background), rgb(&foreground), 0.7))),
        bright_foreground: colour(&["bright_foreground", "bright_fg", "light_foreground", "color15"])
            .unwrap_or_else(|| foreground.clone()),
        dim_foreground: colour(&["dark_foreground", "dark_fg", "color8"])
            .unwrap_or_else(|| hex(mix(rgb(&foreground), rgb(&background), 0.55))),
        red: colour(&["red", "color1"]).unwrap_or_else(|| "#e01b24".into()),
        yellow: colour(&["yellow", "color3"]).unwrap_or_else(|| "#e5a50a".into()),
        orange: colour(&["orange", "yellow", "color3"]).unwrap_or_else(|| "#e5a50a".into()),
        green: colour(&["green", "color2"]).unwrap_or_else(|| "#2ec27e".into()),
        blue: colour(&["blue", "color4"]).unwrap_or_else(|| accent.clone()),
        cyan: colour(&["cyan", "color6"]).unwrap_or_else(|| "#2190a4".into()),
        magenta: colour(&["magenta", "color5", "purple"]).unwrap_or_else(|| "#9141ac".into()),
        accent,
        background,
        foreground,
    })
}

/// One bad line makes a TOML document invalid, where Omarchy's line reader
/// keeps every other line; so on an error each line is read on its own.
fn read_table(text: &str) -> toml::Table {
    text.parse::<toml::Table>().unwrap_or_else(|_| {
        let mut table = toml::Table::new();
        for line in text.lines() {
            if let Ok(one) = line.parse::<toml::Table>() {
                table.extend(one);
            }
        }
        table
    })
}

/// `#` and three or six hex digits, checked byte by byte: nothing else may
/// reach a stylesheet.
fn is_hex(value: &str) -> bool {
    let bytes = value.as_bytes();
    matches!(bytes.len(), 4 | 7) && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}

/// Omarchy's rule for a theme that does not say: light when red, green and
/// blue add up to more than 382 of 765.
fn is_light([r, g, b]: Rgb) -> bool {
    u16::from(r) + u16::from(g) + u16::from(b) > 382
}

type Rgb = [u8; 3];

fn rgb(hex: &str) -> Rgb {
    if !is_hex(hex) {
        return [0, 0, 0];
    }
    let digit = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
    let h = &hex.as_bytes()[1..];
    match h.len() {
        3 => [h[0], h[1], h[2]].map(|c| digit(c) * 17),
        _ => [0, 2, 4].map(|i| (digit(h[i]) << 4) | digit(h[i + 1])),
    }
}

fn hex([r, g, b]: Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// `a` with `share` of it kept and the rest `b`.
fn mix(a: Rgb, b: Rgb, share: f64) -> Rgb {
    [0, 1, 2].map(|i| (f64::from(a[i]) * share + f64::from(b[i]) * (1.0 - share)).round() as u8)
}

impl Palette {
    /// Foreground at `share` over the background, as a solid colour. Mermaid
    /// cannot read `color-mix`, so the page gets these precomputed too.
    pub fn fg_over_bg(&self, share: f64) -> String {
        hex(mix(rgb(&self.foreground), rgb(&self.background), share))
    }
}

/// The page's colour variables, read by `assets/page.css` and `assets/app.js`.
pub fn page_vars(p: &Palette) -> String {
    format!(
        ":root{{--bg:{bg};--bg-lighter:{raised};--fg:{fg};--fg-bright:{bright};--fg-dark:{dim};\
--fg-70:{fg70};--fg-50:{fg50};--fg-22:{fg22};--fg-14:{fg14};--fg-08:{fg08};\
--muted:{muted};--muted-60:{muted60};--accent:{accent};--accent-50:{accent50};\
--red:{red};--yellow:{yellow};--orange:{orange};--green:{green};--blue:{blue};--cyan:{cyan};--magenta:{magenta};\
--dark:{dark};color-scheme:{scheme}}}",
        bg = p.background,
        raised = p.lighter_background,
        fg = p.foreground,
        bright = p.bright_foreground,
        dim = p.dim_foreground,
        fg70 = p.fg_over_bg(0.7),
        fg50 = p.fg_over_bg(0.5),
        fg22 = p.fg_over_bg(0.22),
        fg14 = p.fg_over_bg(0.14),
        fg08 = p.fg_over_bg(0.08),
        muted = p.muted,
        muted60 = hex(mix(rgb(&p.muted), rgb(&p.background), 0.6)),
        accent = p.accent,
        accent50 = hex(mix(rgb(&p.accent), rgb(&p.background), 0.5)),
        red = p.red,
        yellow = p.yellow,
        orange = p.orange,
        green = p.green,
        blue = p.blue,
        cyan = p.cyan,
        magenta = p.magenta,
        dark = if p.dark { 1 } else { 0 },
        scheme = if p.dark { "dark" } else { "light" },
    )
}

/// The GTK stylesheet: libadwaita's named colours redefined from the palette,
/// then omamv's own classes. It sits above libadwaita's provider, at
/// `STYLE_PROVIDER_PRIORITY_APPLICATION`.
pub fn stylesheet(p: &Palette) -> String {
    let on_accent = if is_light(rgb(&p.accent)) { "#000000" } else { "#ffffff" };
    let bg = &p.background;
    let raised = &p.lighter_background;
    let fg = &p.foreground;
    let muted = &p.muted;
    let colours = [
        ("accent_bg_color", p.accent.as_str()),
        ("accent_fg_color", on_accent),
        ("accent_color", p.accent.as_str()),
        ("destructive_bg_color", p.red.as_str()),
        ("destructive_color", p.red.as_str()),
        ("error_color", p.red.as_str()),
        ("warning_color", p.yellow.as_str()),
        ("success_color", p.green.as_str()),
        ("window_bg_color", bg),
        ("window_fg_color", fg),
        ("view_bg_color", bg),
        ("view_fg_color", fg),
        ("headerbar_bg_color", bg),
        ("headerbar_fg_color", fg),
        ("headerbar_border_color", fg),
        ("headerbar_backdrop_color", bg),
        ("headerbar_shade_color", muted),
        ("sidebar_bg_color", raised),
        ("sidebar_fg_color", fg),
        ("sidebar_backdrop_color", raised),
        ("sidebar_border_color", muted),
        ("secondary_sidebar_bg_color", raised),
        ("secondary_sidebar_fg_color", fg),
        ("card_bg_color", raised),
        ("card_fg_color", fg),
        ("dialog_bg_color", raised),
        ("dialog_fg_color", fg),
        ("popover_bg_color", raised),
        ("popover_fg_color", fg),
    ];
    let mut css = String::with_capacity(6000);
    for (name, value) in colours {
        css.push_str(&format!("@define-color {name} {value};\n"));
    }
    css.push_str(&format!(
        r#"
@define-color om_fg {fg};
@define-color om_bright {bright};
@define-color om_muted {muted};
@define-color om_accent {accent};
@define-color om_red {red};
"#,
        bright = p.bright_foreground,
        accent = p.accent,
        red = p.red,
    ));
    css.push_str(OM_CLASSES);
    css
}

/// omamv's classes. Muted text is the foreground at 70%; state fills are the
/// foreground at a low alpha; lines are muted.
const OM_CLASSES: &str = r#"
/* Header: flat, a hairline only while the page is scrolled. */
headerbar.om-header { box-shadow: none; border-bottom: 1px solid transparent; background: @window_bg_color; }
headerbar.om-header.om-scrolled { border-bottom-color: alpha(@om_muted, 0.6); }
headerbar.om-header button { color: alpha(@om_fg, 0.7); }
headerbar.om-header button:checked { background: none; color: alpha(@om_fg, 0.7); }
headerbar.om-header button:hover { background: alpha(@om_fg, 0.08); }
.om-title { font-weight: 700; color: @om_bright; }
.om-subtitle { font-size: 0.86em; color: alpha(@om_fg, 0.7); }
.om-changed { font-size: 0.86em; color: alpha(@om_fg, 0.7); }
.om-dot { color: @om_accent; }
.om-gone { font-size: 0.86em; color: @om_red; }

/* Folder tree: slim, muted until it has the keyboard. */
.om-tree { background: @sidebar_bg_color; border-left: 1px solid alpha(@om_muted, 0.6); color: alpha(@om_fg, 0.7); }
.om-tree:focus-within { color: @om_fg; }
.om-tree-head { padding: 4px 14px 10px 14px; font-size: 0.86em; }
.om-tree-name { font-weight: 600; letter-spacing: 0.02em; color: alpha(@om_fg, 0.7); }
.om-tree:focus-within .om-tree-name { color: @om_bright; }
.om-tree-count { color: alpha(@om_fg, 0.5); }
.om-tree listview { background: transparent; color: inherit; }
.om-tree listview > row { min-height: 28px; padding: 0 12px 0 0; border-radius: 0; color: inherit; background: transparent; outline: none; }
.om-tree listview > row:selected { color: @om_bright; background: alpha(@om_fg, 0.08); box-shadow: inset 2px 0 @om_accent; }
.om-tree:focus-within listview > row:focus-visible,
.om-tree:focus-within listview > row:focus { outline: 2px solid @om_accent; outline-offset: -2px; color: @om_bright; background: alpha(@om_fg, 0.08); }
.om-tree .om-dir { font-weight: 500; }
.om-tree expander { color: alpha(@om_fg, 0.5); }

/* Keys drawn as chips. */
.om-kbd { font-family: "JetBrainsMono Nerd Font", "JetBrains Mono", monospace; font-size: 0.82em; color: @om_fg;
  border: 1px solid @om_muted; border-radius: 4px; padding: 0 6px; background: @window_bg_color; min-width: 12px; }
.om-plus { color: alpha(@om_fg, 0.5); font-size: 0.82em; }
.om-term { font-family: "JetBrainsMono Nerd Font", "JetBrains Mono", monospace; font-size: 0.9em; color: @om_bright;
  background: @sidebar_bg_color; border: 1px solid alpha(@om_muted, 0.6); border-radius: 6px; padding: 3px 10px; }

/* Dialogs: quick open and keys. */
dialog.om-dialog > floating-sheet > sheet,
dialog.om-dialog sheet { background: @sidebar_bg_color; border: 1px solid @om_muted; border-radius: 12px; }
.om-qo-entry { min-height: 40px; font-size: 1.05em; border: 1px solid @om_accent; border-radius: 8px; background: @window_bg_color; }
.om-qo-list { background: transparent; }
.om-qo-list > row { min-height: 36px; border-radius: 8px; padding: 0 10px; background: transparent; }
.om-qo-list > row:selected { background: alpha(@om_fg, 0.08); color: @om_bright; box-shadow: inset 2px 0 @om_accent; }
.om-qo-path { color: alpha(@om_fg, 0.7); font-size: 0.9em; }
.om-qo-empty { color: alpha(@om_fg, 0.7); padding: 10px; }
.om-dlg-title { font-weight: 700; font-size: 1.1em; color: @om_bright; }
.om-dlg-sub { font-size: 0.86em; color: alpha(@om_fg, 0.7); }
.om-kg-title { font-size: 0.86em; font-weight: 600; color: alpha(@om_fg, 0.7); letter-spacing: 0.06em; }
.om-kr { min-height: 32px; border-bottom: 1px solid alpha(@om_muted, 0.6); }
.om-ka { color: alpha(@om_fg, 0.7); }

/* Find bar under the header. */
searchbar.om-find > revealer > box { background: @window_bg_color; border-bottom: 1px solid alpha(@om_muted, 0.6); box-shadow: none; }
.om-find-entry { min-width: 420px; }
.om-find-count { color: alpha(@om_fg, 0.7); font-size: 0.9em; font-feature-settings: "tnum"; }

/* Start: no folder open. */
.om-start-mark { font-weight: 700; letter-spacing: 0.14em; color: alpha(@om_fg, 0.5); }
.om-start-lead { font-size: 1.85em; font-weight: 600; color: @om_bright; }
.om-recent-title { font-size: 0.86em; font-weight: 600; color: alpha(@om_fg, 0.7); letter-spacing: 0.06em; }
.om-recent { background: transparent; }
.om-recent > row { min-height: 38px; border-radius: 8px; padding: 0 10px; background: transparent; }
.om-recent > row:selected, .om-recent > row:focus { background: alpha(@om_fg, 0.08); color: @om_bright; box-shadow: inset 2px 0 @om_accent; outline: none; }
.om-rpath { font-family: "JetBrainsMono Nerd Font", "JetBrains Mono", monospace; font-size: 0.95em; }
.om-rwhen { color: alpha(@om_fg, 0.7); font-size: 0.9em; }
.om-hint { color: alpha(@om_fg, 0.7); }
.om-foot { color: alpha(@om_fg, 0.5); }

/* The banner for a removed file. */
banner.om-banner > revealer > widget { background: @sidebar_bg_color; color: @om_bright; border-bottom: 1px solid alpha(@om_muted, 0.6); }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_an_omarchy_palette() {
        let p = parse(FALLBACK_DARK).unwrap();
        assert!(p.dark);
        assert_eq!(p.background, "#1a1b26");
        assert_eq!(p.accent, "#7aa2f7");
        assert_eq!(p.bright_foreground, "#c0caf5");
    }

    #[test]
    fn falls_back_by_omarchys_rules() {
        let p = parse("background = \"#ffffff\"\nforeground = \"#000\"\nblue = \"#0000ff\"").unwrap();
        assert!(!p.dark, "a white background with no mode is light");
        assert_eq!(p.foreground, "#000000", "#rgb is expanded");
        assert_eq!(p.accent, "#0000ff", "the accent falls back to blue");
    }

    #[test]
    fn keeps_good_lines_of_a_broken_file() {
        let p = parse("background = \"#101010\"\nthis is not toml\nforeground = \"#eeeeee\"").unwrap();
        assert_eq!(p.background, "#101010");
    }

    #[test]
    fn nothing_but_colours_reaches_a_stylesheet() {
        let p = parse("background = \"#000;}*{x\"\nforeground = \"#fff\"");
        assert!(p.is_none(), "a background that is not a colour is no background");
        let css = stylesheet(&fallback(true));
        assert!(css.contains("@define-color window_bg_color #1a1b26;"));
    }

    #[test]
    fn page_vars_are_solid_colours() {
        let vars = page_vars(&fallback(false));
        assert!(!vars.contains("color-mix"), "Mermaid reads these and cannot take color-mix");
        assert!(vars.contains("--fg-70:#"));
    }
}
