//! Search all files (Ctrl+Shift+F, ADR-0025): a word typed, every line in the
//! folder that has it, newest file first; Enter opens the file at that place.
//! Looks like quick open; the search itself runs off the main thread.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gdk, glib};

use crate::search::{self, Doc, Found, Hit};

/// Hits shown at most; the summary counts the rest.
pub const SHOWN: usize = 200;

/// What opening a hit needs: the file, the hit, the query.
type OnOpen = Box<dyn Fn(&PathBuf, &Hit, &str)>;

pub struct SearchAll {
    pub dialog: adw::Dialog,
    pub entry: gtk::SearchEntry,
    pub list: gtk::ListBox,
    pub summary: gtk::Label,
    scroller: gtk::ScrolledWindow,
    corpus: RefCell<Option<Arc<Vec<Doc>>>>,
    found: RefCell<Found>,
    /// Bumped by each search: an answer to an older one is dropped.
    generation: Cell<u64>,
    /// The row to select when the answer comes (reopened with Ctrl+Shift+F).
    select_next: Cell<Option<usize>>,
    /// Searches answered so far, and the query answered last (the checks
    /// wait on them).
    pub answers: Cell<u32>,
    pub answered: RefCell<String>,
    accent: String,
    on_open: OnOpen,
}

impl SearchAll {
    pub fn new(accent: &str, on_open: impl Fn(&PathBuf, &Hit, &str) + 'static) -> Rc<Self> {
        let entry = gtk::SearchEntry::builder().placeholder_text("Search all files").hexpand(true).build();
        entry.add_css_class("om-qo-entry");
        let summary = gtk::Label::builder().xalign(0.0).build();
        summary.add_css_class("om-qo-aside");
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Browse).build();
        list.add_css_class("om-qo-list");
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(420)
            .child(&list)
            .build();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
        body.set_margin_top(10);
        body.set_margin_bottom(10);
        body.set_margin_start(10);
        body.set_margin_end(10);
        body.append(&entry);
        body.append(&summary);
        body.append(&scroller);
        let dialog = adw::Dialog::builder().title("Search all files").content_width(760).child(&body).build();
        dialog.add_css_class("om-dialog");

        let sa = Rc::new(SearchAll {
            dialog,
            entry,
            list,
            summary,
            scroller,
            corpus: RefCell::new(None),
            found: RefCell::new(Found::default()),
            generation: Cell::new(0),
            select_next: Cell::new(None),
            answers: Cell::new(0),
            answered: RefCell::new(String::new()),
            accent: accent.to_owned(),
            on_open: Box::new(on_open),
        });
        let weak = Rc::downgrade(&sa);
        sa.entry.connect_search_changed(move |_| {
            // The entry also says so a moment after its text is set: the
            // same query again would lose the hit selected.
            if let Some(s) = weak.upgrade()
                && *s.answered.borrow() != s.query()
            {
                s.run();
            }
        });
        // Esc: the entry takes it (stop-search), so it closes the dialog from there.
        let weak = Rc::downgrade(&sa);
        sa.entry.connect_stop_search(move |_| {
            if let Some(s) = weak.upgrade() {
                s.dialog.close();
            }
        });
        let weak = Rc::downgrade(&sa);
        sa.entry.connect_activate(move |_| {
            if let Some(s) = weak.upgrade() {
                s.open_at(s.selected());
            }
        });
        let weak = Rc::downgrade(&sa);
        sa.list.connect_row_activated(move |_, row| {
            if let Some(s) = weak.upgrade() {
                s.open_at(row.index().max(0) as usize);
            }
        });
        // The entry keeps the keyboard; the arrows move the selection.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&sa);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(s) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            match (key.name().as_deref().unwrap_or(""), ctrl) {
                ("Down", _) | ("n" | "j", true) => s.step(1),
                ("Up", _) | ("p" | "k", true) => s.step(-1),
                ("Page_Down", _) => s.step(8),
                ("Page_Up", _) => s.step(-8),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        sa.entry.add_controller(keys);
        sa.summary.set_label("Reading the folder…");
        sa
    }

    /// Open with the last query, and the hit picked last selected again.
    pub fn present(&self, parent: &impl IsA<gtk::Widget>, query: &str, selected: usize) {
        self.dialog.present(Some(parent));
        if !query.is_empty() {
            self.select_next.set(Some(selected));
            self.entry.set_text(query);
        }
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
    }

    /// The folder's text, read off the main thread; searched again with what
    /// is typed.
    pub fn set_corpus(self: &Rc<Self>, corpus: Arc<Vec<Doc>>) {
        *self.corpus.borrow_mut() = Some(corpus);
        if self.select_next.get().is_none() {
            self.select_next.set(Some(self.selected()));
        }
        self.run();
    }

    pub fn query(&self) -> String {
        self.entry.text().to_string()
    }

    pub fn selected(&self) -> usize {
        self.list.selected_row().map_or(0, |r| r.index().max(0) as usize)
    }

    /// The hits shown, as `path:line` (for the checks).
    #[cfg(debug_assertions)]
    pub fn shown(&self) -> Vec<String> {
        let corpus = self.corpus.borrow();
        let Some(c) = corpus.as_ref() else {
            return Vec::new();
        };
        self.found.borrow().hits.iter().map(|h| format!("{}:{}", c[h.doc].path.display(), h.line)).collect()
    }

    fn run(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let Some(corpus) = self.corpus.borrow().clone() else {
            return;
        };
        let query = self.query();
        if search::squash(&query).is_empty() {
            *self.found.borrow_mut() = Found::default();
            self.fill();
            self.summary.set_label(&format!("Type to search {} files", corpus.len()));
            self.answers.set(self.answers.get() + 1);
            *self.answered.borrow_mut() = query;
            return;
        }
        let (tx, rx) = async_channel::bounded(1);
        let asked = query.clone();
        std::thread::spawn(move || {
            let _ = tx.send_blocking(search::search(&corpus, &asked, SHOWN));
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(found) = rx.recv().await else { return };
            let Some(s) = weak.upgrade() else { return };
            if s.generation.get() != generation {
                return;
            }
            *s.found.borrow_mut() = found;
            s.fill();
            s.answers.set(s.answers.get() + 1);
            *s.answered.borrow_mut() = query;
        });
    }

    fn fill(&self) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let found = self.found.borrow();
        let corpus = self.corpus.borrow();
        let Some(corpus) = corpus.as_ref() else {
            return;
        };
        if !search::squash(&self.query()).is_empty() {
            self.summary.set_label(&summary(&found));
        }
        let mut last_doc = usize::MAX;
        for hit in &found.hits {
            let path = &corpus[hit.doc].path;
            let where_ = if hit.doc == last_doc { format!(":{}", hit.line) } else { format!("{}:{}", path.display(), hit.line) };
            last_doc = hit.doc;
            self.list.append(&self.row(hit, &where_));
        }
        let want = self.select_next.take().unwrap_or(0).min(found.hits.len().saturating_sub(1));
        if let Some(row) = self.list.row_at_index(want as i32) {
            self.list.select_row(Some(&row));
            self.keep_in_view(&row);
        }
    }

    fn row(&self, hit: &Hit, where_: &str) -> gtk::ListBoxRow {
        let mut markup = String::new();
        let chars: Vec<char> = hit.shown.chars().collect();
        let mut at = 0;
        for m in &hit.marks {
            markup.push_str(&glib::markup_escape_text(&chars[at..m.start].iter().collect::<String>()));
            let t = glib::markup_escape_text(&chars[m.start..m.end].iter().collect::<String>());
            markup.push_str(&format!("<span foreground=\"{}\" weight=\"bold\">{t}</span>", self.accent));
            at = m.end;
        }
        markup.push_str(&glib::markup_escape_text(&chars[at..].iter().collect::<String>()));
        let text = gtk::Label::builder().use_markup(true).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
        text.set_markup(&markup);
        let place = gtk::Label::builder().label(where_).xalign(1.0).max_width_chars(36).ellipsize(gtk::pango::EllipsizeMode::Start).build();
        place.add_css_class("om-qo-path");
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        b.append(&text);
        b.append(&place);
        gtk::ListBoxRow::builder().child(&b).build()
    }

    fn step(&self, by: i32) {
        let n = self.found.borrow().hits.len() as i32;
        if n == 0 {
            return;
        }
        let to = (self.selected() as i32 + by).clamp(0, n - 1);
        if let Some(row) = self.list.row_at_index(to) {
            self.list.select_row(Some(&row));
            self.keep_in_view(&row);
        }
    }

    fn keep_in_view(&self, row: &gtk::ListBoxRow) {
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

    pub fn open_at(&self, at: usize) {
        let hit = self.found.borrow().hits.get(at).cloned();
        let Some(hit) = hit else { return };
        let Some(path) = self.corpus.borrow().as_ref().map(|c| c[hit.doc].path.clone()) else {
            return;
        };
        let query = self.query();
        // Kept as the hit picked: Ctrl+Shift+F comes back to it.
        if let Some(row) = self.list.row_at_index(at as i32) {
            self.list.select_row(Some(&row));
        }
        self.dialog.close();
        (self.on_open)(&path, &hit, &query);
    }
}

/// "12 lines in 3 files", "200 of 1,340 lines in 37 files", "Nothing found".
fn summary(f: &Found) -> String {
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 { one.to_owned() } else { many.to_owned() }
    };
    match f.lines {
        0 => "Nothing found".into(),
        n if n > f.hits.len() => format!("{} of {} lines in {} {}", f.hits.len(), thousands(n), thousands(f.files), plural(f.files, "file", "files")),
        n => format!("{} {} in {} {}", n, plural(n, "line", "lines"), f.files, plural(f.files, "file", "files")),
    }
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_says_what_is_left_out() {
        let f = Found { hits: Vec::new(), lines: 0, files: 0 };
        assert_eq!(summary(&f), "Nothing found");
        let one = Hit { doc: 0, line: 1, shown: String::new(), marks: Vec::new(), text: String::new(), nth_in_line: 0, nth_in_file: 0 };
        let f = Found { hits: vec![one.clone()], lines: 1, files: 1 };
        assert_eq!(summary(&f), "1 line in 1 file");
        let f = Found { hits: vec![one; 2], lines: 1340, files: 37 };
        assert_eq!(summary(&f), "2 of 1,340 lines in 37 files");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}
