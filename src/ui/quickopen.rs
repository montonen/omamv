//! Quick open (Ctrl+P): any file in the folder by a few letters of its path.
//! With nothing typed it lists the files read in this window, newest first,
//! then the rest in tree order.

use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};

use crate::folder;
use crate::fuzzy;

const SHOWN: usize = 60;

pub struct QuickOpen {
    pub dialog: adw::Dialog,
    pub entry: gtk::SearchEntry,
    pub list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    files: Vec<PathBuf>,
    recent: Vec<PathBuf>,
    accent: String,
    shown: std::cell::RefCell<Vec<PathBuf>>,
    on_open: Box<dyn Fn(PathBuf)>,
}

impl QuickOpen {
    pub fn new(files: Vec<PathBuf>, recent: Vec<PathBuf>, accent: &str, on_open: impl Fn(PathBuf) + 'static) -> Rc<Self> {
        let entry = gtk::SearchEntry::builder()
            .placeholder_text("Open a file by name")
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
            .title("Open a file")
            .content_width(620)
            .child(&body)
            .build();
        dialog.add_css_class("om-dialog");

        let qo = Rc::new(QuickOpen {
            dialog,
            entry,
            list,
            scroller,
            files,
            recent,
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

    /// The paths shown for a query, best first.
    pub fn matches(&self, query: &str) -> Vec<(PathBuf, Vec<usize>)> {
        if query.trim().is_empty() {
            let mut out: Vec<PathBuf> = Vec::new();
            for f in self.recent.iter().chain(self.files.iter()) {
                if !out.contains(f) && self.files.contains(f) {
                    out.push(f.clone());
                }
            }
            return out.into_iter().take(SHOWN).map(|p| (p, Vec::new())).collect();
        }
        let mut scored: Vec<(i64, PathBuf, Vec<usize>)> = self
            .files
            .iter()
            .filter_map(|f| {
                let s = f.to_string_lossy();
                fuzzy::score(query, &s).map(|m| (m.score, f.clone(), m.positions))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        scored.into_iter().take(SHOWN).map(|(_, p, m)| (p, m)).collect()
    }

    fn refresh(&self) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let matches = self.matches(&self.entry.text());
        if matches.is_empty() {
            let none = gtk::Label::builder().label("No file by that name").xalign(0.0).build();
            none.add_css_class("om-qo-empty");
            let row = gtk::ListBoxRow::builder().child(&none).activatable(false).selectable(false).build();
            self.list.append(&row);
        }
        for (path, positions) in &matches {
            self.list.append(&self.row(path, positions));
        }
        *self.shown.borrow_mut() = matches.into_iter().map(|(p, _)| p).collect();
        if let Some(first) = self.list.row_at_index(0) {
            self.list.select_row(Some(&first));
        }
    }

    fn row(&self, path: &std::path::Path, positions: &[usize]) -> gtk::ListBoxRow {
        let full = path.to_string_lossy();
        let name_start = full.rfind('/').map_or(0, |i| i + 1);
        let chars: Vec<char> = full.chars().collect();
        let name_from = full[..name_start].chars().count();
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
        b.append(&name);
        b.append(&dir);
        b.set_tooltip_text(Some(&folder::file_name(path)));
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
        let Some(path) = self.shown.borrow().get(at).cloned() else { return };
        self.dialog.close();
        (self.on_open)(path);
    }
}
