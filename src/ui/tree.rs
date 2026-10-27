//! The folder tree on the right (ADR-0004): slim, in muted text until it has
//! the keyboard, the open file marked with a hairline of accent.
//!
//! The tree keeps its own cursor, so that `j`/`k`/`h`/`l` and the arrows move
//! the same row: GTK does not say which row of a list view has the focus.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::folder::{self, Dir, Scan};

/// A row: a folder, or a file (by its path from the root).
#[derive(Clone, Debug)]
pub enum Node {
    Dir(Rc<Dir>),
    File(PathBuf),
}

pub struct Tree {
    pub widget: gtk::Box,
    pub list: gtk::ListView,
    name: gtk::Label,
    count: gtk::Label,
    selection: gtk::SingleSelection,
    model: RefCell<Option<gtk::TreeListModel>>,
    pub cursor: Cell<u32>,
    /// A file was chosen: Enter or a click on a file row.
    on_open: RefCell<Option<Box<dyn Fn(PathBuf)>>>,
}

fn store_for(dir: &Dir) -> gio::ListStore {
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    for d in &dir.dirs {
        store.append(&glib::BoxedAnyObject::new(Node::Dir(Rc::new(d.clone()))));
    }
    for f in &dir.files {
        store.append(&glib::BoxedAnyObject::new(Node::File(f.clone())));
    }
    store
}

pub fn node_of(item: &glib::Object) -> Option<Node> {
    let row = item.downcast_ref::<gtk::TreeListRow>()?;
    let boxed = row.item().and_downcast::<glib::BoxedAnyObject>()?;
    let node = boxed.borrow::<Node>().clone();
    Some(node)
}

impl Tree {
    pub fn new() -> Rc<Self> {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            let label = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build();
            let expander = gtk::TreeExpander::builder().child(&label).indent_for_icon(true).build();
            item.set_child(Some(&expander));
        });
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() else { return };
            let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() else { return };
            let Some(label) = expander.child().and_downcast::<gtk::Label>() else { return };
            expander.set_list_row(Some(&row));
            match node_of(row.upcast_ref()) {
                Some(Node::Dir(d)) => {
                    label.set_label(&d.name);
                    label.add_css_class("om-dir");
                    expander.set_hide_expander(false);
                }
                Some(Node::File(f)) => {
                    label.set_label(&folder::file_name(&f));
                    label.remove_css_class("om-dir");
                    expander.set_hide_expander(true);
                }
                None => {}
            }
        });
        let selection = gtk::SingleSelection::builder().autoselect(false).can_unselect(true).build();
        let list = gtk::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .single_click_activate(false)
            .vexpand(true)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&list)
            .vexpand(true)
            .build();
        let name = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
        name.add_css_class("om-tree-name");
        let count = gtk::Label::new(None);
        count.add_css_class("om-tree-count");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        head.add_css_class("om-tree-head");
        head.append(&name);
        head.append(&count);
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("om-tree");
        widget.set_margin_top(0);
        widget.append(&head);
        widget.append(&scroller);
        widget.set_width_request(200);

        let tree = Rc::new(Tree {
            widget,
            list,
            name,
            count,
            selection,
            model: RefCell::new(None),
            cursor: Cell::new(0),
            on_open: RefCell::new(None),
        });
        let weak = Rc::downgrade(&tree);
        tree.list.connect_activate(move |_, pos| {
            if let Some(t) = weak.upgrade() {
                t.cursor.set(pos);
                t.activate(pos);
            }
        });
        tree
    }

    pub fn connect_open(&self, f: impl Fn(PathBuf) + 'static) {
        *self.on_open.borrow_mut() = Some(Box::new(f));
    }

    /// Show a new scan, keeping open what was open. Every folder opens the
    /// first time, when the tree is small enough to read whole.
    pub fn show(&self, root_name: &str, scan: &Scan, first: bool) {
        let open: HashSet<PathBuf> = if first { HashSet::new() } else { self.expanded() };
        let store = store_for(&scan.tree);
        let model = gtk::TreeListModel::new(store, false, false, |item| {
            let boxed = item.downcast_ref::<glib::BoxedAnyObject>()?;
            match &*boxed.borrow::<Node>() {
                Node::Dir(d) => Some(store_for(d).upcast()),
                Node::File(_) => None,
            }
        });
        self.selection.set_model(Some(&model));
        *self.model.borrow_mut() = Some(model);
        let open_all = first && scan.order.len() <= 200;
        let mut i = 0;
        while let Some(row) = self.row(i) {
            if let Some(Node::Dir(d)) = node_of(row.upcast_ref())
                && (open_all || open.contains(&d.rel))
            {
                row.set_expanded(true);
            }
            i += 1;
        }
        self.name.set_label(root_name);
        let n = scan.order.len();
        let more = if scan.truncated { "+" } else { "" };
        self.count.set_label(&match n {
            0 => "0 files".into(),
            1 => "1 file".into(),
            _ => format!("{n}{more} files"),
        });
    }

    fn expanded(&self) -> HashSet<PathBuf> {
        let mut out = HashSet::new();
        let mut i = 0;
        while let Some(row) = self.row(i) {
            if row.is_expanded()
                && let Some(Node::Dir(d)) = node_of(row.upcast_ref())
            {
                out.insert(d.rel.clone());
            }
            i += 1;
        }
        out
    }

    pub fn row(&self, pos: u32) -> Option<gtk::TreeListRow> {
        self.model.borrow().as_ref()?.row(pos)
    }

    fn len(&self) -> u32 {
        self.model.borrow().as_ref().map_or(0, |m| m.n_items())
    }

    /// Mark `file` as the one open, opening the folders above it.
    pub fn mark(&self, file: Option<&Path>) {
        let Some(file) = file else {
            self.selection.set_selected(gtk::INVALID_LIST_POSITION);
            return;
        };
        let mut i = 0;
        while let Some(row) = self.row(i) {
            match node_of(row.upcast_ref()) {
                Some(Node::Dir(d)) if file.starts_with(&d.rel) && !row.is_expanded() => row.set_expanded(true),
                Some(Node::File(f)) if f == file => {
                    self.selection.set_selected(i);
                    self.cursor.set(i);
                    self.list.scroll_to(i, gtk::ListScrollFlags::NONE, None);
                    return;
                }
                _ => {}
            }
            i += 1;
        }
        self.selection.set_selected(gtk::INVALID_LIST_POSITION);
    }

    /// Give the tree the keyboard, on the open file.
    pub fn focus(&self) {
        let pos = match self.selection.selected() {
            gtk::INVALID_LIST_POSITION => self.cursor.get().min(self.len().saturating_sub(1)),
            p => p,
        };
        self.move_to(pos);
    }

    pub fn has_focus(&self) -> bool {
        self.widget
            .root()
            .and_then(|r| r.focus())
            .is_some_and(|f| f.is_ancestor(&self.widget) || f == *self.widget.upcast_ref::<gtk::Widget>())
    }

    fn move_to(&self, pos: u32) {
        let n = self.len();
        if n == 0 {
            return;
        }
        let pos = pos.min(n - 1);
        self.cursor.set(pos);
        if !self.has_focus() {
            self.list.grab_focus();
        }
        self.list.scroll_to(pos, gtk::ListScrollFlags::FOCUS, None);
    }

    /// A key while the tree has the keyboard. True if it was the tree's.
    pub fn key(&self, name: &str) -> bool {
        let at = self.cursor.get();
        match name {
            "j" | "Down" => self.move_to(at + 1),
            "k" | "Up" => self.move_to(at.saturating_sub(1)),
            "Home" | "g" => self.move_to(0),
            "End" | "G" => self.move_to(self.len().saturating_sub(1)),
            "l" | "Right" => {
                if let Some(row) = self.row(at) {
                    if row.is_expandable() && !row.is_expanded() {
                        row.set_expanded(true);
                    } else if row.is_expandable() {
                        self.move_to(at + 1);
                    }
                }
            }
            "h" | "Left" => {
                if let Some(row) = self.row(at) {
                    if row.is_expandable() && row.is_expanded() {
                        row.set_expanded(false);
                        self.move_to(at);
                    } else if let Some(parent) = row.parent() {
                        let mut i = at;
                        while i > 0 {
                            i -= 1;
                            if self.row(i).is_some_and(|r| r == parent) {
                                self.move_to(i);
                                break;
                            }
                        }
                    }
                }
            }
            "Return" | "KP_Enter" | "space" => self.activate(at),
            _ => return false,
        }
        true
    }

    fn activate(&self, pos: u32) {
        let Some(row) = self.row(pos) else { return };
        match node_of(row.upcast_ref()) {
            Some(Node::Dir(_)) => {
                row.set_expanded(!row.is_expanded());
                self.move_to(pos);
            }
            Some(Node::File(f)) => {
                if let Some(cb) = self.on_open.borrow().as_ref() {
                    cb(f);
                }
            }
            None => {}
        }
    }
}
