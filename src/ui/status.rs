//! The status bar (ADR-0013): the keys that matter where the keyboard is, and
//! the open file's place in the folder. Small, muted, one line.

use std::cell::Cell;

use adw::prelude::*;
use gtk::glib;

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
    /// The gallery of the newest files (ADR-0024).
    Gallery,
}

impl Mode {
    /// (keys, what they do). `j k` are two keys; `Ctrl+P` is a combo.
    pub fn hints(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Mode::Page => &[
                ("j k ↓ ↑", "scroll"),
                ("o", "outline"),
                ("Ctrl+P", "open a file"),
                ("] [", "next / previous file"),
                ("Ctrl+B", "tree"),
                ("/", "find"),
                ("e", "edit"),
                ("?", "all keys"),
            ],
            Mode::Tree => &[
                ("j k ↓ ↑", "move"),
                ("h l ← →", "fold / unfold"),
                ("Enter", "open"),
                ("Esc", "back to the page"),
                ("< >", "width"),
                ("Ctrl+B", "hide the tree"),
                ("?", "all keys"),
            ],
            Mode::Find => &[("Enter", "next"), ("Shift+Enter", "previous"), ("Esc", "close, keep the place")],
            Mode::Dialog => &[("Esc", "close")],
            Mode::QuickOpen => &[("↑ ↓", "move"), ("Enter", "open"), ("Alt+1", "the first"), ("Esc", "close")],
            Mode::Start => &[("1 9", "open a recent folder"), ("j k", "move"), ("Enter", "open"), ("Ctrl+O", "choose a folder"), ("?", "all keys")],
            Mode::Empty => &[("Ctrl+O", "choose a folder"), ("Ctrl+Shift+O", "recent folders"), ("?", "all keys")],
            Mode::Reading => &[],
            Mode::Gallery => &[("1–0", "open a card"), ("← → ↑ ↓", "move"), ("Enter", "open"), ("Esc", "back"), ("?", "all keys")],
        }
    }
}

/// (keys, what they do), built at run time.
type Hints = Vec<(String, String)>;

/// What the window asks the bar to run later.
type Hook = std::rc::Rc<std::cell::RefCell<Option<std::rc::Rc<dyn Fn()>>>>;

pub struct Status {
    pub widget: gtk::Box,
    hints: gtk::Box,
    /// Shortcuts left out for want of room.
    left_out: Cell<usize>,
    /// Fits the hints again once a note has timed out (set by the window).
    refit_hook: Hook,
    place: gtk::Label,
    mode: Cell<Option<Mode>>,
    /// A passing note on the right (the load time), in place of the file's
    /// place, until it times out.
    note: std::rc::Rc<std::cell::RefCell<Option<String>>>,
    last_place: std::rc::Rc<std::cell::RefCell<String>>,
    /// Modifiers held ("Ctrl+Shift"): their shortcuts are shown in place of
    /// the mode's hints until they are let go.
    held: std::cell::RefCell<Option<(String, Hints)>>,
    /// Key hints on in Settings (ADR-0014).
    hints_on: Cell<bool>,
}

impl Status {
    pub fn new() -> Status {
        let hints = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        hints.set_hexpand(true);
        let place = gtk::Label::new(None);
        place.add_css_class("om-status-place");
        // The hints may be cut at the right edge; they never make the window wider.
        let clip = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .hexpand(true)
            .child(&hints)
            .build();
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        widget.add_css_class("om-status");
        widget.append(&clip);
        widget.append(&place);
        Status {
            widget,
            hints,
            left_out: Cell::new(0),
            refit_hook: Default::default(),
            place,
            mode: Cell::new(None),
            note: Default::default(),
            last_place: Default::default(),
            held: Default::default(),
            hints_on: Cell::new(true),
        }
    }

    #[cfg(debug_assertions)]
    pub fn mode(&self) -> Option<Mode> {
        self.mode.get()
    }

    /// Show the keys for `mode`, and `place` (like "3 / 7") on the right.
    pub fn set(&self, mode: Mode, place: Option<(usize, usize)>) {
        let place = place.map(|(at, of)| format!("{at} / {of}")).unwrap_or_default();
        *self.last_place.borrow_mut() = place.clone();
        let before = self.place.label();
        match self.note.borrow().as_ref() {
            Some(note) if place.is_empty() => self.place.set_label(note),
            Some(note) => self.place.set_label(&format!("{note}   ·   {place}")),
            None => self.place.set_label(&place),
        }
        let same = self.mode.replace(Some(mode)) == Some(mode);
        if !same || self.place.label() != before {
            self.refit();
        }
    }

    /// Fill the bar with what it shows now, held shortcuts or the mode's
    /// keys, and fit them to the room there is.
    pub fn refit(&self) {
        if let Some((p, list)) = self.held.borrow().clone() {
            self.fill(Some(&p), list.iter().map(|(k, w)| (k.as_str(), w.as_str())));
        } else if let Some(mode) = self.mode.get() {
            self.fill(None, mode.hints().iter().map(|(k, w)| (*k, *w)));
        }
        self.fit();
    }

    /// What the window does to fit the hints again later (a note timing out).
    pub fn on_refit(&self, f: impl Fn() + 'static) {
        *self.refit_hook.borrow_mut() = Some(std::rc::Rc::new(f));
    }

    fn fill<'a>(&self, prefix: Option<&str>, hints: impl Iterator<Item = (&'a str, &'a str)>) {
        while let Some(child) = self.hints.first_child() {
            self.hints.remove(&child);
        }
        if let Some(prefix) = prefix {
            // "Ctrl +": what is held, then what finishes it.
            let lead = super::keys(prefix);
            let plus = gtk::Label::new(Some("+"));
            plus.add_css_class("om-plus");
            lead.append(&plus);
            self.hints.append(&lead);
        }
        for (keys, what) in hints {
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

    /// Modifiers held (`Some("Ctrl+Shift")`) or let go (`None`). Held, the
    /// bar lists the shortcuts that start with them, also with key hints
    /// off: holding them is asking.
    pub fn hold(&self, prefix: Option<&str>) {
        let list = prefix.map(|p| super::keys::held(&p.split('+').collect::<Vec<_>>())).filter(|l| !l.is_empty());
        match (prefix, list) {
            (Some(p), Some(list)) => {
                if self.held.borrow().as_ref().is_some_and(|(q, _)| q == p) {
                    return;
                }
                *self.held.borrow_mut() = Some((p.to_owned(), list));
                self.hints.set_visible(true);
                self.refit();
            }
            _ => {
                if self.held.borrow_mut().take().is_none() {
                    return;
                }
                self.refit();
                self.hints.set_visible(self.hints_on.get());
            }
        }
    }

    /// Hints that would run past the edge (or into the note on the right)
    /// are left out, whole, and a last chip says how many and where they all
    /// are (`?`). The room is the bar less the note, worked out now rather
    /// than from the last layout, so a note that just came is counted.
    fn fit(&self) {
        self.left_out.set(0);
        let bar = self.widget.width();
        if bar <= 0 {
            return;
        }
        let (_, place, _, _) = self.place.measure(gtk::Orientation::Horizontal, -1);
        let room = bar - place - self.widget.spacing() - 24;
        // What the "+N more" chip at the end needs.
        const MORE: i32 = 120;
        let kids: Vec<gtk::Widget> = std::iter::successors(self.hints.first_child(), |c| c.next_sibling()).collect();
        let mut x = 0;
        let mut cut = None;
        for (i, kid) in kids.iter().enumerate() {
            let (_, natural, _, _) = kid.measure(gtk::Orientation::Horizontal, -1);
            x += natural + if i > 0 { self.hints.spacing() } else { 0 };
            let last = i + 1 == kids.len();
            if x > room - if last { 0 } else { MORE } && i > 0 {
                cut = Some(i);
                break;
            }
        }
        let Some(cut) = cut else { return };
        for kid in &kids[cut..] {
            self.hints.remove(kid);
        }
        self.left_out.set(kids.len() - cut);
        let more = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        more.append(&super::kbd("?"));
        let label = gtk::Label::new(Some(&format!("{} more", kids.len() - cut)));
        label.add_css_class("om-status-label");
        more.append(&label);
        self.hints.append(&more);
    }

    /// Held shortcuts not shown for want of room (for the checks).
    #[cfg(debug_assertions)]
    pub fn left_out(&self) -> usize {
        self.left_out.get()
    }

    /// The modifiers whose shortcuts are showing.
    pub fn held(&self) -> Option<String> {
        self.held.borrow().as_ref().map(|(p, _)| p.clone())
    }

    /// Show `text` on the right for a few seconds.
    pub fn note(&self, text: &str) {
        *self.note.borrow_mut() = Some(text.to_owned());
        let place = self.last_place.borrow().clone();
        self.place.set_label(&if place.is_empty() { text.to_owned() } else { format!("{text}   ·   {place}") });
        let (note, label, last) = (self.note.clone(), self.place.clone(), self.last_place.clone());
        let mine = text.to_owned();
        self.refit();
        let hook = self.refit_hook.clone();
        glib::timeout_add_local_once(std::time::Duration::from_secs(8), move || {
            if note.borrow().as_deref() == Some(mine.as_str()) {
                note.borrow_mut().take();
                label.set_label(&last.borrow());
                if let Some(f) = hook.borrow().clone() {
                    f();
                }
            }
        });
    }

    /// The note on the right, if one is showing (for the checks).
    #[cfg(debug_assertions)]
    pub fn shown_note(&self) -> Option<String> {
        self.note.borrow().clone()
    }

    /// Key hints on or off (ADR-0014); the bar stays for the place and notes.
    pub fn show_hints(&self, on: bool) {
        self.hints_on.set(on);
        self.hints.set_visible(on || self.held.borrow().is_some());
    }

    #[cfg(debug_assertions)]
    pub fn hints_shown(&self) -> bool {
        self.hints.is_visible()
    }

    /// Every hint as text, for the checks.
    #[cfg(debug_assertions)]
    pub fn text(&self) -> String {
        if let Some((_, list)) = self.held.borrow().as_ref() {
            return list.iter().map(|(k, w)| format!("{k} {w}")).collect::<Vec<_>>().join(" · ");
        }
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
