//! The windows, and what they share: the theme, the `omamv://` scheme.

pub mod debug;
mod keys;
mod quickopen;
mod settings;
mod start;
pub mod status;
mod tree;
pub mod window;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use window::Win;

thread_local! {
    /// The windows' state, held here for as long as each window is open:
    /// everything else (signal handlers, timers, the page) holds it weakly.
    static WINDOWS: RefCell<Vec<Rc<Win>>> = const { RefCell::new(Vec::new()) };
    static PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static THEME_WATCH: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
}

pub fn windows() -> Vec<Rc<Win>> {
    WINDOWS.with(|w| w.borrow().clone())
}

pub(crate) fn remember(win: &Rc<Win>) {
    WINDOWS.with(|w| w.borrow_mut().push(win.clone()));
}

/// The window closed: let its state go.
pub(crate) fn forget(win: &Win) {
    WINDOWS.with(|w| w.borrow_mut().retain(|x| !std::ptr::eq(Rc::as_ptr(x), win)));
}

pub fn startup(app: &adw::Application) {
    crate::scheme::register(&webkit6::WebContext::default().expect("WebKit has a default context"));
    let provider = gtk::CssProvider::new();
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
    PROVIDER.with(|p| *p.borrow_mut() = Some(provider));
    apply_theme(false);
    follow_theme();
    // Without an Omarchy theme, follow the desktop's light or dark.
    adw::StyleManager::default().connect_dark_notify(|_| {
        if crate::theme::load().is_none() {
            apply_theme(true);
        }
    });
    let _ = app;
}

/// Read the palette and paint everything with it; `redraw` draws the open
/// pages again too, keeping each reader's place.
fn apply_theme(redraw: bool) {
    let style = adw::StyleManager::default();
    let palette = match crate::theme::load() {
        Some(p) => {
            style.set_color_scheme(if p.dark { adw::ColorScheme::ForceDark } else { adw::ColorScheme::ForceLight });
            p
        }
        None => {
            style.set_color_scheme(adw::ColorScheme::Default);
            crate::theme::fallback(style.is_dark())
        }
    };
    if redraw && *crate::scheme::palette() == palette {
        return;
    }
    PROVIDER.with(|p| {
        if let Some(p) = p.borrow().as_ref() {
            p.load_from_string(&crate::theme::stylesheet(&palette));
        }
    });
    crate::scheme::set_palette(palette.clone());
    if redraw {
        for w in windows() {
            w.repaint(&palette);
        }
    }
}

/// Settings changed (ADR-0014): every window takes them at once.
pub fn apply_settings() {
    for w in windows() {
        w.apply_settings();
    }
}

/// Watch Omarchy's `current` folder: a theme switch moves `next-theme` to
/// `theme` and then writes `theme.name`.
fn follow_theme() {
    let dir = gio::File::for_path(crate::theme::current_dir());
    let Ok(monitor) = dir.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else {
        return;
    };
    let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    monitor.connect_changed(move |_, _, _, _| {
        if let Some(id) = pending.borrow_mut().take() {
            id.remove();
        }
        let pending2 = pending.clone();
        *pending.borrow_mut() = Some(glib::timeout_add_local_once(Duration::from_millis(250), move || {
            pending2.borrow_mut().take();
            apply_theme(true);
        }));
    });
    THEME_WATCH.with(|t| *t.borrow_mut() = Some(monitor));
}

/// Open `target` (a folder, or a file in one) or, with none, bring a window
/// forward. A folder already open has its window brought forward.
pub fn open(app: &adw::Application, target: Option<PathBuf>) {
    let Some(target) = target else {
        if let Some(w) = windows().into_iter().last() {
            w.window.present();
        } else {
            Win::new(app).window.present();
        }
        return;
    };
    let (root, file) = split_target(&target);
    let open = windows();
    let win = open
        .iter()
        .find(|w| w.root().as_deref() == Some(root.as_path()))
        .or_else(|| open.iter().find(|w| w.root().is_none()))
        .cloned()
        .unwrap_or_else(|| Win::new(app));
    win.open_folder(&root, file);
    win.window.present();
    debug::maybe_start(&win);
}

/// A folder is itself; a file is its folder, and the file in it.
fn split_target(target: &Path) -> (PathBuf, Option<PathBuf>) {
    if target.is_dir() {
        return (target.to_path_buf(), None);
    }
    let root = target.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
    let file = target.file_name().map(PathBuf::from);
    (root, file)
}

/// A key drawn as a chip.
pub fn kbd(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("om-kbd");
    l
}

/// Keys like `Ctrl+P`, drawn as chips joined by a small plus.
pub fn keys(combo: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    for (i, part) in combo.split('+').enumerate() {
        if i > 0 {
            let plus = gtk::Label::new(Some("+"));
            plus.add_css_class("om-plus");
            b.append(&plus);
        }
        b.append(&kbd(if part.is_empty() { "+" } else { part }));
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_opens_its_folder() {
        let dir = std::env::temp_dir();
        assert_eq!(split_target(&dir), (dir.clone(), None));
        let (root, file) = split_target(Path::new("/x/y/README.md"));
        assert_eq!((root, file), (PathBuf::from("/x/y"), Some(PathBuf::from("README.md"))));
    }
}
