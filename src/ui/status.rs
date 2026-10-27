//! The status bar (ADR-0013): the keys that matter where the keyboard is, and
//! the open file's place in the folder. Small, muted, one line.

use std::cell::Cell;

use adw::prelude::*;

/// Where the keyboard is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Page,
    Tree,
    Find,
    QuickOpen,
    /// Another dialog: the keys, About.
    Dialog,
    Start,
    Empty,
    Reading,
}

impl Mode {
    /// (keys, what they do). `j k` are two keys; `Ctrl+P` is a combo.
    pub fn hints(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Mode::Page => &[
                ("j k", "scroll"),
                ("d u", "half page"),
                ("Ctrl+P", "open a file"),
                ("] [", "next / previous file"),
                ("Ctrl+B", "tree"),
                ("/", "find"),
                ("?", "all keys"),
            ],
            Mode::Tree => &[
                ("j k", "move"),
                ("h l", "fold / unfold"),
                ("Enter", "open"),
                ("Esc", "back to the page"),
                ("Ctrl+B", "hide the tree"),
                ("?", "all keys"),
            ],
            Mode::Find => &[("Enter", "next"), ("Shift+Enter", "previous"), ("Esc", "close, keep the place")],
            Mode::Dialog => &[("Esc", "close")],
            Mode::QuickOpen => &[("↑ ↓", "move"), ("Enter", "open"), ("Alt+1", "open the first"), ("Esc", "close")],
            Mode::Start => &[("1 9", "open a recent folder"), ("j k", "move"), ("Enter", "open"), ("Ctrl+O", "choose a folder"), ("?", "all keys")],
            Mode::Empty => &[("Ctrl+O", "choose a folder"), ("Ctrl+Shift+O", "recent folders"), ("?", "all keys")],
            Mode::Reading => &[],
        }
    }
}

pub struct Status {
    pub widget: gtk::Box,
    hints: gtk::Box,
    place: gtk::Label,
    mode: Cell<Option<Mode>>,
}

impl Status {
    pub fn new() -> Status {
        let hints = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        hints.set_hexpand(true);
        let place = gtk::Label::new(None);
        place.add_css_class("om-status-place");
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        widget.add_css_class("om-status");
        widget.append(&hints);
        widget.append(&place);
        Status { widget, hints, place, mode: Cell::new(None) }
    }

    pub fn mode(&self) -> Option<Mode> {
        self.mode.get()
    }

    /// Show the keys for `mode`, and `place` (like "3 / 7") on the right.
    pub fn set(&self, mode: Mode, place: Option<(usize, usize)>) {
        self.place.set_label(&place.map(|(at, of)| format!("{at} / {of}")).unwrap_or_default());
        if self.mode.replace(Some(mode)) == Some(mode) {
            return;
        }
        while let Some(child) = self.hints.first_child() {
            self.hints.remove(&child);
        }
        for (keys, what) in mode.hints() {
            let hint = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let chips = gtk::Box::new(gtk::Orientation::Horizontal, 3);
            if keys.contains(' ') {
                for k in keys.split(' ') {
                    chips.append(&super::kbd(k));
                }
            } else {
                chips.append(&super::keys(keys));
            }
            hint.append(&chips);
            let label = gtk::Label::new(Some(what));
            label.add_css_class("om-status-label");
            hint.append(&label);
            self.hints.append(&hint);
        }
    }

    /// Every hint as text, for the checks.
    pub fn text(&self) -> String {
        self.mode.get().map(|m| m.hints().iter().map(|(k, w)| format!("{k} {w}")).collect::<Vec<_>>().join(" · ")).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_place_with_keys_names_a_way_to_all_of_them_or_out() {
        for mode in [Mode::Page, Mode::Tree, Mode::Start, Mode::Empty] {
            assert!(mode.hints().iter().any(|(k, _)| *k == "?"), "{mode:?} points to ?");
        }
        for mode in [Mode::Tree, Mode::Find, Mode::QuickOpen, Mode::Dialog] {
            assert!(mode.hints().iter().any(|(k, _)| *k == "Esc"), "{mode:?} says how to get out");
        }
    }
}
