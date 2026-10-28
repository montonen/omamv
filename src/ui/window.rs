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

use super::quickopen::{Item, QuickOpen};
use super::start::Start;
use super::status::{Mode, Status};
use super::tree::Tree;
use crate::folder::{self, Scan};
use crate::scheme;

static NEXT_HOST: AtomicU32 = AtomicU32::new(1);

/// A window action: its name and what it does.
type Action = (&'static str, fn(&Win));

/// Visits kept for back and forward.
const HISTORY: usize = 200;

/// How long modifiers are held before their shortcuts show.
pub const HELD_MS: u64 = 400;

/// The modifier a key is, if it is one.
fn modifier_of(key: gdk::Key) -> Option<gdk::ModifierType> {
    match key {
        gdk::Key::Control_L | gdk::Key::Control_R => Some(gdk::ModifierType::CONTROL_MASK),
        gdk::Key::Shift_L | gdk::Key::Shift_R => Some(gdk::ModifierType::SHIFT_MASK),
        gdk::Key::Alt_L | gdk::Key::Alt_R | gdk::Key::Meta_L | gdk::Key::Meta_R => Some(gdk::ModifierType::ALT_MASK),
        _ => None,
    }
}

/// `OMAMV_DEBUG=1`: each step of opening a folder and a page, on stderr.
macro_rules! trace {
    ($($arg:tt)*) => {
        if std::env::var_os("OMAMV_DEBUG").is_some() {
            eprintln!("omamv: {}", format!($($arg)*));
        }
    };
}

#[derive(Clone, Copy)]
enum Timer {
    Reload,
    Rescan,
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
    /// The settings this window last took (ADR-0014).
    applied: RefCell<Option<crate::settings::Settings>>,
    pub split: adw::OverlaySplitView,
    tree_handle: gtk::Box,
    /// The width a drag of the tree's edge started from.
    drag_from: Cell<f64>,
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
    watcher: RefCell<Option<crate::watch::Watch>>,
    /// The file asked for while the folder was still being read.
    pending_open: RefCell<Option<PathBuf>>,
    /// When the folder was asked for, and how long its scan took: the load
    /// time shown in the status bar.
    opened_at: Cell<Option<std::time::Instant>>,
    scan_ms: Cell<u128>,
    /// Walks of the folder so far (the checks count them).
    pub scans: Cell<u32>,
    pub gone: Cell<bool>,
    /// The next walk follows a change on disk: files new to it are named.
    from_disk: Cell<bool>,
    pending_g: Cell<bool>,
    /// Shows the held modifiers' shortcuts once they have been held a moment.
    held_timer: RefCell<Option<glib::SourceId>>,
    reload_timer: RefCell<Option<glib::SourceId>>,
    rescan_timer: RefCell<Option<glib::SourceId>>,
    changed_timer: RefCell<Option<glib::SourceId>>,
    zoom: Cell<f64>,
    pub ready: Cell<bool>,
    pub loads: Cell<u32>,
    /// What the page said last (debug and the self-check read it).
    pub last_message: RefCell<String>,
    pub quick: RefCell<Option<Rc<QuickOpen>>>,
    /// Command lines waiting for this window to close (`omamv -w`).
    held: RefCell<Vec<gio::ApplicationCommandLine>>,
    me: RefCell<std::rc::Weak<Win>>,
}

impl Win {
    pub fn new(app: &adw::Application) -> Rc<Win> {
        let host = format!("w{}", NEXT_HOST.fetch_add(1, Ordering::Relaxed));

        // The page.
        let ucm = webkit6::UserContentManager::new();
        ucm.register_script_message_handler("omamv", None);
        // Nothing to keep between runs (cookies, caches): an ephemeral
        // session writes nothing to disk (ADR-0002).
        thread_local! {
            static SESSION: webkit6::NetworkSession = webkit6::NetworkSession::new_ephemeral();
        }
        let web = SESSION.with(|session| WebView::builder().user_content_manager(&ucm).network_session(session).build());
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
        // A thin edge on the tree's page side: drag it to resize the tree.
        let tree_handle = gtk::Box::builder()
            .width_request(6)
            .halign(gtk::Align::Start)
            .vexpand(true)
            .tooltip_text("Drag to resize, double-click for the usual width (< and > too)")
            .build();
        tree_handle.add_css_class("om-tree-handle");
        tree_handle.set_cursor_from_name(Some("col-resize"));
        let tree_box = gtk::Overlay::new();
        tree_box.set_child(Some(&tree.widget));
        tree_box.add_overlay(&tree_handle);
        let width = f64::from(crate::settings::get().tree_width);
        let split = adw::OverlaySplitView::builder()
            .sidebar(&tree_box)
            .content(&web)
            .sidebar_position(gtk::PackType::End)
            .max_sidebar_width(width)
            .min_sidebar_width(width)
            .sidebar_width_fraction(1.0)
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
            applied: RefCell::new(None),
            split,
            tree_handle,
            drag_from: Cell::new(0.0),
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
            pending_open: RefCell::new(None),
            opened_at: Cell::new(None),
            scan_ms: Cell::new(0),
            scans: Cell::new(0),
            gone: Cell::new(false),
            from_disk: Cell::new(false),
            pending_g: Cell::new(false),
            held_timer: RefCell::new(None),
            reload_timer: RefCell::new(None),
            rescan_timer: RefCell::new(None),
            changed_timer: RefCell::new(None),
            zoom: Cell::new(1.0),
            ready: Cell::new(false),
            loads: Cell::new(0),
            last_message: RefCell::new(String::new()),
            quick: RefCell::new(None),
            held: RefCell::new(Vec::new()),
            me: RefCell::new(std::rc::Weak::new()),
        });
        *win.me.borrow_mut() = Rc::downgrade(&win);
        *win_holder.borrow_mut() = Rc::downgrade(&win);
        super::remember(&win);
        win.wire(&ucm);
        win.apply_settings();
        win.show_start();
        win
    }

    /// Keep `cmd` until this window closes: `omamv -w` waits for that.
    pub fn hold(&self, cmd: &gio::ApplicationCommandLine) {
        self.held.borrow_mut().push(cmd.clone());
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

    fn wire(&self, ucm: &webkit6::UserContentManager) {
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
            if msg.contains("\"place\":") {
                w.keep_place(Place::parse(&msg));
            }
            if msg.contains("\"ready\":true") {
                w.ready.set(true);
                if w.gone.get() {
                    w.js("omamv.gone(true)");
                }
                // How fast: the folder's scan, and the first page drawn.
                if let Some(t) = w.opened_at.take() {
                    let files = w.scan.borrow().order.len();
                    let total = t.elapsed().as_millis();
                    let note = format!("{files} files read in {} ms · first page in {total} ms", w.scan_ms.get());
                    trace!("{note}");
                    w.status.note(&note);
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
                // An HTML page from the folder has no omamv script of its own
                // (its policy blocks scripts); the helper comes in through
                // WebKit, which the policy does not stop (ADR-0016).
                let html = w.current.borrow().as_deref().is_some_and(folder::is_html);
                if html {
                    let helper = String::from_utf8_lossy(scheme::APP_JS).into_owned();
                    w.web.evaluate_javascript(&helper, None, None, gio::Cancellable::NONE, |r| {
                        if let Err(e) = r {
                            eprintln!("omamv: the page helper did not start: {e}");
                        }
                    });
                }
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
        let actions: [Action; 5] = [
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

        // The mouse's back and forward buttons (by the thumb): the window
        // takes them before the page, which would walk WebKit's own list.
        let thumb = gtk::GestureClick::builder().button(0).propagation_phase(gtk::PropagationPhase::Capture).build();
        let weak = self.weak();
        thumb.connect_pressed(move |g, _, _, _| {
            let Some(w) = weak.upgrade() else { return };
            let dir = match g.current_button() {
                8 => -1,
                9 => 1,
                _ => return,
            };
            g.set_state(gtk::EventSequenceState::Claimed);
            if w.window.visible_dialog().is_none() {
                w.go_back(dir);
            }
        });
        self.window.add_controller(thumb);

        // The tree's edge: drag to resize, double-click for the usual width.
        let drag = gtk::GestureDrag::new();
        let weak = self.weak();
        drag.connect_drag_begin(move |_, _, _| {
            if let Some(w) = weak.upgrade() {
                w.drag_from.set(w.split.max_sidebar_width());
            }
        });
        let weak = self.weak();
        drag.connect_drag_update(move |_, dx, _| {
            if let Some(w) = weak.upgrade() {
                // The edge faces the page: dragging toward the page widens.
                let sign = if crate::settings::get().tree_left { 1.0 } else { -1.0 };
                w.set_tree_width(w.drag_from.get() + sign * dx);
            }
        });
        let weak = self.weak();
        drag.connect_drag_end(move |_, _, _| {
            if let Some(w) = weak.upgrade() {
                w.keep_tree_width();
            }
        });
        self.tree_handle.add_controller(drag);
        let twice = gtk::GestureClick::new();
        let weak = self.weak();
        twice.connect_pressed(move |_, n, _, _| {
            if n == 2
                && let Some(w) = weak.upgrade()
            {
                w.set_tree_width(f64::from(crate::settings::TREE_DEFAULT));
                w.keep_tree_width();
            }
        });
        self.tree_handle.add_controller(twice);

        // Keys: the window sees them before the page does.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.weak();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(w) = weak.upgrade() else { return glib::Propagation::Proceed };
            // The state given is from before this key: a modifier adds its own.
            match modifier_of(key) {
                Some(m) => w.modifiers(mods | m),
                None => w.cancel_held(),
            }
            if w.key(key, mods) { glib::Propagation::Stop } else { glib::Propagation::Proceed }
        });
        let weak = self.weak();
        keys.connect_key_released(move |_, key, _, mods| {
            if let (Some(w), Some(m)) = (weak.upgrade(), modifier_of(key)) {
                w.modifiers(mods - m);
            }
        });
        self.window.add_controller(keys);
        // Keys let go elsewhere never reach the window: start clean on return.
        let weak = self.weak();
        self.window.connect_is_active_notify(move |win| {
            if let Some(w) = weak.upgrade()
                && !win.is_active()
            {
                w.modifiers(gdk::ModifierType::empty());
            }
        });

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
                w.held.borrow_mut().clear();
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
            (true, _, "comma") => {
                self.show_settings();
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
            "j" | "Down" => self.js("omamv.scroll('down')"),
            "k" | "Up" => self.js("omamv.scroll('up')"),
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
            "H" => self.go_back(-1),
            "L" => self.go_back(1),
            "bracketright" | "J" => self.step_file(1),
            "bracketleft" | "K" => self.step_file(-1),
            "slash" => self.open_find(),
            "o" => self.outline(),
            "less" => self.step_tree_width(-32.0),
            "greater" => self.step_tree_width(32.0),
            "y" => self.copy_path(),
            "n" => self.find_again(1),
            "N" => self.find_again(-1),
            "question" => self.show_keys(),
            _ => return false,
        }
        true
    }

    /// The modifiers now held. Ctrl or Alt held a moment (Shift adds to
    /// them) lists their shortcuts in the status bar; a quick Ctrl+P never
    /// gets that far.
    pub fn modifiers(&self, mods: gdk::ModifierType) {
        let mut parts = Vec::new();
        if mods.contains(gdk::ModifierType::CONTROL_MASK) {
            parts.push("Ctrl");
        }
        if mods.contains(gdk::ModifierType::SHIFT_MASK) {
            parts.push("Shift");
        }
        if mods.contains(gdk::ModifierType::ALT_MASK) {
            parts.push("Alt");
        }
        if let Some(id) = self.held_timer.borrow_mut().take() {
            id.remove();
        }
        if !mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
            self.status.hold(None);
            return;
        }
        let prefix = parts.join("+");
        // Already showing: follow at once (Shift added or let go).
        if self.status.held().is_some() {
            self.status.hold(Some(&prefix));
            return;
        }
        let weak = self.weak();
        *self.held_timer.borrow_mut() = Some(glib::timeout_add_local_once(Duration::from_millis(HELD_MS), move || {
            if let Some(w) = weak.upgrade() {
                w.held_timer.borrow_mut().take();
                w.status.hold(Some(&prefix));
            }
        }));
    }

    /// Another key came: the modifiers are in use, not being asked about.
    fn cancel_held(&self) {
        if let Some(id) = self.held_timer.borrow_mut().take() {
            id.remove();
        }
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
            match self.showing().as_str() {
                // Still reading: the file waits for the scan.
                "reading" => {
                    if file.is_some() {
                        *self.pending_open.borrow_mut() = file;
                    }
                }
                _ => {
                    let empty = self.scan.borrow().order.is_empty();
                    self.stack.set_visible_child_name(if empty { "empty" } else { "doc" });
                    self.set_chrome(!empty);
                    if let Some(f) = file {
                        self.open_file(&f, None, None, true);
                    }
                    if !empty {
                        self.web.grab_focus();
                    }
                }
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
        *self.pending_open.borrow_mut() = file;
        self.opened_at.set(Some(std::time::Instant::now()));
        for slot in [&self.reload_timer, &self.rescan_timer] {
            if let Some(id) = slot.borrow_mut().take() {
                id.remove();
            }
        }
        self.set_gone(false);
        self.last_find_at.set(0);
        self.stack.set_visible_child_name("reading");
        self.set_chrome(false);
        self.title.set_label(&folder::file_name(&root));
        self.subtitle.set_label(&folder::tilde(&root));
        self.web.load_html(&crate::render::wrap("", "", &scheme::palette()), None);

        // Follow the disk: the folders to watch come with the scan.
        *self.watcher.borrow_mut() = None;
        if let Some((watch, changes)) = crate::watch::Watch::new() {
            *self.watcher.borrow_mut() = Some(watch);
            let weak = self.weak();
            let watched = root.clone();
            glib::spawn_future_local(async move {
                while let Ok(change) = changes.recv().await {
                    let Some(w) = weak.upgrade() else { break };
                    if w.root.borrow().as_deref() != Some(watched.as_path()) {
                        break;
                    }
                    w.on_change(&change);
                }
            });
        }
        self.rescan(true);
    }

    /// Walk the folder again, off the main thread.
    /// Walk the folder again, off the main thread. The tree is rebuilt only
    /// when what it shows changed.
    fn rescan(&self, first: bool) {
        let Some(root) = self.root() else { return };
        let from_disk = self.from_disk.replace(false) && !first;
        self.scans.set(self.scans.get() + 1);
        let (tx, rx) = async_channel::bounded(1);
        let walk_root = root.clone();
        std::thread::spawn(move || {
            let s = crate::settings::get();
            let skip = folder::Skip { hidden: !s.hidden, gitignored: s.gitignore };
            let t = std::time::Instant::now();
            let scan = folder::scan(&walk_root, skip);
            let _ = tx.send_blocking((scan, t.elapsed().as_millis()));
        });
        let weak = self.weak();
        glib::spawn_future_local(async move {
            let Ok((scan, ms)) = rx.recv().await else { return };
            trace!("scan done: {} files, {} folders, {ms} ms", scan.order.len(), scan.dirs.len());
            let Some(w) = weak.upgrade() else { return };
            if w.root.borrow().as_deref() != Some(root.as_path()) {
                trace!("scan is for another folder; dropped");
                return;
            }
            if let Some(watch) = w.watcher.borrow_mut().as_mut() {
                watch.follow(&scan.dirs);
                trace!("watching {} folders", watch.count());
            }
            if first {
                w.scan_ms.set(ms);
            }
            let changed = first || w.scan.borrow().tree != scan.tree;
            if changed {
                w.tree.show(&folder::file_name(&root), &scan, first);
                trace!("tree shown");
            } else {
                w.tree.set_left_out(&scan);
            }
            let empty = scan.order.is_empty();
            if matches!(w.showing().as_str(), "reading" | "empty" | "doc") {
                w.stack.set_visible_child_name(if empty { "empty" } else { "doc" });
                w.set_chrome(!empty);
            }
            let nothing_open = w.current.borrow().is_none();
            let asked = w.pending_open.borrow_mut().take().filter(|f| scan.order.contains(f));
            // "Open a file by name" (ADR-0014): no file of omamv's choosing,
            // now or on a later walk; a file asked for still opens.
            let by_name = crate::settings::get().ask_on_open;
            let ask_now = first && asked.is_none() && by_name && !empty;
            // A folder read before opens where the reader left it (ADR-0019).
            let left = if first && asked.is_none() && !by_name { crate::places::get(&root).filter(|s| scan.order.contains(&s.file)) } else { None };
            let place = left.as_ref().map(|s| Place { id: s.id.clone(), off: s.off, y: s.y });
            let pick = asked.or_else(|| left.map(|s| s.file)).or_else(|| if (first || nothing_open) && !by_name { folder::first_file(&scan) } else { None });
            trace!("pick {pick:?}");
            if from_disk {
                w.name_new_files(&scan);
            }
            scheme::set_files(&w.host, scan.order.clone());
            *w.scan.borrow_mut() = scan;
            if ask_now {
                w.pick_a_file();
            } else if let Some(f) = pick {
                w.open_file(&f, None, place, true);
                w.web.grab_focus();
            } else if empty {
                w.empty_folder();
            } else if changed {
                let cur = w.current.borrow().clone();
                w.tree.mark(cur.as_deref());
            }
            w.update_status();
        });
    }

    /// Files that appeared on disk since the last walk, in the status bar
    /// (ADR-0017): an agent just wrote a report. Ctrl+P lists it first.
    fn name_new_files(&self, scan: &Scan) {
        let new: Vec<PathBuf> = {
            let old = self.scan.borrow();
            let known: std::collections::HashSet<&PathBuf> = old.order.iter().collect();
            scan.order.iter().filter(|f| !known.contains(f)).cloned().collect()
        };
        match new.as_slice() {
            [] => {}
            [one] => self.status.note(&format!("new: {} · Ctrl+P", one.display())),
            many => self.status.note(&format!("{} new files · Ctrl+P", many.len())),
        }
    }

    /// A clean page and quick open: the reader names the file.
    fn pick_a_file(&self) {
        trace!("asking for a file by name");
        let body = r#"<div class="om-pick"><p>Open a file by name: <kbd>Ctrl</kbd>+<kbd>P</kbd></p><p>or pick one in the tree.</p></div>"#;
        self.web.load_html(&crate::render::wrap("omamv", body, &scheme::palette()), None);
        self.title.set_label(&self.root().map(|r| folder::file_name(&r)).unwrap_or_default());
        self.window.set_title(Some("omamv"));
        self.update_status();
        self.quick_open();
    }

    fn empty_folder(&self) {
        trace!("nothing to show");
        self.set_gone(false);
        let s = crate::settings::get();
        let skipped = match (s.hidden, s.gitignore) {
            (false, true) => " Hidden folders and anything .gitignore leaves out are skipped (Ctrl+, changes that).",
            (false, false) => " Hidden folders are skipped (Ctrl+, changes that).",
            (true, true) => " Anything .gitignore leaves out is skipped (Ctrl+, changes that).",
            (true, false) => "",
        };
        self.empty_page.set_description(Some(&format!(
            "omamv shows .md and .txt files, in this folder and the folders below it.{skipped}"
        )));
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
            // A long day's reading: the oldest visits go first.
            let over = h.len().saturating_sub(HISTORY);
            h.drain(..over);
            self.hist_at.set(h.len() - 1);
        }
        *self.current.borrow_mut() = Some(file.to_path_buf());
        let p = place.clone().unwrap_or_default();
        crate::places::set(&root, &crate::places::Spot { file: file.to_path_buf(), id: p.id, off: p.off, y: p.y });
        // Back to a file removed since: it is shown as gone, not as an error.
        self.set_gone(!root.join(file).is_file());
        self.last_find_at.set(0);
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
        self.ready.set(false);
        *self.expect.borrow_mut() = uri.to_owned();
        self.web.load_uri(uri);
    }

    /// Note where the reader is in the current file, for Alt+← later.
    /// The page says where the reader stopped: kept for this visit and for
    /// the folder (ADR-0019).
    fn keep_place(&self, place: Place) {
        let (Some(root), Some(file)) = (self.root(), self.current.borrow().clone()) else { return };
        if !self.ready.get() {
            return;
        }
        if let Some(visit) = self.history.borrow_mut().get_mut(self.hist_at.get())
            && visit.file == file
        {
            visit.place = place.clone();
        }
        crate::places::set(&root, &crate::places::Spot { file, id: place.id, off: place.off, y: place.y });
    }

    fn remember_place(&self) {
        // A page still loading would answer for the one before it.
        if !self.ready.get() {
            return;
        }
        let at = self.hist_at.get();
        let Some(file) = self.current.borrow().clone() else { return };
        let weak = self.weak();
        self.web.evaluate_javascript("omamv.place()", None, None, gio::Cancellable::NONE, move |r| {
            let (Some(w), Ok(v)) = (weak.upgrade(), r) else { return };
            let place = Place::parse(&v.to_str());
            if let Some(visit) = w.history.borrow_mut().get_mut(at)
                && visit.file == file
            {
                visit.place = place;
            }
        });
    }

    /// Back (`dir` -1) or forward (+1) through the files read in this
    /// folder, each at the place it was left (ADR-0020).
    pub fn go_back(&self, dir: i32) {
        let to = self.hist_at.get() as i64 + dir as i64;
        let len = self.history.borrow().len() as i64;
        if self.current.borrow().is_none() || self.showing() != "doc" {
            return;
        }
        if to < 0 || to >= len {
            self.status.note(if dir < 0 { "no earlier file in this folder" } else { "no later file" });
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
            // A file omamv shows opens here. Anything else is left alone: a
            // document must never get the desktop to open (and so perhaps
            // run) another file with one click, ADR-0010.
            if folder::is_shown(&rel) && full.is_file() {
                self.open_file(&rel, fragment.as_deref(), None, true);
            } else {
                self.say(if crate::settings::get().html { "omamv shows .md, .txt and .html files" } else { "omamv shows .md and .txt files (HTML pages: Ctrl+,)" });
            }
            return;
        }
        if uri.starts_with("https://") || uri.starts_with("http://") || uri.starts_with("mailto:") {
            gtk::UriLauncher::new(uri).launch(Some(&self.window), gio::Cancellable::NONE, |_| {});
        }
    }

    pub fn reload_keeping_place(&self) {
        // A removed file's last version stays as it is: there is nothing to load.
        if self.gone.get() {
            return;
        }
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

    fn on_change(&self, change: &crate::watch::Change) {
        let Some(root) = self.root() else { return };
        let current = self.current.borrow().as_ref().map(|c| root.join(c));
        if current.as_deref() == Some(change.path.as_path()) {
            self.debounce(Timer::Reload);
        }
        if crate::watch::touches_tree(change) {
            self.debounce(Timer::Rescan);
        }
    }

    fn debounce(&self, which: Timer) {
        let (slot, ms) = match which {
            Timer::Reload => (&self.reload_timer, 120),
            Timer::Rescan => (&self.rescan_timer, 300),
        };
        if let Some(id) = slot.borrow_mut().take() {
            id.remove();
        }
        let weak = self.weak();
        let id = glib::timeout_add_local_once(Duration::from_millis(ms), move || {
            let Some(w) = weak.upgrade() else { return };
            match which {
                Timer::Reload => {
                    w.reload_timer.borrow_mut().take();
                    w.current_changed();
                }
                Timer::Rescan => {
                    w.rescan_timer.borrow_mut().take();
                    w.from_disk.set(true);
                    w.rescan(false);
                }
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
        self.gone_label.set_label("gone");
        self.gone_label.set_visible(gone);
        if gone {
            self.changed.set_visible(false);
            let name = self.current.borrow().as_deref().map(folder::file_name).unwrap_or_default();
            self.banner.set_title(&format!("{name} was removed. The last version stays on screen."));
            // Over the page only; set_chrome shows it again on the way back.
            self.banner.set_revealed(self.showing() == "doc");
            self.js("omamv.gone(true)");
            if !was {
                // The tree has dropped it; the keyboard goes there to pick another.
                let weak = self.weak();
                glib::timeout_add_local_once(Duration::from_millis(400), move || {
                    // Only from the page: never out of find, quick open or a dialog.
                    if let Some(w) = weak.upgrade()
                        && w.gone.get()
                        && w.keyboard_mode() == Mode::Page
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

    /// Quick open: every file, the newest first, with its age (ADR-0017).
    /// The ages are read now, not at the scan: a report rewritten since
    /// counts as new.
    pub fn quick_open(&self) {
        let Some(root) = self.root() else { return };
        let files = self.scan.borrow().order.clone();
        if files.is_empty() {
            return;
        }
        let t = std::time::Instant::now();
        let now = std::time::SystemTime::now();
        let mut dated: Vec<(Option<std::time::SystemTime>, PathBuf)> =
            files.into_iter().map(|f| (root.join(&f).metadata().and_then(|m| m.modified()).ok(), f)).collect();
        // Newest first; the tree's order among equals (sort_by is stable).
        dated.sort_by_key(|d| std::cmp::Reverse(d.0));
        let items: Vec<Item> = dated
            .iter()
            .map(|(m, f)| {
                let age = m.and_then(|m| now.duration_since(m).ok()).map(|d| super::quickopen::age(d.as_secs())).unwrap_or_default();
                Item::file(f, age)
            })
            .collect();
        trace!("quick open: {} files dated in {} µs", items.len(), t.elapsed().as_micros());
        let paths: Vec<PathBuf> = dated.into_iter().map(|(_, f)| f).collect();
        let weak = self.weak();
        let qo = QuickOpen::new(super::quickopen::FILES, items, &scheme::palette().accent, move |i| {
            if let Some(w) = weak.upgrade() {
                w.open_file(&paths[i], None, None, true);
                w.web.grab_focus();
            }
        });
        self.present_picker(qo);
    }

    /// The outline (`o`): the open document's headings, to jump to one.
    pub fn outline(&self) {
        let (Some(root), Some(file)) = (self.root(), self.current.borrow().clone()) else { return };
        if !folder::is_markdown(&file) {
            self.status.note("only Markdown has an outline");
            return;
        }
        let heads = std::fs::read_to_string(root.join(&file)).map(|t| crate::render::headings(&t)).unwrap_or_default();
        if heads.is_empty() {
            self.status.note("no headings in this file");
            return;
        }
        let top = heads.iter().map(|h| h.0).min().unwrap_or(1);
        let items = heads
            .iter()
            .map(|(level, text, _)| Item { text: text.clone(), name_from: 0, aside: String::new(), indent: i32::from(level - top) })
            .collect();
        let ids: Vec<String> = heads.into_iter().map(|h| h.2).collect();
        let weak = self.weak();
        let qo = QuickOpen::new(super::quickopen::OUTLINE, items, &scheme::palette().accent, move |i| {
            if let Some(w) = weak.upgrade() {
                w.js(&format!("document.getElementById({})?.scrollIntoView({{block:'start',behavior:'instant'}})", js_string(&ids[i])));
                w.web.grab_focus();
            }
        });
        self.present_picker(qo);
    }

    fn present_picker(&self, qo: Rc<QuickOpen>) {
        // Noted first: the status bar asks which dialog it is as it appears.
        *self.quick.borrow_mut() = Some(qo.clone());
        let weak = self.weak();
        qo.dialog.connect_closed(move |_| {
            if let Some(w) = weak.upgrade() {
                // Not while the self-check still reads it; the next one replaces it anyway.
                if !super::debug::driven() {
                    w.quick.borrow_mut().take();
                }
            }
        });
        qo.present(&self.window);
    }

    /// `y`: the open file's full path to the clipboard.
    fn copy_path(&self) {
        let (Some(root), Some(file)) = (self.root(), self.current.borrow().clone()) else { return };
        let full = root.join(&file);
        self.window.clipboard().set_text(&full.to_string_lossy());
        self.status.note(&format!("copied {}", folder::tilde(&full)));
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
            const want = {at} === 0 ? ({dir} > 0 ? 0 : r.count - 1) : ((({at} - 1 + ({dir})) % r.count) + r.count) % r.count; \
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

    /// The tree this wide, within its limits and leaving the page room.
    pub fn set_tree_width(&self, px: f64) {
        let room = f64::from(self.window.width().max(0)) - 320.0;
        let max = f64::from(crate::settings::TREE_MAX).min(room).max(f64::from(crate::settings::TREE_MIN));
        let px = px.clamp(f64::from(crate::settings::TREE_MIN), max).round();
        // Max first when widening, min first when narrowing: never min > max.
        if px > self.split.max_sidebar_width() {
            self.split.set_max_sidebar_width(px);
            self.split.set_min_sidebar_width(px);
        } else {
            self.split.set_min_sidebar_width(px);
            self.split.set_max_sidebar_width(px);
        }
    }

    pub fn tree_width(&self) -> u32 {
        self.split.max_sidebar_width() as u32
    }

    /// Keep the tree's width for the next window, and every open one.
    fn keep_tree_width(&self) {
        let px = self.tree_width();
        let mut s = crate::settings::get();
        if s.tree_width != px {
            s.tree_width = px;
            crate::settings::set(s);
            super::apply_settings();
        }
    }

    /// `<` and `>`: the tree narrower or wider.
    fn step_tree_width(&self, by: f64) {
        if !self.split.shows_sidebar() {
            return;
        }
        self.set_tree_width(self.split.max_sidebar_width() + by);
        self.keep_tree_width();
        self.status.note(&format!("tree {} px", self.tree_width()));
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
        let was = self.applied.replace(Some(s.clone()));
        self.split.set_sidebar_position(if s.tree_left { gtk::PackType::Start } else { gtk::PackType::End });
        if s.tree_left {
            self.tree.widget.add_css_class("om-tree-left");
        } else {
            self.tree.widget.remove_css_class("om-tree-left");
        }
        self.tree_button.set_icon_name(if s.tree_left { "sidebar-show-symbolic" } else { "sidebar-show-right-symbolic" });
        self.tree_handle.set_halign(if s.tree_left { gtk::Align::End } else { gtk::Align::Start });
        if self.tree_width() != s.tree_width {
            self.set_tree_width(f64::from(s.tree_width));
        }
        self.status.show_hints(s.key_hints);
        // Only what changed: a new text size resets a passing Ctrl+plus, a
        // new skip rule walks the folder again, a new page width or picture
        // rule draws the page again.
        let differs = |f: fn(&crate::settings::Settings) -> String| was.as_ref().is_none_or(|w| f(w) != f(&s));
        if differs(|x| x.text_size.to_string()) {
            self.set_zoom(s.zoom());
        }
        if self.root.borrow().is_none() || was.is_none() {
            return;
        }
        if differs(|x| format!("{}{}{}", x.hidden, x.gitignore, x.html)) {
            self.rescan(false);
            if self.showing() == "empty" {
                self.empty_folder();
            }
        }
        if differs(|x| format!("{:?}{}", x.width, x.web_pictures)) {
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
