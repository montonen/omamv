//! Pick one by a few letters: quick open (Ctrl+P), any file in the folder,
//! newest first with its age (ADR-0017); and the outline (`o`), the headings
//! of the open document in their order.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};

use crate::fuzzy;

/// One line to pick.
pub struct Item {
    /// What the letters typed are matched against.
    pub text: String,
    /// Where the name starts in `text`, in chars; what comes before (a
    /// file's folder) is shown dimmed after the name.
    pub name_from: usize,
    /// On the right, dimmed: a file's age.
    pub aside: String,
    /// Steps in from the left: a heading's depth.
    pub indent: i32,
}

impl Item {
    /// A file, by its path from the root.
    pub fn file(path: &std::path::Path, aside: String) -> Item {
        let text = path.to_string_lossy().into_owned();
        let name_from = text.rfind('/').map_or(0, |i| text[..=i].chars().count());
        Item { text, name_from, aside, indent: 0 }
    }
}

pub struct Kind {
    pub title: &'static str,
    pub placeholder: &'static str,
    pub none: &'static str,
    /// Lines shown at most.
    pub shown: usize,
}

pub const FILES: Kind = Kind { title: "Open a file", placeholder: "Open a file by name", none: "No file by that name", shown: 60 };
pub const OUTLINE: Kind = Kind { title: "Outline", placeholder: "Go to a heading", none: "No heading by that name", shown: 400 };

pub struct QuickOpen {
    pub dialog: adw::Dialog,
    pub entry: gtk::SearchEntry,
    pub list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    items: Vec<Item>,
    kind: Kind,
    accent: String,
    shown: std::cell::RefCell<Vec<usize>>,
    on_open: Box<dyn Fn(usize)>,
}

impl QuickOpen {
    /// `items` in the order shown with nothing typed; `on_open` gets the
    /// index of the one picked.
    pub fn new(kind: Kind, items: Vec<Item>, accent: &str, on_open: impl Fn(usize) + 'static) -> Rc<Self> {
        let entry = gtk::SearchEntry::builder()
            .placeholder_text(kind.placeholder)
            .hexpand(true)
            .build();
        entry.add_css_class("om-qo-entry");
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Browse).build();
        list.add_css_class("om-qo-list");
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(380)
            .child(&list)
            .build();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
        body.set_margin_top(10);
        body.set_margin_bottom(10);
        body.set_margin_start(10);
        body.set_margin_end(10);
        body.append(&entry);
        body.append(&scroller);
        let dialog = adw::Dialog::builder()
            .title(kind.title)
            .content_width(620)
            .child(&body)
            .build();
        dialog.add_css_class("om-dialog");

        let qo = Rc::new(QuickOpen {
            dialog,
            entry,
            list,
            scroller,
            items,
            kind,
            accent: accent.to_owned(),
            shown: Default::default(),
            on_open: Box::new(on_open),
        });
        let weak = Rc::downgrade(&qo);
        qo.entry.connect_search_changed(move |_| {
            if let Some(q) = weak.upgrade() {
                q.refresh();
            }
        });
        // Esc: the search entry takes it for itself (stop-search), so it
        // closes the dialog from there.
        let weak = Rc::downgrade(&qo);
        qo.entry.connect_stop_search(move |_| {
            if let Some(q) = weak.upgrade() {
                q.dialog.close();
            }
        });
        let weak = Rc::downgrade(&qo);
        qo.entry.connect_activate(move |_| {
            if let Some(q) = weak.upgrade() {
                q.open_selected();
            }
        });
        let weak = Rc::downgrade(&qo);
        qo.list.connect_row_activated(move |_, row| {
            if let Some(q) = weak.upgrade() {
                q.open_at(row.index() as usize);
            }
        });
        // The entry keeps the keyboard; the arrows move the selection.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&qo);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(q) = weak.upgrade() else { return glib::Propagation::Proceed };
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            let alt = mods.contains(gdk::ModifierType::ALT_MASK);
            match (key.name().as_deref().unwrap_or(""), ctrl, alt) {
                ("Down", _, _) | ("n" | "j", true, _) => q.step(1),
                ("Up", _, _) | ("p" | "k", true, _) => q.step(-1),
                ("Page_Down", _, _) => q.step(8),
                ("Page_Up", _, _) => q.step(-8),
                (d, false, true) if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() && d != "0" => {
                    q.open_at((d.as_bytes()[0] - b'1') as usize);
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        qo.entry.add_controller(keys);
        qo.refresh();
        qo
    }

    pub fn present(&self, parent: &impl IsA<gtk::Widget>) {
        self.dialog.present(Some(parent));
        self.entry.grab_focus();
    }

    /// The items shown for a query, best first: (index, matched chars).
    /// Equal scores keep the given order (newest first, document order).
    pub fn matches(&self, query: &str) -> Vec<(usize, Vec<usize>)> {
        if query.trim().is_empty() {
            return (0..self.items.len()).take(self.kind.shown).map(|i| (i, Vec::new())).collect();
        }
        let mut scored: Vec<(i64, usize, Vec<usize>)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| fuzzy::score(query, &it.text).map(|m| (m.score, i, m.positions)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        scored.into_iter().take(self.kind.shown).map(|(_, i, m)| (i, m)).collect()
    }

    /// The texts shown for a query (for the checks).
    pub fn texts(&self, query: &str) -> Vec<String> {
        self.matches(query).into_iter().map(|(i, _)| self.items[i].text.clone()).collect()
    }

    fn refresh(&self) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let matches = self.matches(&self.entry.text());
        if matches.is_empty() {
            let none = gtk::Label::builder().label(self.kind.none).xalign(0.0).build();
            none.add_css_class("om-qo-empty");
            let row = gtk::ListBoxRow::builder().child(&none).activatable(false).selectable(false).build();
            self.list.append(&row);
        }
        for (i, positions) in &matches {
            self.list.append(&self.row(&self.items[*i], positions));
        }
        *self.shown.borrow_mut() = matches.into_iter().map(|(i, _)| i).collect();
        if let Some(first) = self.list.row_at_index(0) {
            self.list.select_row(Some(&first));
        }
    }

    fn row(&self, item: &Item, positions: &[usize]) -> gtk::ListBoxRow {
        let chars: Vec<char> = item.text.chars().collect();
        let name_from = item.name_from;
        let marked = |from: usize, to: usize| -> String {
            let mut s = String::new();
            for (i, c) in chars.iter().enumerate().take(to).skip(from) {
                let t = glib::markup_escape_text(&c.to_string());
                if positions.contains(&i) {
                    s.push_str(&format!("<span foreground=\"{}\" weight=\"bold\">{t}</span>", self.accent));
                } else {
                    s.push_str(&t);
                }
            }
            s
        };
        let name = gtk::Label::builder().use_markup(true).xalign(0.0).build();
        name.set_markup(&marked(name_from, chars.len()));
        let dir = gtk::Label::builder()
            .use_markup(true)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::Start)
            .build();
        dir.add_css_class("om-qo-path");
        if name_from > 0 {
            dir.set_markup(&marked(0, name_from.saturating_sub(1)));
        }
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        b.set_margin_start(item.indent * 16);
        b.append(&name);
        b.append(&dir);
        if !item.aside.is_empty() {
            let aside = gtk::Label::new(Some(&item.aside));
            aside.add_css_class("om-qo-aside");
            b.append(&aside);
        }
        gtk::ListBoxRow::builder().child(&b).build()
    }

    fn step(&self, by: i32) {
        let n = self.shown.borrow().len() as i32;
        if n == 0 {
            return;
        }
        let at = self.list.selected_row().map_or(0, |r| r.index());
        let to = (at + by).clamp(0, n - 1);
        if let Some(row) = self.list.row_at_index(to) {
            self.list.select_row(Some(&row));
            // Keep the selected row in view; the keyboard stays in the entry.
            if let Some(b) = row.compute_bounds(&self.list) {
                let adj = self.scroller.vadjustment();
                let (top, bottom) = (b.y() as f64, (b.y() + b.height()) as f64);
                if top < adj.value() {
                    adj.set_value(top);
                } else if bottom > adj.value() + adj.page_size() {
                    adj.set_value(bottom - adj.page_size());
                }
            }
        }
    }

    fn open_selected(&self) {
        let at = self.list.selected_row().map_or(0, |r| r.index().max(0) as usize);
        self.open_at(at);
    }

    fn open_at(&self, at: usize) {
        let Some(i) = self.shown.borrow().get(at).copied() else { return };
        self.dialog.close();
        (self.on_open)(i);
    }
}

/// How long ago, in a few characters: "now", "4 min", "3 h", "2 d", "5 mo", "2 y".
pub fn age(secs: u64) -> String {
    match secs {
        0..60 => "now".into(),
        60..3_600 => format!("{} min", secs / 60),
        3_600..86_400 => format!("{} h", secs / 3_600),
        86_400..5_184_000 => format!("{} d", secs / 86_400),
        5_184_000..63_072_000 => format!("{} mo", secs / 2_592_000),
        _ => format!("{} y", secs / 31_536_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_short() {
        assert_eq!(age(5), "now");
        assert_eq!(age(4 * 60 + 10), "4 min");
        assert_eq!(age(3 * 3600), "3 h");
        assert_eq!(age(2 * 86_400), "2 d");
        assert_eq!(age(150 * 86_400), "5 mo");
        assert_eq!(age(800 * 86_400), "2 y");
    }

    #[test]
    fn a_file_shows_its_name_first() {
        let it = Item::file(std::path::Path::new("docs/adr/0001-x.md"), String::new());
        assert_eq!(it.name_from, "docs/adr/".len());
        assert_eq!(Item::file(std::path::Path::new("README.md"), String::new()).name_from, 0);
    }
}
