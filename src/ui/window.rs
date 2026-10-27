//! A window: one folder, its tree on the right, one file at a time on the page.
//!
//! The chrome is the Quiet page design (ADR-0011): a flat header carrying the
//! file and its folder, a hairline under it only while the page is scrolled,
//! a slim tree, a find bar that slides in, a banner for a removed file. State
//! lives in the subtitle and goes away by itself.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use webkit6::prelude::*;
use webkit6::{NavigationPolicyDecision, PolicyDecisionType, WebView};

use super::quickopen::QuickOpen;
use super::start::Start;
use super::status::{Mode, Status};
use super::tree::Tree;
use crate::folder::{self, Scan};
use crate::scheme;

static NEXT_HOST: AtomicU32 = AtomicU32::new(1);

/// `OMAMV_DEBUG=1`: each step of opening a folder and a page, on stderr.
macro_rules! trace {
    ($($arg:tt)*) => {
        if std::env::var_os("OMAMV_DEBUG").is_some() {
            eprintln!("omamv: {}", format!($($arg)*));
        }
    };
}

/// Where a reader was in a file: the heading above the top of the window and
/// how far below it, or a plain position.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Place {
    pub id: String,
    pub off: i64,
    pub y: i64,
}

impl Place {
    fn parse(json: &str) -> Place {
        // {"id":"x","off":3,"y":120}; ids are GitHub slugs, so no quotes inside.
        let field = |name: &str| -> Option<String> {
            let at = json.find(&format!("\"{name}\":"))? + name.len() + 3;
            let rest = &json[at..];
            if let Some(s) = rest.strip_prefix('"') {
                return s.find('"').map(|e| s[..e].to_owned());
            }
            let end = rest.find([',', '}']).unwrap_or(rest.len());
            Some(rest[..end].trim().to_owned())
        };
        Place {
            id: field("id").unwrap_or_default(),
            off: field("off").and_then(|v| v.parse().ok()).unwrap_or(0),
            y: field("y").and_then(|v| v.parse().ok()).unwrap_or(0),
        }
    }

    fn query(&self) -> String {
        let id = glib::Uri::escape_string(&self.id, None, false);
        format!("?a={id}&o={}&y={}", self.off, self.y)
    }
}

#[derive(Clone, Debug)]
struct Visit {
    file: PathBuf,
    place: Place,
}

pub struct Win {
    pub window: adw::ApplicationWindow,
    pub host: String,
    root: RefCell<Option<PathBuf>>,
    pub scan: RefCell<Scan>,
    pub current: RefCell<Option<PathBuf>>,
    history: RefCell<Vec<Visit>>,
    hist_at: Cell<usize>,
    pub web: WebView,
    pub header: adw::HeaderBar,
    /// The address omamv itself asked the page to load; any other is a link.
    expect: RefCell<String>,
    title: gtk::Label,
    subtitle: gtk::Label,
    pub changed: gtk::Label,
    gone_label: gtk::Label,
    pub banner: adw::Banner,
    pub status: Status,
    pub split: adw::OverlaySplitView,
    pub tree: Rc<Tree>,
    stack: gtk::Stack,
    empty_page: adw::StatusPage,
    pub start: Rc<Start>,
    find_button: gtk::Button,
    tree_button: gtk::ToggleButton,
    pub findbar: gtk::SearchBar,
    pub find_entry: gtk::SearchEntry,
    pub find_count: gtk::Label,
    last_find: RefCell<String>,
    last_find_at: Cell<i64>,
    watcher: RefCell<Option<notify::RecommendedWatcher>>,
    pub gone: Cell<bool>,
    pending_g: Cell<bool>,
    reload_timer: RefCell<Option<glib::SourceId>>,
    rescan_timer: RefCell<Option<glib::SourceId>>,
    changed_timer: RefCell<Option<glib::SourceId>>,
    zoom: Cell<f64>,
    pub ready: Cell<bool>,
    pub loads: Cell<u32>,
    /// What the page said last (debug and the self-check read it).
    pub last_message: RefCell<String>,
    pub quick: RefCell<Option<Rc<QuickOpen>>>,
    me: RefCell<std::rc::Weak<Win>>,
}

impl Win {
    pub fn new(app: &adw::Application) -> Rc<Win> {
        let host = format!("w{}", NEXT_HOST.fetch_add(1, Ordering::Relaxed));

        // The page.
        let ucm = webkit6::UserContentManager::new();
        ucm.register_script_message_handler("omamv", None);
        let web = WebView::builder().user_content_manager(&ucm).build();
        web.set_hexpand(true);
        web.set_vexpand(true);
        if let Some(settings) = WebViewExt::settings(&web) {
            settings.set_enable_developer_extras(cfg!(debug_assertions));
            settings.set_javascript_can_access_clipboard(false);
            settings.set_javascript_can_open_windows_automatically(false);
            settings.set_enable_back_forward_navigation_gestures(false);
            settings.set_default_font_family("Adwaita Sans");
            settings.set_monospace_font_family("JetBrainsMono Nerd Font");
            settings.set_enable_write_console_messages_to_stdout(std::env::var_os("OMAMV_CONSOLE").is_some());
        }
        paint_background(&web, &crate::scheme::palette());

        // Header: the file, its folder, and the quiet signs of state.
        let title = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::Middle).build();
        title.add_css_class("om-title");
        let subtitle = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::Start).build();
        subtitle.add_css_class("om-subtitle");
        let changed = gtk::Label::builder().use_markup(true).visible(false).build();
        changed.add_css_class("om-changed");
        let gone_label = gtk::Label::builder().label("gone").visible(false).build();
        gone_label.add_css_class("om-gone");
        let sub_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        sub_row.set_halign(gtk::Align::Center);
        sub_row.append(&subtitle);
        sub_row.append(&changed);
        sub_row.append(&gone_label);
        let title_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        title_box.set_valign(gtk::Align::Center);
        title_box.append(&title);
        title_box.append(&sub_row);

        let header = adw::HeaderBar::builder().title_widget(&title_box).show_start_title_buttons(false).show_end_title_buttons(false).build();
        header.add_css_class("om-header");
        let menu = gio::Menu::new();
        menu.append(Some("Open Folder…"), Some("win.open-folder"));
        menu.append(Some("Recent Folders"), Some("win.recent"));
        let more = gio::Menu::new();
        more.append(Some("Settings"), Some("win.settings"));
        more.append(Some("Keys"), Some("win.keys"));
        more.append(Some("About omamv"), Some("win.about"));
        menu.append_section(None, &more);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text("Menu  F10")
            .focusable(false)
            .build();
        menu_button.add_css_class("flat");
        let tree_button = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-right-symbolic")
            .tooltip_text("Folder tree  Ctrl+B")
            .focusable(false)
            .active(true)
            .build();
        tree_button.add_css_class("flat");
        let find_button = gtk::Button::builder()
            .icon_name("system-search-symbolic")
            .tooltip_text("Find  Ctrl+F")
            .focusable(false)
            .build();
        find_button.add_css_class("flat");
        header.pack_end(&menu_button);
        header.pack_end(&tree_button);
        header.pack_end(&find_button);

        // Find bar.
        let find_entry = gtk::SearchEntry::builder().placeholder_text("Find in the page").build();
        find_entry.add_css_class("om-find-entry");
        let find_count = gtk::Label::new(None);
        find_count.add_css_class("om-find-count");
        let find_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        find_box.append(&find_entry);
        find_box.append(&find_count);
        let findbar = gtk::SearchBar::builder().child(&find_box).show_close_button(true).build();
        findbar.add_css_class("om-find");
        findbar.connect_entry(&find_entry);

        let banner = adw::Banner::builder().button_label("Another file").revealed(false).build();
        banner.add_css_class("om-banner");

        let tree = Tree::new();
        let split = adw::OverlaySplitView::builder()
            .sidebar(&tree.widget)
            .content(&web)
            .sidebar_position(gtk::PackType::End)
            .max_sidebar_width(248.0)
            .min_sidebar_width(200.0)
            .sidebar_width_fraction(0.22)
            .show_sidebar(true)
            .build();
        tree_button.bind_property("active", &split, "show-sidebar").bidirectional().sync_create().build();

        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::None).build();

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&findbar);
        toolbar.add_top_bar(&banner);
        toolbar.set_top_bar_style(adw::ToolbarStyle::Flat);
        toolbar.set_content(Some(&stack));
        let status = Status::new();
        toolbar.add_bottom_bar(&status.widget);
        toolbar.set_bottom_bar_style(adw::ToolbarStyle::Flat);

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .default_width(1280)
            .default_height(800)
            .content(&toolbar)
            .title("omamv")
            .build();

        let win_holder: Rc<RefCell<std::rc::Weak<Win>>> = Rc::default();
        let holder = win_holder.clone();
        let start = Start::new(move |path| {
            if let Some(w) = holder.borrow().upgrade() {
                w.open_folder(&path, None);
            }
        });
        stack.add_named(&start.widget, Some("start"));
        stack.add_named(&split, Some("doc"));
        // ADR-0012: the window, not the page, says what the folder holds.
        let reading_page = adw::StatusPage::builder().title("Reading the folder…").build();
        stack.add_named(&reading_page, Some("reading"));
        let other = gtk::Button::builder()
            .label("Open another folder…")
            .action_name("win.open-folder")
            .halign(gtk::Align::Center)
            .build();
        other.add_css_class("pill");
        let empty_page = adw::StatusPage::builder()
            .title("No Markdown or text files here")
            .description("omamv shows .md and .txt files, in this folder and the folders below it. Hidden folders and anything .gitignore leaves out are skipped.")
            .child(&other)
            .build();
        stack.add_named(&empty_page, Some("empty"));

        let win = Rc::new(Win {
            window,
            host,
            root: RefCell::new(None),
            scan: RefCell::new(Scan::default()),
            current: RefCell::new(None),
            history: RefCell::new(Vec::new()),
            hist_at: Cell::new(0),
            web,
            header,
            expect: RefCell::new(String::new()),
            title,
            subtitle,
            changed,
            gone_label,
            banner,
            status,
            split,
            tree,
            stack,
            empty_page,
            start,
            find_button,
            tree_button,
            findbar,
            find_entry,
            find_count,
            last_find: RefCell::new(String::new()),
            last_find_at: Cell::new(0),
            watcher: RefCell::new(None),
            gone: Cell::new(false),
            pending_g: Cell::new(false),
            reload_timer: RefCell::new(None),
            rescan_timer: RefCell::new(None),
            changed_timer: RefCell::new(None),
            zoom: Cell::new(1.0),
            ready: Cell::new(false),
            loads: Cell::new(0),
            last_message: RefCell::new(String::new()),
            quick: RefCell::new(None),
            me: RefCell::new(std::rc::Weak::new()),
        });
        *win.me.borrow_mut() = Rc::downgrade(&win);
        *win_holder.borrow_mut() = Rc::downgrade(&win);
        super::remember(&win);
        win.wire(&ucm, &menu_button);
        win.apply_settings();
        win.show_start();
        win
    }

    /// Which screen the window shows: start, reading, empty or doc.
    pub fn showing(&self) -> String {
        self.stack.visible_child_name().map(|s| s.to_string()).unwrap_or_default()
    }

    pub fn root(&self) -> Option<PathBuf> {
        self.root.borrow().clone()
    }

    fn weak(&self) -> std::rc::Weak<Win> {
        self.me.borrow().clone()
    }

    fn wire(&self, ucm: &webkit6::UserContentManager, _menu: &gtk::MenuButton) {
        // Messages from the page.
        let weak = self.weak();
        ucm.connect_script_message_received(Some("omamv"), move |_, value| {
            let Some(w) = weak.upgrade() else { return };
            let msg = value.to_str().to_string();
            if msg.contains("\"scrolled\":true") {
                w.header.add_css_class("om-scrolled");
            } else if msg.contains("\"scrolled\":false") {
                w.header.remove_css_class("om-scrolled");
            }
            trace!("page says {msg}");
            if msg.contains("\"ready\":true") {
                w.ready.set(true);
                if w.gone.get() {
                    w.js("omamv.gone(true)");
                }
            }
            *w.last_message.borrow_mut() = msg;
        });

        let weak = self.weak();
        self.web.connect_load_changed(move |_, ev| {
            trace!("load event {ev:?}");
            if ev == webkit6::LoadEvent::Started
                && let Some(w) = weak.upgrade()
            {
                w.ready.set(false);
            }
            if ev == webkit6::LoadEvent::Finished
                && let Some(w) = weak.upgrade()
            {
                w.loads.set(w.loads.get() + 1);
            }
        });

        // Links: in the folder they open here, on the web in the browser.
        let weak = self.weak();
        self.web.connect_decide_policy(move |_, decision, kind| {
            let Some(w) = weak.upgrade() else { return false };
            match kind {
                PolicyDecisionType::NavigationAction => {}
                PolicyDecisionType::NewWindowAction => {
                    decision.ignore();
                    return true;
                }
                _ => return false,
            }
            let Some(nav) = decision.downcast_ref::<NavigationPolicyDecision>() else { return false };
            let Some(action) = nav.navigation_action() else { return false };
            let uri = action.request().and_then(|r| r.uri()).map(|u| u.to_string()).unwrap_or_default();
            trace!("navigation to {uri} ({:?})", action.navigation_type());
            // What omamv loaded itself, a reload, and a jump within the page
            // go ahead; anything else is a link, and omamv follows it.
            let base = |u: &str| u.split('#').next().unwrap_or("").to_owned();
            let same_page = uri.contains('#') && w.web.uri().is_some_and(|now| base(&now) == base(&uri));
            let ours = |u: &str| scheme::parse(u).map(|(h, r, _)| (h, r));
            let expected = ours(&w.expect.borrow()).is_some_and(|e| Some(e) == ours(&uri));
            if expected || uri.starts_with("about:") || same_page
                || action.navigation_type() == webkit6::NavigationType::Reload
            {
                return false;
            }
            trace!("  treated as a link");
            decision.ignore();
            w.follow(&uri);
            true
        });
        // A viewer has no verbs to offer; Ctrl+C copies.
        self.web.connect_context_menu(|_, _, _| true);
        let weak = self.weak();
        self.web.connect_web_process_terminated(move |_, why| {
            trace!("web process ended: {why:?}");
            if let Some(w) = weak.upgrade() {
                w.reload_keeping_place();
            }
        });

        // Tree.
        let weak = self.weak();
        self.tree.connect_open(move |file| {
            if let Some(w) = weak.upgrade() {
                w.open_file(&file, None, None, true);
                w.web.grab_focus();
            }
        });

        // Header buttons.
        let weak = self.weak();
        self.find_button.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.open_find();
            }
        });

        // Find.
        let weak = self.weak();
        self.find_entry.connect_search_changed(move |e| {
            if let Some(w) = weak.upgrade() {
                let q = e.text().to_string();
                // Closing the bar empties the entry; n and N still want the words.
                if q.is_empty() && !w.findbar.is_search_mode() {
                    return;
                }
                *w.last_find.borrow_mut() = q.clone();
                w.find_js(&format!("omamv.find({})", js_string(&q)));
            }
        });
        let weak = self.weak();
        self.find_entry.connect_activate(move |_| {
            if let Some(w) = weak.upgrade() {
                w.find_js("omamv.step(1)");
            }
        });
        let shift_enter = gtk::EventControllerKey::new();
        let weak = self.weak();
        shift_enter.connect_key_pressed(move |_, key, _, mods| {
            let Some(w) = weak.upgrade() else { return glib::Propagation::Proceed };
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) && mods.contains(gdk::ModifierType::SHIFT_MASK) {
                w.find_js("omamv.step(-1)");
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.find_entry.add_controller(shift_enter);
        let weak = self.weak();
        self.find_entry.connect_stop_search(move |_| {
            if let Some(w) = weak.upgrade() {
                w.close_find();
            }
        });
        let weak = self.weak();
        self.findbar.connect_search_mode_enabled_notify(move |bar| {
            if !bar.is_search_mode()
                && let Some(w) = weak.upgrade()
            {
                w.js("omamv.endFind()");
                w.web.grab_focus();
            }
        });

        // A removed file: another one.
        let weak = self.weak();
        self.banner.connect_button_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.quick_open();
            }
        });

        // Actions for the menu.
        let actions: [(&str, fn(&Win)); 5] = [
            ("open-folder", Win::choose_folder),
            ("recent", Win::show_recent),
            ("settings", Win::show_settings),
            ("keys", Win::show_keys),
            ("about", Win::show_about),
        ];
        for (name, f) in actions {
            let action = gio::SimpleAction::new(name, None);
            let weak = self.weak();
            action.connect_activate(move |_, _| {
                if let Some(w) = weak.upgrade() {
                    f(&w);
                }
            });
            self.window.add_action(&action);
        }

        // Keys: the window sees them before the page does.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.weak();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(w) = weak.upgrade() else { return glib::Propagation::Proceed };
            if w.key(key, mods) { glib::Propagation::Stop } else { glib::Propagation::Proceed }
        });
        self.window.add_controller(keys);

        // The status bar follows the keyboard.
        let weak = self.weak();
        self.window.connect_notify_local(Some("focus-widget"), move |_, _| {
            if let Some(w) = weak.upgrade() {
                w.update_status();
            }
        });
        let weak = self.weak();
        self.window.connect_notify_local(Some("visible-dialog"), move |_, _| {
            if let Some(w) = weak.upgrade() {
                w.update_status();
            }
        });
        let weak = self.weak();
        self.stack.connect_visible_child_name_notify(move |_| {
            if let Some(w) = weak.upgrade() {
                w.update_status();
            }
        });
        let weak = self.weak();
        self.findbar.connect_search_mode_enabled_notify(move |_| {
            if let Some(w) = weak.upgrade() {
                w.update_status();
            }
        });

        let weak = self.weak();
        self.window.connect_close_request(move |_| {
            if let Some(w) = weak.upgrade() {
                scheme::set_root(&w.host, None);
                w.watcher.borrow_mut().take();
                super::forget(&w);
            }
            glib::Propagation::Proceed
        });
    }

    // ---------------------------------------------------------------- keys

    /// Every key the window answers. True when it was handled.
    pub fn key(&self, key: gdk::Key, mods: gdk::ModifierType) -> bool {
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = mods.contains(gdk::ModifierType::ALT_MASK);
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        let name = key.name().map(|n| n.to_string()).unwrap_or_default();
        // A dialog has the keyboard: it answers its own keys.
        if self.window.visible_dialog().is_some() {
            return false;
        }
        let typing = gtk::prelude::GtkWindowExt::focus(&self.window)
            .is_some_and(|f| f.is::<gtk::Text>() || f.ancestor(gtk::Text::static_type()).is_some() || f.is::<gtk::SearchEntry>());

        // Everywhere.
        match (ctrl, alt, name.as_str()) {
            (true, _, "q") => {
                self.window.application().inspect(|a| a.quit());
                return true;
            }
            (true, _, "w") => {
                self.window.close();
                return true;
            }
            (true, _, "o") if !shift => {
                self.choose_folder();
                return true;
            }
            (true, _, "O") | (true, _, "o") => {
                self.show_recent();
                return true;
            }
            _ => {}
        }

        if matches!(self.stack.visible_child_name().as_deref(), Some("reading" | "empty")) {
            if name == "question" && !typing && !ctrl {
                self.show_keys();
                return true;
            }
            return false;
        }
        if self.stack.visible_child_name().as_deref() == Some("start") {
            if typing || ctrl || alt {
                return false;
            }
            if name == "question" {
                self.show_keys();
                return true;
            }
            if name == "Escape" && self.root.borrow().is_some() {
                let empty = self.scan.borrow().order.is_empty();
                self.stack.set_visible_child_name(if empty { "empty" } else { "doc" });
                self.set_chrome(!empty);
                self.web.grab_focus();
                return true;
            }
            return self.start.key(&name);
        }

        match (ctrl, alt, name.as_str()) {
            (true, _, "p") => {
                self.quick_open();
                return true;
            }
            (true, _, "comma") => {
                self.show_settings();
                return true;
            }
            (true, _, "f") => {
                self.open_find();
                return true;
            }
            (true, _, "b") => {
                self.toggle_tree();
                return true;
            }
            (true, _, "plus" | "equal" | "KP_Add") => {
                self.set_zoom(self.zoom.get() + 0.1);
                return true;
            }
            (true, _, "minus" | "KP_Subtract") => {
                self.set_zoom(self.zoom.get() - 0.1);
                return true;
            }
            (true, _, "0" | "KP_0") => {
                self.set_zoom(crate::settings::get().zoom());
                return true;
            }
            (false, true, "Left") => {
                self.go_back(-1);
                return true;
            }
            (false, true, "Right") => {
                self.go_back(1);
                return true;
            }
            _ => {}
        }
        if typing || ctrl || alt {
            return false;
        }

        if self.tree.has_focus() {
            if name == "Escape" {
                self.web.grab_focus();
                return true;
            }
            if self.tree.key(&name) {
                return true;
            }
        }

        let g = self.pending_g.replace(false);
        match name.as_str() {
            "j" => self.js("omamv.scroll('down')"),
            "k" => self.js("omamv.scroll('up')"),
            "d" => self.js("omamv.scroll('half-down')"),
            "u" => self.js("omamv.scroll('half-up')"),
            "G" => self.js("omamv.scroll('bottom')"),
            "g" if g => self.js("omamv.scroll('top')"),
            "g" => {
                self.pending_g.set(true);
                let weak = self.weak();
                glib::timeout_add_local_once(Duration::from_millis(700), move || {
                    if let Some(w) = weak.upgrade() {
                        w.pending_g.set(false);
                    }
                });
            }
            "bracketright" | "J" => self.step_file(1),
            "bracketleft" | "K" => self.step_file(-1),
            "slash" => self.open_find(),
            "n" => self.find_again(1),
            "N" => self.find_again(-1),
            "question" => self.show_keys(),
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------ folders

    fn show_start(&self) {
        self.start.refresh();
        self.stack.set_visible_child_name("start");
        self.set_chrome(false);
        self.title.set_label("omamv");
        self.subtitle.set_label(if self.root.borrow().is_some() { "recent folders" } else { "no folder open" });
        self.changed.set_visible(false);
        self.gone_label.set_visible(false);
        self.window.set_title(Some("omamv"));
        self.start.focus();
    }

    fn set_chrome(&self, doc: bool) {
        self.find_button.set_visible(doc);
        self.tree_button.set_visible(doc);
        if !doc {
            self.header.remove_css_class("om-scrolled");
            self.banner.set_revealed(false);
        } else if self.gone.get() {
            self.banner.set_revealed(true);
        }
    }

    fn show_recent(&self) {
        self.show_start();
    }

    fn choose_folder(&self) {
        let dialog = gtk::FileDialog::builder().title("Open a folder").modal(true).build();
        if let Some(root) = self.root.borrow().as_ref() {
            dialog.set_initial_folder(Some(&gio::File::for_path(root)));
        }
        let weak = self.weak();
        dialog.select_folder(Some(&self.window), gio::Cancellable::NONE, move |res| {
            let Some(w) = weak.upgrade() else { return };
            match res {
                Ok(dir) => match dir.path() {
                    Some(path) => w.open_folder(&path, None),
                    None => w.say("That folder is not on this computer's disk"),
                },
                Err(e) if e.matches(gtk::DialogError::Dismissed) || e.matches(gtk::DialogError::Cancelled) => {}
                Err(e) => {
                    eprintln!("omamv: the folder picker failed: {e}");
                    w.say(&format!("The folder picker failed: {e}"));
                }
            }
        });
    }

    /// Show `root`, with `file` (a path from the root) or the first file.
    pub fn open_folder(&self, root: &Path, file: Option<PathBuf>) {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let same = self.root.borrow().as_deref() == Some(root.as_path());
        if same {
            self.stack.set_visible_child_name("doc");
            self.set_chrome(true);
            if let Some(f) = file {
                self.open_file(&f, None, None, true);
            }
            return;
        }
        trace!("open folder {}", root.display());
        *self.root.borrow_mut() = Some(root.clone());
        scheme::set_root(&self.host, Some(root.clone()));
        crate::recent::touch(&root);
        self.history.borrow_mut().clear();
        self.hist_at.set(0);
        *self.current.borrow_mut() = None;
        self.gone.set(false);
        self.banner.set_revealed(false);
        self.stack.set_visible_child_name("reading");
        self.set_chrome(false);
        self.title.set_label(&folder::file_name(&root));
        self.subtitle.set_label(&folder::tilde(&root));
        self.web.load_html(&crate::render::wrap("", "", &scheme::palette()), None);

        // Follow the disk. Watching every folder below a big one takes a
        // second or more, so it is set up off the main thread.
        *self.watcher.borrow_mut() = None;
        let (tx, rx) = async_channel::bounded(1);
        let watch_root = root.clone();
        std::thread::spawn(move || {
            let _ = tx.send_blocking(crate::watch::start(&watch_root));
        });
        let weak = self.weak();
        let watched = root.clone();
        glib::spawn_future_local(async move {
            let Ok(Some((watcher, changes))) = rx.recv().await else {
                trace!("could not watch the folder");
                return;
            };
            trace!("watching the folder");
            {
                let Some(w) = weak.upgrade() else { return };
                if w.root.borrow().as_deref() != Some(watched.as_path()) {
                    return;
                }
                *w.watcher.borrow_mut() = Some(watcher);
            }
            while let Ok(change) = changes.recv().await {
                let Some(w) = weak.upgrade() else { break };
                if w.root.borrow().as_deref() != Some(watched.as_path()) {
                    break;
                }
                w.on_change(&change.path);
            }
        });
        self.rescan(true, file);
    }

    /// Walk the folder again, off the main thread.
    fn rescan(&self, first: bool, open: Option<PathBuf>) {
        let Some(root) = self.root() else { return };
        let (tx, rx) = async_channel::bounded(1);
        let walk_root = root.clone();
        std::thread::spawn(move || {
            let s = crate::settings::get();
            let skip = folder::Skip { hidden: !s.hidden, gitignored: s.gitignore };
            let _ = tx.send_blocking(folder::scan(&walk_root, skip));
        });
        let weak = self.weak();
        glib::spawn_future_local(async move {
            let Ok(scan) = rx.recv().await else { return };
            trace!("scan done: {} files", scan.order.len());
            let Some(w) = weak.upgrade() else { return };
            if w.root.borrow().as_deref() != Some(root.as_path()) {
                trace!("scan is for another folder; dropped");
                return;
            }
            w.tree.show(&folder::file_name(&root), &scan, first);
            trace!("tree shown");
            let empty = scan.order.is_empty();
            let showing = w.stack.visible_child_name();
            if matches!(showing.as_deref(), Some("reading" | "empty" | "doc")) {
                w.stack.set_visible_child_name(if empty { "empty" } else { "doc" });
                w.set_chrome(!empty);
            }
            let nothing_open = w.current.borrow().is_none();
            let pick = open
                .filter(|f| scan.order.contains(f))
                .or_else(|| if first || nothing_open { folder::first_file(&scan) } else { None });
            trace!("pick {pick:?}");
            *w.scan.borrow_mut() = scan;
            if let Some(f) = pick {
                w.open_file(&f, None, None, true);
                w.web.grab_focus();
            } else if empty {
                w.empty_folder();
            } else {
                let cur = w.current.borrow().clone();
                w.tree.mark(cur.as_deref());
            }
        });
    }

    fn empty_folder(&self) {
        trace!("nothing to show");
        self.title.set_label(&self.root().map(|r| folder::file_name(&r)).unwrap_or_default());
        *self.current.borrow_mut() = None;
        self.window.set_title(Some("omamv"));
        self.empty_page.grab_focus();
    }

    // -------------------------------------------------------------- files

    /// Open `file` (a path from the root). `place` puts the reader back where
    /// they were; `fragment` is a heading to go to.
    pub fn open_file(&self, file: &Path, fragment: Option<&str>, place: Option<Place>, record: bool) {
        let Some(root) = self.root() else { return };
        if record {
            self.remember_place();
            let mut h = self.history.borrow_mut();
            let at = self.hist_at.get();
            let keep = if h.is_empty() { 0 } else { at + 1 };
            h.truncate(keep);
            h.push(Visit { file: file.to_path_buf(), place: Place::default() });
            self.hist_at.set(h.len() - 1);
        }
        *self.current.borrow_mut() = Some(file.to_path_buf());
        self.set_gone(false);
        self.close_find_quietly();
        let mut uri = scheme::uri(&self.host, file);
        if let Some(p) = place {
            uri.push_str(&p.query());
        }
        if let Some(f) = fragment.filter(|f| !f.is_empty()) {
            uri.push('#');
            uri.push_str(f);
        }
        self.load(&uri);
        let name = folder::file_name(file);
        self.title.set_label(&name);
        let dir = file.parent().map(|p| root.join(p)).unwrap_or(root);
        self.subtitle.set_label(&folder::tilde(&dir));
        self.window.set_title(Some(&format!("{name} — omamv")));
        self.changed.set_visible(false);
        self.tree.mark(Some(file));
        self.update_status();
    }

    fn load(&self, uri: &str) {
        trace!("load {uri}");
        *self.expect.borrow_mut() = uri.to_owned();
        self.web.load_uri(uri);
    }

    /// Note where the reader is in the current file, for Alt+← later.
    fn remember_place(&self) {
        let at = self.hist_at.get();
        let weak = self.weak();
        self.web.evaluate_javascript("omamv.place()", None, None, gio::Cancellable::NONE, move |r| {
            let (Some(w), Ok(v)) = (weak.upgrade(), r) else { return };
            let place = Place::parse(&v.to_str());
            if let Some(visit) = w.history.borrow_mut().get_mut(at) {
                visit.place = place;
            }
        });
    }

    fn go_back(&self, dir: i32) {
        let to = self.hist_at.get() as i64 + dir as i64;
        let len = self.history.borrow().len() as i64;
        if to < 0 || to >= len {
            return;
        }
        self.remember_place();
        self.hist_at.set(to as usize);
        let visit = self.history.borrow()[to as usize].clone();
        self.open_file(&visit.file, None, Some(visit.place), false);
    }

    fn step_file(&self, by: i64) {
        let scan = self.scan.borrow();
        if scan.order.is_empty() {
            return;
        }
        let cur = self.current.borrow().clone();
        let at = cur.and_then(|c| scan.order.iter().position(|f| *f == c));
        let n = scan.order.len() as i64;
        let to = match at {
            Some(i) => (i as i64 + by).clamp(0, n - 1) as usize,
            None => 0,
        };
        if Some(to) == at {
            return;
        }
        let file = scan.order[to].clone();
        drop(scan);
        self.open_file(&file, None, None, true);
    }

    /// A link was followed in the page.
    fn follow(&self, uri: &str) {
        if let Some((host, rel, fragment)) = scheme::parse(uri) {
            if host != self.host {
                return;
            }
            let Some(root) = self.root() else { return };
            let full = root.join(&rel);
            if self.current.borrow().as_deref() == Some(rel.as_path()) {
                if let Some(f) = fragment {
                    self.js(&format!("document.getElementById(decodeURIComponent({}))?.scrollIntoView({{behavior:'instant'}})", js_string(&f)));
                }
                return;
            }
            if folder::is_shown(&rel) && full.is_file() {
                self.open_file(&rel, fragment.as_deref(), None, true);
            } else if full.exists() {
                // Not something omamv shows: the desktop's own app for it.
                let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&full)));
                launcher.launch(Some(&self.window), gio::Cancellable::NONE, |_| {});
            }
            return;
        }
        if uri.starts_with("https://") || uri.starts_with("http://") || uri.starts_with("mailto:") {
            gtk::UriLauncher::new(uri).launch(Some(&self.window), gio::Cancellable::NONE, |_| {});
        }
    }

    pub fn reload_keeping_place(&self) {
        let Some(file) = self.current.borrow().clone() else { return };
        let weak = self.weak();
        self.web.evaluate_javascript("omamv.place()", None, None, gio::Cancellable::NONE, move |r| {
            let Some(w) = weak.upgrade() else { return };
            let place = r.map(|v| Place::parse(&v.to_str())).unwrap_or_default();
            if w.current.borrow().as_deref() != Some(file.as_path()) {
                return;
            }
            let uri = format!("{}{}", scheme::uri(&w.host, &file), place.query());
            w.load(&uri);
        });
    }

    /// A new palette: the page background at once, the page itself again.
    pub fn repaint(&self, palette: &crate::theme::Palette) {
        paint_background(&self.web, palette);
        if self.current.borrow().is_some() {
            self.reload_keeping_place();
        }
    }

    // --------------------------------------------------------- the disk

    fn on_change(&self, path: &Path) {
        let Some(root) = self.root() else { return };
        let current = self.current.borrow().as_ref().map(|c| root.join(c));
        if current.as_deref() == Some(path) {
            self.debounce(&self.reload_timer, 120, |w| w.current_changed());
        }
        if crate::watch::touches_tree(path) {
            self.debounce(&self.rescan_timer, 300, |w| w.rescan(false, None));
        }
    }

    fn debounce(&self, slot: &RefCell<Option<glib::SourceId>>, ms: u64, f: fn(&Win)) {
        if let Some(id) = slot.borrow_mut().take() {
            id.remove();
        }
        let weak = self.weak();
        let id = glib::timeout_add_local_once(Duration::from_millis(ms), move || {
            if let Some(w) = weak.upgrade() {
                if ms == 120 {
                    w.reload_timer.borrow_mut().take();
                } else {
                    w.rescan_timer.borrow_mut().take();
                }
                f(&w);
            }
        });
        *slot.borrow_mut() = Some(id);
    }

    fn current_changed(&self) {
        let (Some(root), Some(file)) = (self.root(), self.current.borrow().clone()) else { return };
        if root.join(&file).is_file() {
            self.set_gone(false);
            self.reload_keeping_place();
            self.show_changed();
        } else {
            self.set_gone(true);
        }
    }

    /// The only sign of a reload: a dot and a time beside the folder, for a
    /// few seconds.
    fn show_changed(&self) {
        let time = glib::DateTime::now_local().ok().and_then(|t| t.format("%H:%M").ok()).map(|s| s.to_string()).unwrap_or_default();
        // Pango markup has no classes: the dot takes the accent by attribute.
        let accent = scheme::palette().accent.clone();
        self.changed.set_markup(&format!("<span foreground=\"{accent}\">●</span> updated {time}"));
        self.changed.set_visible(true);
        if let Some(id) = self.changed_timer.borrow_mut().take() {
            id.remove();
        }
        let weak = self.weak();
        *self.changed_timer.borrow_mut() = Some(glib::timeout_add_local_once(Duration::from_secs(6), move || {
            if let Some(w) = weak.upgrade() {
                w.changed_timer.borrow_mut().take();
                w.changed.set_visible(false);
            }
        }));
    }

    fn set_gone(&self, gone: bool) {
        let was = self.gone.replace(gone);
        self.gone_label.set_visible(gone);
        if gone {
            self.changed.set_visible(false);
            let name = self.current.borrow().as_deref().map(folder::file_name).unwrap_or_default();
            self.banner.set_title(&format!("{name} was removed. The last version stays on screen."));
            self.banner.set_revealed(true);
            self.js("omamv.gone(true)");
            if !was {
                // The tree has dropped it; the keyboard goes there to pick another.
                let weak = self.weak();
                glib::timeout_add_local_once(Duration::from_millis(400), move || {
                    if let Some(w) = weak.upgrade()
                        && w.gone.get()
                    {
                        w.split.set_show_sidebar(true);
                        w.tree.focus();
                    }
                });
            }
        } else if was {
            self.banner.set_revealed(false);
            self.js("omamv.gone(false)");
        }
    }

    // ------------------------------------------------------------ tools

    pub fn quick_open(&self) {
        let files = self.scan.borrow().order.clone();
        if files.is_empty() {
            return;
        }
        let mut recent: Vec<PathBuf> = self.history.borrow().iter().rev().map(|v| v.file.clone()).collect();
        recent.dedup();
        let weak = self.weak();
        let qo = QuickOpen::new(files, recent, &scheme::palette().accent, move |file| {
            if let Some(w) = weak.upgrade() {
                w.open_file(&file, None, None, true);
                w.web.grab_focus();
            }
        });
        // Noted first: the status bar asks which dialog it is as it appears.
        *self.quick.borrow_mut() = Some(qo.clone());
        qo.present(&self.window);
    }

    pub fn open_find(&self) {
        if self.current.borrow().is_none() {
            return;
        }
        self.findbar.set_search_mode(true);
        self.find_entry.grab_focus();
        let last = self.last_find.borrow().clone();
        if !last.is_empty() && self.find_entry.text().is_empty() {
            self.find_entry.set_text(&last);
        }
        self.find_entry.select_region(0, -1);
    }

    fn close_find(&self) {
        self.findbar.set_search_mode(false);
    }

    fn close_find_quietly(&self) {
        if self.findbar.is_search_mode() {
            self.findbar.set_search_mode(false);
        }
        self.find_count.set_label("");
    }

    /// `n` / `N` after the bar is closed: the same words, from the last match.
    fn find_again(&self, dir: i64) {
        let q = self.last_find.borrow().clone();
        if q.is_empty() {
            return;
        }
        let at = self.last_find_at.get();
        // Find again, then step from where the search starts to the match
        // after the last one.
        self.find_js(&format!("(() => {{ const r = JSON.parse(omamv.find({})); if (!r.count) return JSON.stringify(r); \
            const want = ((({at} - 1 + ({dir})) % r.count) + r.count) % r.count; \
            let now = JSON.parse(omamv.step(0)).at - 1; return omamv.step(want - now) }})()", js_string(&q)));
        let weak = self.weak();
        glib::timeout_add_local_once(Duration::from_millis(1200), move || {
            if let Some(w) = weak.upgrade()
                && !w.findbar.is_search_mode()
            {
                w.js("omamv.endFind()");
            }
        });
    }

    /// Run a find call; the page answers with {count, at}.
    fn find_js(&self, script: &str) {
        let weak = self.weak();
        self.web.evaluate_javascript(script, None, None, gio::Cancellable::NONE, move |r| {
            let (Some(w), Ok(v)) = (weak.upgrade(), r) else { return };
            let s = v.to_str().to_string();
            let num = |k: &str| -> i64 {
                s.find(&format!("\"{k}\":"))
                    .map(|i| &s[i + k.len() + 3..])
                    .and_then(|r| r.split([',', '}']).next())
                    .and_then(|n| n.trim().parse().ok())
                    .unwrap_or(0)
            };
            let (count, at) = (num("count"), num("at"));
            w.last_find_at.set(at);
            let label = match count {
                _ if w.find_entry.text().is_empty() && w.findbar.is_search_mode() => String::new(),
                0 => "No match".into(),
                _ => format!("{at} of {count}"),
            };
            w.find_count.set_label(&label);
        });
    }

    fn toggle_tree(&self) {
        if !self.split.shows_sidebar() {
            self.split.set_show_sidebar(true);
            self.tree.focus();
        } else if self.tree.has_focus() {
            self.split.set_show_sidebar(false);
            self.web.grab_focus();
        } else {
            self.tree.focus();
        }
    }

    fn set_zoom(&self, z: f64) {
        let z = (z * 20.0).round() / 20.0;
        let z = z.clamp(0.5, 3.0);
        self.zoom.set(z);
        self.web.set_zoom_level(z);
    }

    pub fn show_settings(&self) {
        super::settings::dialog().present(Some(&self.window));
    }

    /// Take the settings as they are now (ADR-0014).
    pub fn apply_settings(&self) {
        let s = crate::settings::get();
        self.split.set_sidebar_position(if s.tree_left { gtk::PackType::Start } else { gtk::PackType::End });
        if s.tree_left {
            self.tree.widget.add_css_class("om-tree-left");
        } else {
            self.tree.widget.remove_css_class("om-tree-left");
        }
        self.tree_button.set_icon_name(if s.tree_left { "sidebar-show-symbolic" } else { "sidebar-show-right-symbolic" });
        self.status.widget.set_visible(s.key_hints);
        self.set_zoom(s.zoom());
        if self.root.borrow().is_some() {
            self.rescan(false, None);
            self.reload_keeping_place();
        }
    }

    pub fn show_keys(&self) {
        super::keys::dialog().present(Some(&self.window));
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("omamv")
            .application_icon(crate::APP_ID)
            .developer_name("Jonas Montonen")
            .version(env!("CARGO_PKG_VERSION"))
            .comments("Read a folder of Markdown, rendered. omamv only reads.")
            .license_type(gtk::License::MitX11)
            .build();
        about.present(Some(&self.window));
    }

    /// Where the keyboard is, for the status bar.
    pub fn keyboard_mode(&self) -> Mode {
        if let Some(d) = self.window.visible_dialog() {
            let quick = self.quick.borrow().as_ref().is_some_and(|q| q.dialog == d);
            return if quick { Mode::QuickOpen } else { Mode::Dialog };
        }
        match self.showing().as_str() {
            "start" => return Mode::Start,
            "empty" => return Mode::Empty,
            "reading" => return Mode::Reading,
            _ => {}
        }
        if self.findbar.is_search_mode() {
            return Mode::Find;
        }
        if self.tree.has_focus() {
            return Mode::Tree;
        }
        Mode::Page
    }

    pub fn update_status(&self) {
        let mode = self.keyboard_mode();
        let place = self.current.borrow().as_ref().and_then(|c| {
            let scan = self.scan.borrow();
            scan.order.iter().position(|f| f == c).map(|i| (i + 1, scan.order.len()))
        });
        let place = if matches!(mode, Mode::Page | Mode::Tree | Mode::Find) { place } else { None };
        self.status.set(mode, place);
    }

    /// A short message in the subtitle, red, until the next file opens.
    fn say(&self, text: &str) {
        self.gone_label.set_label(text);
        self.gone_label.set_visible(true);
        let weak = self.weak();
        glib::timeout_add_local_once(Duration::from_secs(8), move || {
            if let Some(w) = weak.upgrade()
                && !w.gone.get()
            {
                w.gone_label.set_visible(false);
                w.gone_label.set_label("gone");
            }
        });
    }

    /// Run page script, ignoring the answer.
    pub fn js(&self, script: &str) {
        self.web.evaluate_javascript(script, None, None, gio::Cancellable::NONE, |_| {});
    }
}

fn paint_background(web: &WebView, palette: &crate::theme::Palette) {
    if let Ok(c) = gdk::RGBA::parse(palette.background.as_str()) {
        web.set_background_color(&c);
    }
}

/// A Rust string as a JavaScript string literal.
pub fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_round_trip() {
        let p = Place::parse(r#"{"id":"open-a-folder","off":12,"y":840}"#);
        assert_eq!(p, Place { id: "open-a-folder".into(), off: 12, y: 840 });
        assert_eq!(p.query(), "?a=open-a-folder&o=12&y=840");
        assert_eq!(Place::parse(r#"{"id":"","off":0,"y":0}"#), Place::default());
    }

    #[test]
    fn strings_reach_javascript_whole() {
        assert_eq!(js_string("a\"b\\c\n</script>"), r#""a\"b\\c\n</script>""#);
    }
}
