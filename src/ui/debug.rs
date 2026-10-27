//! The self-check (debug builds only): `OMAMV_SELFTEST=<report>` runs a
//! window through every key path with the same handler the keyboard uses,
//! edits and removes files on disk, switches the theme, takes pictures, and
//! writes what it saw to the report. No compositor, no input injection: run
//! it on GTK's Broadway display (`scripts/check.sh`).

/// A self-check is running: no hand-over to a background copy, no single
/// instance shared with the user's own omamv.
pub fn driven() -> bool {
    cfg!(debug_assertions) && std::env::var_os("OMAMV_SELFTEST").is_some()
}

#[cfg(debug_assertions)]
pub use imp::maybe_start;

#[cfg(not(debug_assertions))]
pub fn maybe_start(_: &std::rc::Rc<super::window::Win>) {}

#[cfg(debug_assertions)]
mod imp {
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use adw::prelude::*;
    use gtk::{gdk, glib};
    use webkit6::prelude::*;

    use super::super::window::Win;

    thread_local! {
        static STARTED: Cell<bool> = const { Cell::new(false) };
    }

    pub fn maybe_start(win: &Rc<Win>) {
        let Some(report) = std::env::var_os("OMAMV_SELFTEST").map(PathBuf::from) else { return };
        if STARTED.with(|s| s.replace(true)) {
            return;
        }
        glib::spawn_future_local(run(win.clone(), report));
    }

    struct Report {
        lines: Vec<String>,
        failed: usize,
    }

    impl Report {
        fn note(&mut self, k: &str, v: impl std::fmt::Display) {
            let line = format!("{k}: {v}");
            eprintln!("{line}");
            self.lines.push(line);
        }
        fn check(&mut self, what: &str, ok: bool, detail: impl std::fmt::Display) {
            if !ok {
                self.failed += 1;
            }
            let line = format!("{} {what} ({detail})", if ok { "ok  " } else { "FAIL" });
            eprintln!("{line}");
            self.lines.push(line);
        }
    }

    async fn sleep(ms: u64) {
        glib::timeout_future(Duration::from_millis(ms)).await;
    }

    async fn js(w: &Win, code: &str) -> String {
        match w.web.evaluate_javascript_future(code, None, None).await {
            Ok(v) => v.to_str().to_string(),
            Err(e) => format!("ERR {e}"),
        }
    }

    async fn wait_until(limit_ms: u64, mut f: impl FnMut() -> bool) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(limit_ms) {
            if f() {
                return true;
            }
            sleep(20).await;
        }
        false
    }

    async fn ready(w: &Win) -> bool {
        sleep(60).await;
        wait_until(15_000, || w.ready.get()).await
    }

    /// Do something that loads a page, and wait for that page to be drawn.
    async fn nav(w: &Win, f: impl FnOnce()) -> bool {
        let before = w.loads.get();
        f();
        wait_until(8_000, || w.loads.get() > before).await && wait_until(15_000, || w.ready.get()).await
    }

    fn key(w: &Win, k: gdk::Key, mods: gdk::ModifierType) -> bool {
        w.key(k, mods)
    }

    fn press(w: &Win, k: gdk::Key) -> bool {
        key(w, k, gdk::ModifierType::empty())
    }

    fn ctrl(w: &Win, k: gdk::Key) -> bool {
        key(w, k, gdk::ModifierType::CONTROL_MASK)
    }

    fn current(w: &Win) -> String {
        w.current.borrow().as_ref().map(|p| p.display().to_string()).unwrap_or_default()
    }

    /// A picture of the whole window, page included. GTK draws nothing for a
    /// window holding a web view here (no GL on Broadway), so the picture is
    /// put together from its parts, each drawn on its own at its place: the
    /// bars, the tree, the start screen, any dialog, and WebKit's own picture
    /// of the page, scrolled to where the reader is. A part that changed after
    /// the last frame GTK managed to draw (a dialog, the find bar, anything
    /// after a theme switch) can come out blank: Broadway has no GL, and a
    /// frame holding a web view fails there. The checks above do not depend
    /// on the pictures.
    async fn shot(w: &Win, file: &Path) -> String {
        sleep(400).await;
        let window: gtk::Widget = w.window.clone().upcast();
        let (width, height) = (window.width() as f32, window.height() as f32);
        if width < 2.0 {
            return "no size".into();
        }
        let snapshot = gtk::Snapshot::new();
        let bg = gdk::RGBA::parse(crate::scheme::palette().background.as_str()).unwrap_or(gdk::RGBA::BLACK);
        snapshot.append_color(&bg, &gtk::graphene::Rect::new(0.0, 0.0, width, height));
        let part = |snapshot: &gtk::Snapshot, widget: &gtk::Widget| {
            if !widget.is_drawable() {
                return;
            }
            let Some(b) = widget.compute_bounds(&window) else { return };
            snapshot.save();
            snapshot.translate(&gtk::graphene::Point::new(b.x(), b.y()));
            gtk::WidgetPaintable::new(Some(widget)).snapshot(snapshot, b.width() as f64, b.height() as f64);
            snapshot.restore();
        };
        if w.web.is_drawable()
            && let Some(b) = w.web.compute_bounds(&window)
        {
            let y = js(w, "String(window.scrollY)").await.parse::<f32>().unwrap_or(0.0);
            if let Ok(t) = w.web.snapshot_future(webkit6::SnapshotRegion::FullDocument, webkit6::SnapshotOptions::NONE).await {
                let s = w.web.scale_factor() as f32;
                snapshot.push_clip(&b);
                snapshot.append_texture(&t, &gtk::graphene::Rect::new(b.x(), b.y() - y, t.width() as f32 / s, t.height() as f32 / s));
                snapshot.pop();
            }
        }
        for widget in [
            w.header.clone().upcast::<gtk::Widget>(),
            w.findbar.clone().upcast(),
            w.banner.clone().upcast(),
            w.tree.widget.clone().upcast(),
            w.start.widget.clone().upcast(),
        ] {
            part(&snapshot, &widget);
        }
        // A dialog is drawn by the sheet that holds it: draw the highest
        // ancestor below libadwaita's dialog host, which also holds the page.
        if let Some(d) = w.window.visible_dialog() {
            let mut top: gtk::Widget = d.clone().upcast();
            while let Some(parent) = top.parent() {
                if parent.type_().name() == "AdwDialogHost" || parent == window {
                    break;
                }
                top = parent;
            }
            part(&snapshot, &top);
        }
        let Some(node) = snapshot.to_node() else { return "nothing drawn".into() };
        let renderer = gtk::gsk::CairoRenderer::new();
        if renderer.realize(None::<&gdk::Surface>).is_err() {
            return "no renderer".into();
        }
        let texture = renderer.render_texture(&node, Some(&gtk::graphene::Rect::new(0.0, 0.0, width, height)));
        renderer.unrealize();
        match texture.save_to_png(file) {
            Ok(()) => file.display().to_string(),
            Err(e) => format!("ERR {e}"),
        }
    }

    async fn run(w: Rc<Win>, report: PathBuf) {
        let t0 = Instant::now();
        let mut r = Report { lines: Vec::new(), failed: 0 };
        let shots = report.with_extension("shots");
        let _ = std::fs::create_dir_all(&shots);
        let root = loop {
            if let Some(root) = w.root() {
                break root;
            }
            sleep(20).await;
        };

        // Broadway's screen is 1024x768; a size request is the one thing that
        // outranks it, and the pictures want the design's 1280x800.
        w.window.set_size_request(1280, 800);

        // ---- open and draw
        let ok = ready(&w).await;
        sleep(1500).await;
        r.check("first page drawn", ok, format!("{} ms after the window", t0.elapsed().as_millis()));
        r.check("README opened first", current(&w) == "README.md", current(&w));
        r.check("tree lists the files", w.scan.borrow().order.len() >= 5, w.scan.borrow().order.len());
        let mermaid = js(&w, "String(document.querySelectorAll('figure.mermaid-drawn svg').length)").await;
        r.check("Mermaid drawn", mermaid == "2", &mermaid);
        let failed = js(&w, "String(document.querySelectorAll('pre.diagram-failed').length)").await;
        r.check("a broken diagram shows as code", failed == "1", &failed);
        let puml = js(&w, "String([...document.querySelectorAll('figure.plantuml img')].filter(i => i.naturalWidth > 0).length)").await;
        r.note("PlantUML drawn", &puml);
        let code = js(&w, "String(document.querySelectorAll('[class*=sy-]').length)").await;
        r.check("code highlighted", code.parse::<i32>().unwrap_or(0) > 5, &code);
        let alert = js(&w, "document.querySelector('.alert .alert-t')?.textContent || ''").await;
        r.check("alert has its title", alert == "Note", &alert);
        let tasks = js(&w, "String(document.querySelectorAll('.cb').length)").await;
        r.check("task boxes", tasks == "3", &tasks);
        let pwned = js(&w, "String(window.__pwned)").await;
        r.check("document scripts do not run", pwned == "undefined", &pwned);
        let img = js(&w, "String(document.querySelector('img[alt=dot]')?.naturalWidth)").await;
        r.check("local picture shown", img == "40", &img);
        r.note("shot reading", shot(&w, &shots.join("1-reading.png")).await);

        // ---- reading keys
        for _ in 0..6 {
            press(&w, gdk::Key::j);
        }
        press(&w, gdk::Key::d);
        sleep(150).await;
        let y = js(&w, "String(window.scrollY)").await;
        r.check("j and d scroll", y.parse::<f64>().unwrap_or(0.0) > 300.0, &y);
        r.check("header hairline while scrolled", w.header.has_css_class("om-scrolled"), "");
        press(&w, gdk::Key::G);
        sleep(100).await;
        let bottom = js(&w, "String(Math.round(window.scrollY + innerHeight) >= document.documentElement.scrollHeight - 2)").await;
        r.check("G goes to the end", bottom == "true", &bottom);
        press(&w, gdk::Key::g);
        press(&w, gdk::Key::g);
        sleep(100).await;
        let top = js(&w, "String(window.scrollY)").await;
        r.check("g g goes to the top", top == "0", &top);

        // ---- next / previous file, back / forward
        press(&w, gdk::Key::bracketleft);
        ready(&w).await;
        let after = current(&w);
        r.check("[ opens the previous file", after == "CONTRIBUTING.md", &after);
        press(&w, gdk::Key::J);
        ready(&w).await;
        r.check("J opens the next file", current(&w) == "README.md", current(&w));
        key(&w, gdk::Key::Left, gdk::ModifierType::ALT_MASK);
        ready(&w).await;
        r.check("Alt+Left goes back", current(&w) == after, current(&w));
        key(&w, gdk::Key::Right, gdk::ModifierType::ALT_MASK);
        ready(&w).await;
        r.check("Alt+Right goes forward", current(&w) == "README.md", current(&w));

        // ---- quick open
        ctrl(&w, gdk::Key::p);
        sleep(300).await;
        r.check("Ctrl+P opens quick open", w.window.visible_dialog().is_some(), "");
        let qo = w.quick.borrow().clone();
        if let Some(qo) = qo {
            let empty = qo.matches("");
            r.check("empty query starts with files just read", empty.first().is_some_and(|(p, _)| p == Path::new("README.md")), format!("{:?}", empty.first().map(|m| &m.0)));
            qo.entry.set_text("arch");
            sleep(300).await;
            r.note("shot quick open", shot(&w, &shots.join("2-quick-open.png")).await);
            qo.entry.emit_activate();
            ready(&w).await;
        }
        r.check("Enter opens the match", current(&w) == "docs/architecture.md", current(&w));
        r.check("the dialog closed", w.window.visible_dialog().is_none(), "");

        // ---- links between files
        let page = w.web.clone();
        nav(&w, move || page.evaluate_javascript("document.querySelector('a[href*=\"README\"]').click()", None, None, gtk::gio::Cancellable::NONE, |_| {})).await;
        r.check("a link to a file opens it here", current(&w) == "README.md", current(&w));
        let page = w.web.clone();
        nav(&w, move || page.evaluate_javascript("document.querySelector('a[href*=\"other.md#second\"]').click()", None, None, gtk::gio::Cancellable::NONE, |_| {})).await;
        sleep(300).await;
        let hash_y = js(&w, "String(window.scrollY > 0 || document.documentElement.scrollHeight <= innerHeight)").await;
        r.check("a link with #heading lands on it", current(&w) == "docs/other.md" && hash_y == "true", format!("{} {hash_y}", current(&w)));
        key(&w, gdk::Key::Left, gdk::ModifierType::ALT_MASK);
        ready(&w).await;

        // ---- tree
        ctrl(&w, gdk::Key::b);
        sleep(200).await;
        r.check("Ctrl+B gives the tree the keyboard", w.tree.has_focus(), "");
        r.note("shot tree", shot(&w, &shots.join("3-tree.png")).await);
        press(&w, gdk::Key::Home);
        press(&w, gdk::Key::j);
        sleep(50).await;
        let row = w.tree.cursor.get();
        r.check("j moves in the tree", row == 1, row);
        press(&w, gdk::Key::k);
        press(&w, gdk::Key::h);
        sleep(50).await;
        let folded = w.tree.row(0).is_some_and(|r| !r.is_expanded());
        r.check("h folds a folder", folded, "");
        press(&w, gdk::Key::l); // unfold docs
        press(&w, gdk::Key::l); // into it: adr
        press(&w, gdk::Key::l); // adr opens again folded: unfold it
        press(&w, gdk::Key::l); // into adr: its first file
        nav(&w, || {
            press(&w, gdk::Key::Return);
        })
        .await;
        r.check("Enter in the tree opens a file", current(&w) == "docs/adr/0001-open-a-folder.md", current(&w));
        r.check("the page has the keyboard again", !w.tree.has_focus(), "");
        ctrl(&w, gdk::Key::b);
        ctrl(&w, gdk::Key::b);
        sleep(300).await;
        r.check("Ctrl+B twice hides the tree", !w.split.shows_sidebar(), "");
        ctrl(&w, gdk::Key::b);
        press(&w, gdk::Key::Escape);
        sleep(100).await;
        r.check("Esc in the tree returns to the page", !w.tree.has_focus() && w.split.shows_sidebar(), "");
        w.open_file(Path::new("README.md"), None, None, true);
        ready(&w).await;

        // ---- find
        ctrl(&w, gdk::Key::f);
        w.find_entry.set_text("folder");
        sleep(400).await;
        let count = w.find_count.label().to_string();
        r.check("find counts matches", count.ends_with(" of 4") || count.contains(" of "), &count);
        r.note("shot find", shot(&w, &shots.join("4-find.png")).await);
        w.find_entry.emit_activate();
        sleep(150).await;
        let count2 = w.find_count.label().to_string();
        r.check("Enter goes to the next match", count2.starts_with('2'), &count2);
        w.find_entry.emit_by_name::<()>("stop-search", &[]);
        sleep(200).await;
        let marks = js(&w, "String(document.querySelectorAll('mark.om-find').length)").await;
        r.check("Esc closes find and clears the marks", !w.findbar.is_search_mode() && marks == "0", &marks);
        w.web.grab_focus();
        press(&w, gdk::Key::n);
        sleep(300).await;
        let cur = js(&w, "String(document.querySelectorAll('mark.om-find.cur').length)").await;
        r.check("n finds again after Esc", cur == "1", &cur);
        press(&w, gdk::Key::slash);
        sleep(100).await;
        r.check("/ opens find", w.findbar.is_search_mode(), "");
        w.find_entry.emit_by_name::<()>("stop-search", &[]);
        w.web.grab_focus();

        // ---- keys overview
        press(&w, gdk::Key::question);
        sleep(300).await;
        r.check("? shows the keys", w.window.visible_dialog().is_some(), "");
        r.note("shot keys", shot(&w, &shots.join("5-keys.png")).await);
        if let Some(d) = w.window.visible_dialog() {
            d.close();
        }
        sleep(300).await;

        // ---- the disk: an edit keeps the place
        for _ in 0..12 {
            press(&w, gdk::Key::j);
        }
        sleep(150).await;
        let y_before = js(&w, "String(Math.round(window.scrollY))").await;
        let readme = root.join("README.md");
        let text = std::fs::read_to_string(&readme).unwrap_or_default();
        let edited = text.replacen("A paragraph", "An edited paragraph that is a good deal longer than before, to push everything below it down the page a little, as edits do.\n\nA paragraph", 1);
        let loads = w.loads.get();
        let tmp = readme.with_extension("md.swp~");
        let _ = std::fs::write(&tmp, &edited);
        let saved = Instant::now();
        let _ = std::fs::rename(&tmp, &readme);
        let reloaded = wait_until(5_000, || w.loads.get() > loads).await && ready(&w).await;
        r.check("a save reloads the page", reloaded, format!("{} ms", saved.elapsed().as_millis()));
        let y_after = js(&w, "String(Math.round(window.scrollY))").await;
        let shown = js(&w, "String(document.body.innerText.includes('An edited paragraph'))").await;
        r.check("the new text is shown", shown == "true", &shown);
        let moved = y_after.parse::<i64>().unwrap_or(0) - y_before.parse::<i64>().unwrap_or(0);
        r.check("the place is kept by heading, not pixels", moved > 20, format!("{y_before} -> {y_after}"));
        r.check("subtitle says updated", w.changed.is_visible(), w.changed.text());

        // ---- the disk: removed, then back
        let _ = std::fs::remove_file(&readme);
        let gone = wait_until(3_000, || w.gone.get()).await;
        r.check("a removed file is marked gone", gone && w.banner.is_revealed(), "");
        sleep(700).await;
        let faded = js(&w, "String(document.body.classList.contains('gone'))").await;
        r.check("its last version stays, faded", faded == "true", &faded);
        r.check("the tree dropped it", !w.scan.borrow().order.contains(&PathBuf::from("README.md")), "");
        r.note("shot gone", shot(&w, &shots.join("6-gone.png")).await);
        let _ = std::fs::write(&readme, &text);
        let back = wait_until(3_000, || !w.gone.get()).await;
        r.check("it comes back when written again", back && !w.banner.is_revealed(), "");
        ready(&w).await;

        // ---- a new file appears in the tree
        wait_until(3_000, || w.scan.borrow().order.contains(&PathBuf::from("README.md"))).await;
        let n_before = w.scan.borrow().order.len();
        let _ = std::fs::write(root.join("docs/new-note.md"), "# New\n");
        let grew = wait_until(3_000, || w.scan.borrow().order.len() == n_before + 1).await;
        r.check("a new file appears in the tree", grew, w.scan.borrow().order.len());

        // ---- the theme follows Omarchy
        if let Some(state) = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from) {
            let current = state.join("omarchy/current");
            let next = current.join("next-theme");
            let _ = std::fs::create_dir_all(&next);
            let _ = std::fs::write(next.join("colors.toml"), LATTE);
            let _ = std::fs::remove_dir_all(current.join("theme"));
            let _ = std::fs::rename(&next, current.join("theme"));
            let _ = std::fs::write(current.join("theme.name"), "catppuccin-latte\n");
            let switched = wait_until(4_000, || crate::scheme::palette().background == "#eff1f5").await;
            r.check("a theme switch repaints", switched, crate::scheme::palette().background.clone());
            ready(&w).await;
            sleep(300).await;
            let bg = js(&w, "getComputedStyle(document.documentElement).getPropertyValue('--bg').trim()").await;
            r.check("the page takes the new palette", bg == "#eff1f5", &bg);
            r.note("shot light", shot(&w, &shots.join("7-light.png")).await);
        }

        // ---- start screen
        ctrl(&w, gdk::Key::O);
        key(&w, gdk::Key::O, gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
        sleep(200).await;
        r.note("shot start", shot(&w, &shots.join("8-start.png")).await);
        press(&w, gdk::Key::Escape);
        sleep(100).await;

        r.note("failed", r.failed);
        r.note("took ms", t0.elapsed().as_millis());
        let _ = std::fs::write(&report, r.lines.join("\n") + "\n");
        if let Some(app) = w.window.application() {
            app.quit();
        }
    }

    const LATTE: &str = r##"mode = "light"
accent = "#1e66f5"
muted = "#acb0be"
background = "#eff1f5"
lighter_background = "#dce0e8"
foreground = "#4c4f69"
dark_foreground = "#9ca0b0"
red = "#d20f39"
yellow = "#df8e1d"
green = "#40a02b"
cyan = "#179299"
blue = "#1e66f5"
magenta = "#ea76cb"
"##;
}
