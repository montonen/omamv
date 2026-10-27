//! The start screen: no folder open. A short lead, the recent folders with a
//! number key each, and the ways in.

use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;

use crate::{folder, recent};

pub struct Start {
    pub widget: gtk::Box,
    pub list: gtk::ListBox,
    recent: std::cell::RefCell<Vec<PathBuf>>,
    on_open: Box<dyn Fn(PathBuf)>,
}

impl Start {
    pub fn new(on_open: impl Fn(PathBuf) + 'static) -> Rc<Self> {
        let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
        col.set_width_request(520);
        col.set_halign(gtk::Align::Center);
        col.set_valign(gtk::Align::Center);
        let mark = gtk::Label::builder().label("OMAMV").xalign(0.0).build();
        mark.add_css_class("om-start-mark");
        let lead = gtk::Label::builder().label("Open a folder to read it.").xalign(0.0).margin_top(8).margin_bottom(34).build();
        lead.add_css_class("om-start-lead");
        let rtitle = gtk::Label::builder().label("RECENT").xalign(0.0).margin_bottom(6).build();
        rtitle.add_css_class("om-recent-title");
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Browse).build();
        list.add_css_class("om-recent");

        let hints = gtk::Box::new(gtk::Orientation::Vertical, 12);
        hints.set_margin_top(34);
        let hint = |keys: gtk::Box, text: &str| {
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            b.append(&keys);
            let l = gtk::Label::new(Some(text));
            l.add_css_class("om-hint");
            b.append(&l);
            b
        };
        // The way in that a mouse can use too: the window's own action.
        let choose = gtk::Button::builder().action_name("win.open-folder").halign(gtk::Align::Start).build();
        choose.add_css_class("flat");
        choose.add_css_class("om-choose");
        choose.set_child(Some(&hint(super::keys("Ctrl+O"), "choose a folder")));
        hints.append(&choose);
        let jk = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        for k in ["j", "k", "Enter"] {
            jk.append(&super::kbd(k));
        }
        hints.append(&hint(jk, "pick one above"));
        let term = gtk::Label::new(Some("$ omamv ."));
        term.add_css_class("om-term");
        let tb = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tb.append(&term);
        hints.append(&hint(tb, "in a terminal, like code ."));
        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        foot.set_margin_top(48);
        foot.append(&super::kbd("?"));
        let fl = gtk::Label::new(Some("keys"));
        fl.add_css_class("om-foot");
        foot.append(&fl);

        col.append(&mark);
        col.append(&lead);
        col.append(&rtitle);
        col.append(&list);
        col.append(&hints);
        col.append(&foot);
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_hexpand(true);
        widget.set_vexpand(true);
        widget.set_valign(gtk::Align::Center);
        widget.append(&col);

        let start = Rc::new(Start { widget, list, recent: Default::default(), on_open: Box::new(on_open) });
        let weak = Rc::downgrade(&start);
        start.list.connect_row_activated(move |_, row| {
            if let Some(s) = weak.upgrade() {
                s.open(row.index() as usize);
            }
        });
        start
    }

    /// Fill the list from the recent folders, the first one selected.
    pub fn refresh(&self) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let list = recent::load();
        if list.is_empty() {
            let none = gtk::Label::builder().label("No folders yet").xalign(0.0).margin_start(10).build();
            none.add_css_class("om-hint");
            self.list.append(&gtk::ListBoxRow::builder().child(&none).selectable(false).activatable(false).build());
        }
        for (i, r) in list.iter().enumerate() {
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 14);
            b.append(&super::kbd(&(i + 1).to_string()));
            let path = gtk::Label::builder()
                .label(folder::tilde(&r.path))
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build();
            path.add_css_class("om-rpath");
            let when = gtk::Label::new(Some(&recent::when(r.when)));
            when.add_css_class("om-rwhen");
            b.append(&path);
            b.append(&when);
            self.list.append(&gtk::ListBoxRow::builder().child(&b).build());
        }
        *self.recent.borrow_mut() = list.into_iter().map(|r| r.path).collect();
        if let Some(first) = self.list.row_at_index(0) {
            self.list.select_row(Some(&first));
        }
    }

    pub fn focus(&self) {
        if let Some(row) = self.list.selected_row() {
            row.grab_focus();
        }
    }

    pub fn open(&self, at: usize) {
        if let Some(p) = self.recent.borrow().get(at).cloned() {
            (self.on_open)(p);
        }
    }

    /// A key on the start screen. True if it was the screen's.
    pub fn key(&self, name: &str) -> bool {
        let at = self.list.selected_row().map_or(0, |r| r.index());
        let n = self.recent.borrow().len() as i32;
        let go = |i: i32| {
            if let Some(row) = self.list.row_at_index(i.clamp(0, (n - 1).max(0))) {
                self.list.select_row(Some(&row));
                row.grab_focus();
            }
        };
        match name {
            "j" | "Down" => go(at + 1),
            "k" | "Up" => go(at - 1),
            "Return" | "KP_Enter" => self.open(at.max(0) as usize),
            d if d.len() == 1 && ("1"..="9").contains(&d) => self.open((d.as_bytes()[0] - b'1') as usize),
            _ => return false,
        }
        true
    }
}
