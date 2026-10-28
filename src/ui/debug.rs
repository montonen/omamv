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

/// A debug run of its own, never handing over to (or taking over) the
/// user's running omamv: the self-check, or `OMAMV_ALONE=1`.
pub fn alone() -> bool {
    driven() || (cfg!(debug_assertions) && std::env::var_os("OMAMV_ALONE").is_some())
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

    use super::super::status::Mode;
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
            w.status.widget.clone().upcast(),
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

        // The app, not this check, must keep the window's state: with only
        // the check holding it, every handler in a real launch finds it gone
        // (the empty window of 2026-10-27).
        r.check("the app keeps the window", Rc::strong_count(&w) >= 2, Rc::strong_count(&w));

        // ---- open and draw
        let ok = ready(&w).await;
        sleep(1500).await;
        r.check("first page drawn", ok, format!("{} ms after the window", t0.elapsed().as_millis()));
        r.check("README opened first", current(&w) == "README.md", current(&w));
        r.check("tree lists the files", w.scan.borrow().order.len() >= 5, w.scan.borrow().order.len());
        let mermaid = js(&w, "String(document.querySelectorAll('figure.mermaid-drawn svg').length)").await;
        r.check("Mermaid drawn", mermaid == "2", &mermaid);
        // Without PlantUML (CI's container) its diagrams say so instead; that is not this check.
        let failed = js(&w, "String([...document.querySelectorAll('pre.diagram-failed')].filter(p => !(p.dataset.error || '').includes('not installed')).length)").await;
        r.check("a broken diagram shows as code", failed == "1", &failed);
        let leftover = js(&w, "String(document.body.innerText.includes('Syntax error in text'))").await;
        r.check("  and Mermaid leaves no error drawing at the end of the page", leftover == "false", &leftover);
        let missing = js(&w, "String(document.querySelectorAll('pre.diagram-failed[data-error*=\"not installed\"]').length)").await;
        if missing != "0" {
            r.note("PlantUML not installed here; its diagrams say so", &missing);
        }
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
        let closed = (0..20).filter_map(|i| w.tree.row(i)).all(|r| !r.is_expanded());
        r.check("folders start closed (the open file is at the top)", closed, "");
        let line = w.tree.left_out_text().unwrap_or_default();
        r.check("the tree says what .gitignore left out", line == "1 folder left out by .gitignore", &line);
        let note = w.status.shown_note().unwrap_or_default();
        r.check("the load time is shown", note.contains("files read in") && note.contains("first page in"), &note);
        w.update_status();
        r.check("status bar: page keys", w.status.mode() == Some(Mode::Page) && w.status.text().contains("Ctrl+P"), w.status.text());

        // ---- held modifiers list their shortcuts
        let hold = |m: gdk::ModifierType| w.modifiers(m);
        let wait = crate::ui::window::HELD_MS + 200;
        hold(gdk::ModifierType::CONTROL_MASK);
        sleep(100).await;
        hold(gdk::ModifierType::empty());
        sleep(wait).await;
        r.check("a quick Ctrl tap leaves the bar alone", w.status.held().is_none() && w.status.text().contains("Ctrl+P"), w.status.text());
        hold(gdk::ModifierType::CONTROL_MASK);
        r.check("  not at once", w.status.held().is_none(), "");
        sleep(wait).await;
        r.check("Ctrl held lists its shortcuts", w.status.held().as_deref() == Some("Ctrl") && w.status.text().contains("P open a file") && !w.status.text().contains("j k"), w.status.text());
        sleep(600).await;
        r.note("shot ctrl held", shot(&w, &shots.join("1b-ctrl-held.png")).await);
        r.check("  more than fits: whole ones shown, the rest counted", w.status.left_out() > 0, w.status.left_out());
        hold(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
        r.check("  Shift added: Ctrl+Shift's", w.status.held().as_deref() == Some("Ctrl+Shift") && w.status.text().contains("O recent folders") && w.status.text().contains("S the whole page as a picture"), w.status.text());
        hold(gdk::ModifierType::empty());
        r.check("  let go: the page keys again", w.status.held().is_none() && w.status.text().contains("j k ↓ ↑ scroll"), w.status.text());
        let mut s = crate::settings::get();
        s.key_hints = false;
        crate::settings::set(s);
        super::super::apply_settings();
        hold(gdk::ModifierType::ALT_MASK);
        sleep(wait).await;
        let shown = w.status.hints_shown() && w.status.text().contains("back / forward");
        hold(gdk::ModifierType::empty());
        r.check("  with key hints off too, and hidden again after", shown && !w.status.hints_shown(), w.status.text());
        crate::settings::set(crate::settings::Settings::default());
        super::super::apply_settings();

        // ---- reading keys
        for _ in 0..6 {
            press(&w, gdk::Key::j);
        }
        press(&w, gdk::Key::d);
        sleep(150).await;
        let y = js(&w, "String(window.scrollY)").await;
        r.check("j and d scroll", y.parse::<f64>().unwrap_or(0.0) > 300.0, &y);
        r.check("header hairline while scrolled", w.header.has_css_class("om-scrolled"), "");
        let before = js(&w, "String(window.scrollY)").await.parse::<f64>().unwrap_or(0.0);
        press(&w, gdk::Key::Down);
        press(&w, gdk::Key::Down);
        sleep(150).await;
        let after = js(&w, "String(window.scrollY)").await.parse::<f64>().unwrap_or(0.0);
        r.check("↓ scrolls the page, as j does", after > before, format!("{before} -> {after}"));
        press(&w, gdk::Key::Up);
        sleep(150).await;
        let up = js(&w, "String(window.scrollY)").await.parse::<f64>().unwrap_or(0.0);
        r.check("↑ scrolls back", up < after, format!("{after} -> {up}"));
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
        press(&w, gdk::Key::H);
        ready(&w).await;
        r.check("H goes back too (ADR-0020)", current(&w) == after, current(&w));
        press(&w, gdk::Key::L);
        ready(&w).await;
        r.check("L goes forward", current(&w) == "README.md", current(&w));
        press(&w, gdk::Key::L);
        r.check("  at the newest, it says so", w.status.shown_note().as_deref() == Some("no later file") && current(&w) == "README.md", format!("{:?}", w.status.shown_note()));

        // ---- quick open
        // An agent rewrote this one just now: it comes first (ADR-0017).
        if let Ok(f) = std::fs::File::options().write(true).open(root.join("notes/todo.txt")) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
        ctrl(&w, gdk::Key::p);
        sleep(300).await;
        if let Some(qo) = w.quick.borrow().clone() {
            qo.entry.emit_by_name::<()>("stop-search", &[]);
        }
        sleep(300).await;
        r.check("Esc closes quick open", w.window.visible_dialog().is_none(), "");
        ctrl(&w, gdk::Key::p);
        sleep(300).await;
        r.check("Ctrl+P opens quick open", w.window.visible_dialog().is_some(), "");
        r.check("status bar: quick open keys", w.status.mode() == Some(Mode::QuickOpen), format!("{:?}", w.status.mode()));
        let qo = w.quick.borrow().clone();
        if let Some(qo) = qo {
            let empty = qo.texts("");
            r.check("empty query starts with the newest file", empty.first().is_some_and(|p| p == "notes/todo.txt"), format!("{:?}", empty.first()));
            qo.entry.set_text("arch");
            sleep(300).await;
            r.note("shot quick open", shot(&w, &shots.join("2-quick-open.png")).await);
            qo.entry.emit_activate();
            ready(&w).await;
        }
        r.check("Enter opens the match", current(&w) == "docs/architecture.md", current(&w));
        r.check("the dialog closed", w.window.visible_dialog().is_none(), "");

        // ---- the outline, copy the path, a new file (ADR-0017)
        let heads = crate::render::headings(&std::fs::read_to_string(root.join("docs/architecture.md")).unwrap_or_default());
        press(&w, gdk::Key::o);
        sleep(300).await;
        let qo = w.quick.borrow().clone();
        r.check("o opens the outline", w.keyboard_mode() == Mode::QuickOpen && qo.is_some(), format!("{:?}", w.keyboard_mode()));
        if let Some(qo) = qo {
            let texts = qo.texts("");
            r.check("the outline lists the headings in order", !heads.is_empty() && texts == heads.iter().map(|h| h.1.clone()).collect::<Vec<_>>(), format!("{texts:?}"));
            if let Some(last) = heads.last() {
                qo.entry.set_text(&last.1);
                sleep(200).await;
                qo.entry.emit_activate();
            }
            sleep(300).await;
            r.check("a heading picked closes the outline", w.window.visible_dialog().is_none() && current(&w) == "docs/architecture.md", current(&w));
        }
        press(&w, gdk::Key::y);
        r.check("y copies the file's path", w.status.shown_note().is_some_and(|n| n.starts_with("copied ") && n.ends_with("docs/architecture.md")), format!("{:?}", w.status.shown_note()));
        // ---- the open file in git (ADR-0022), read-only
        let git = |args: &[&str]| {
            let _ = std::process::Command::new("git").arg("-C").arg(&root).args(args).output();
        };
        let label = || if w.git_label.is_visible() { w.git_label.label().to_string() } else { String::new() };
        let shown = |want: fn(&str) -> bool| wait_until(3_000, move || want(&label()));
        let clean = shown(|t| (t.ends_with(" ago") || t.ends_with(" just now")) && !t.contains("modified") && !t.contains("staged")).await;
        r.check("a file in git shows its last commit", clean, label());
        let arch = root.join("docs/architecture.md");
        let text = std::fs::read_to_string(&arch).unwrap_or_default();
        let _ = std::fs::write(&arch, format!("{text}\nOne more line.\n"));
        r.check("  written: modified", shown(|t| t.starts_with("modified · ")).await, label());
        git(&["add", "docs/architecture.md"]);
        r.check("  git add: staged, seen through the repository's own folder", shown(|t| t.starts_with("staged · ")).await, label());
        git(&["reset", "-q", "docs/architecture.md"]);
        git(&["checkout", "-q", "--", "docs/architecture.md"]);
        r.check("  put back: clean again", shown(|t| (t.ends_with(" ago") || t.ends_with(" just now")) && !t.contains("modified") && !t.contains("staged")).await, label());
        r.check("  and omamv wrote nothing into the repository", std::fs::read_to_string(&arch).unwrap_or_default() == text, "");
        ready(&w).await;

        // ---- the page as one tall picture (ADR-0023)
        let pictures = report.parent().map(|d| d.join("pictures/omamv")).unwrap_or_default();
        nav(&w, || w.open_file(Path::new("README.md"), None, None, true)).await;
        sleep(800).await;
        key(&w, gdk::Key::S, gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
        let saved = wait_until(5_000, || std::fs::read_dir(&pictures).is_ok_and(|mut d| d.next().is_some())).await;
        let png = std::fs::read_dir(&pictures).ok().and_then(|mut d| d.next()).and_then(|e| e.ok()).map(|e| e.path());
        let size = png.as_ref().and_then(|p| std::fs::read(p).ok()).filter(|b| b.len() > 24 && b.starts_with(b"\x89PNG")).map(|b| {
            let n = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
            (n(16), n(20))
        });
        let page_h = js(&w, "String(document.documentElement.scrollHeight)").await.parse::<u32>().unwrap_or(0);
        r.check(
            "Ctrl+Shift+S saves the whole page as one picture, taller than the window",
            // The page less the room for scrolling past its end (40 % of the window).
            saved && size.is_some_and(|(pw, ph)| ph > w.web.height() as u32 && ph < page_h && ph + (w.web.height() as u32 * 2 / 5) >= page_h && pw <= w.web.width() as u32 + 4),
            format!("{:?} {size:?} page {page_h}", png.as_ref().and_then(|p| p.file_name())),
        );
        let after = js(&w, "String(document.body.classList.contains('om-shot') + ' ' + document.documentElement.scrollHeight)").await;
        r.check("  the page is as it was afterwards", after == format!("false {page_h}"), &after);
        r.check("  and says where it went", w.status.shown_note().is_some_and(|n| n.starts_with("page saved as ") && n.contains("copied")), format!("{:?}", w.status.shown_note()));
        nav(&w, || w.open_file(Path::new("docs/architecture.md"), None, None, true)).await;

        // e: a stand-in editor that writes down what it was given, outside the folder.
        let handed = report.with_extension("edited");
        let mut s = crate::settings::get();
        s.editor = format!("sh -c 'printf %s \"$0\" > {}'", handed.display());
        crate::settings::set(s);
        press(&w, gdk::Key::e);
        wait_until(3_000, || handed.is_file()).await;
        let given = std::fs::read_to_string(&handed).unwrap_or_default();
        r.check("e hands the open file to the editor set", given == root.join("docs/architecture.md").to_string_lossy() && w.status.shown_note().as_deref() == Some("opened in sh"), format!("{given:?} {:?}", w.status.shown_note()));
        let mut s = crate::settings::get();
        s.editor = "no-such-editor-omamv".into();
        crate::settings::set(s);
        press(&w, gdk::Key::e);
        r.check("  an editor that is not there says so", w.status.shown_note().is_some_and(|n| n.contains("no-such-editor-omamv not found")), format!("{:?}", w.status.shown_note()));
        crate::settings::set(crate::settings::Settings::default());
        super::super::apply_settings();
        let report_md = root.join("notes/agent-report.md");
        let _ = std::fs::write(&report_md, "# Agent report\n");
        let named = wait_until(4_000, || w.status.shown_note().is_some_and(|n| n.contains("notes/agent-report.md"))).await;
        r.check("a new file is named in the status bar", named, format!("{:?}", w.status.shown_note()));
        let _ = std::fs::remove_file(&report_md);
        let dropped = wait_until(4_000, || !w.scan.borrow().order.iter().any(|f| f.ends_with("agent-report.md"))).await;
        r.check("and dropped again when removed", dropped, "");

        // ---- an HTML report (ADR-0016): off by default, then on
        let html_page = PathBuf::from("docs/report.html");
        r.check("HTML pages are off by default: not listed", !w.scan.borrow().order.contains(&html_page), "");
        let mut s = crate::settings::get();
        s.html = true;
        crate::settings::set(s);
        super::super::apply_settings();
        let listed = wait_until(3_000, || w.scan.borrow().order.contains(&html_page)).await;
        r.check("turned on, they are listed", listed, "");
        nav(&w, || w.open_file(Path::new("docs/report.html"), None, None, true)).await;
        sleep(300).await;
        let h1 = js(&w, "document.querySelector('h1')?.textContent || ''").await;
        r.check("an HTML page is shown", h1 == "Scan report", &h1);
        let pwned = js(&w, "String(window.__pwned) + ' ' + document.title").await;
        r.check("its scripts do not run", pwned == "undefined Scan report", &pwned);
        let lead = js(&w, "getComputedStyle(document.querySelector('.lead')).color").await;
        r.check("its own stylesheet applies", lead == "rgb(1, 2, 3)", &lead);
        let img = js(&w, "String(document.querySelector('img[alt=dot]')?.naturalWidth)").await;
        r.check("its pictures from the folder show", img == "40", &img);
        let helper = js(&w, "typeof omamv").await;
        r.check("omamv's keys work on it", helper == "object", &helper);
        r.note("shot html", shot(&w, &shots.join("2b-html.png")).await);
        let page = w.web.clone();
        nav(&w, move || page.evaluate_javascript("document.querySelector('a[href*=\"README\"]').click()", None, None, gtk::gio::Cancellable::NONE, |_| {})).await;
        r.check("a link from it to Markdown opens here", current(&w) == "README.md", current(&w));
        crate::settings::set(crate::settings::Settings::default());
        super::super::apply_settings();
        let unlisted = wait_until(3_000, || !w.scan.borrow().order.contains(&html_page)).await;
        r.check("turned off again, they are gone from the tree", unlisted, "");
        nav(&w, || w.open_file(Path::new("docs/architecture.md"), None, None, true)).await;

        // ---- links between files
        let page = w.web.clone();
        nav(&w, move || page.evaluate_javascript("document.querySelector('a[href*=\"README\"]').click()", None, None, gtk::gio::Cancellable::NONE, |_| {})).await;
        r.check("a link to a file opens it here", current(&w) == "README.md", current(&w));
        let page = w.web.clone();
        nav(&w, move || page.evaluate_javascript("document.querySelector('a[href*=\"other.md#second\"]').click()", None, None, gtk::gio::Cancellable::NONE, |_| {})).await;
        sleep(300).await;
        let hash_y = js(&w, "String(window.scrollY > 0 || document.documentElement.scrollHeight <= innerHeight)").await;
        r.check("a link with #heading lands on it", current(&w) == "docs/other.md" && hash_y == "true", format!("{} {hash_y}", current(&w)));
        // ---- wikilinks (ADR-0018)
        let wiki = js(&w, "[...document.querySelectorAll('.page a')].map(a => a.getAttribute('href')).join(' ') + ' | ' + document.querySelectorAll('.om-wiki-missing').length + ' | ' + document.body.innerText.includes('[[')").await;
        r.check(
            "wikilinks are links: by name, to a heading elsewhere, on the page; a dead one is marked",
            wiki.contains("/docs/architecture.md ") && wiki.contains("/README.md#open-a-folder") && wiki.contains("#second") && wiki.ends_with("| 1 | false"),
            &wiki,
        );
        let note = root.join("notes/Not here.md");
        let _ = std::fs::write(&note, "# Here now\n");
        let mut alive = String::new();
        for _ in 0..40 {
            sleep(100).await;
            alive = js(&w, "String(document.querySelectorAll('.om-wiki-missing').length + ' ' + !!document.querySelector('a[href=\"/notes/Not%20here.md\"]'))").await;
            if alive == "0 true" {
                break;
            }
        }
        r.check("  a note made later brings its dead link to life", alive == "0 true", &alive);
        let _ = std::fs::remove_file(&note);
        wait_until(4_000, || !w.scan.borrow().order.iter().any(|f| f.ends_with("Not here.md"))).await;
        ready(&w).await;
        let page = w.web.clone();
        nav(&w, move || page.evaluate_javascript("document.querySelector('a[href=\"/README.md#open-a-folder\"]').click()", None, None, gtk::gio::Cancellable::NONE, |_| {})).await;
        sleep(300).await;
        let y = js(&w, "String(Math.round(document.getElementById('open-a-folder').getBoundingClientRect().top))").await;
        r.check("  a wikilink opens its file at the heading", current(&w) == "README.md" && y.parse::<i32>().is_ok_and(|t| (0..80).contains(&t)), format!("{} top {y}", current(&w)));
        key(&w, gdk::Key::Left, gdk::ModifierType::ALT_MASK);
        ready(&w).await;
        key(&w, gdk::Key::Left, gdk::ModifierType::ALT_MASK);
        ready(&w).await;

        // ---- tree
        ctrl(&w, gdk::Key::b);
        sleep(200).await;
        r.check("Ctrl+B gives the tree the keyboard", w.tree.has_focus(), "");
        r.check("status bar: tree keys", w.status.mode() == Some(Mode::Tree) && w.status.text().contains("h l"), w.status.text());
        r.check("the tree's cursor is drawn on the open file", w.tree.cursor_label().as_deref() == Some(current(&w).rsplit('/').next().unwrap_or("")), format!("{:?}", w.tree.cursor_label()));
        r.note("shot tree", shot(&w, &shots.join("3-tree.png")).await);
        let open = |i: u32| w.tree.row(i).is_some_and(|r| r.is_expanded());
        press(&w, gdk::Key::Home);
        if !open(0) {
            press(&w, gdk::Key::l);
        }
        press(&w, gdk::Key::h);
        r.check("h folds a folder", !open(0), "");
        press(&w, gdk::Key::l);
        r.check("l unfolds it", open(0), "");
        press(&w, gdk::Key::Down);
        r.check("↓ moves in the tree", w.tree.cursor() == 1, w.tree.cursor());
        r.check("and the cursor is drawn there", w.tree.cursor_label().as_deref() == Some("adr"), format!("{:?}", w.tree.cursor_label()));
        press(&w, gdk::Key::Up);
        r.check("↑ moves back", w.tree.cursor() == 0, w.tree.cursor());
        press(&w, gdk::Key::j);
        let row = w.tree.cursor();
        r.check("j moves in the tree", row == 1, row);
        if !open(1) {
            press(&w, gdk::Key::Right);
        }
        r.check("→ unfolds", open(1), "");
        press(&w, gdk::Key::Right);
        r.check("→ again goes into it", w.tree.cursor_label().as_deref() == Some("0001-open-a-folder.md"), format!("{:?}", w.tree.cursor_label()));
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
        sleep(300).await;
        r.check("Ctrl+B with the tree hidden shows it and gives it the keyboard", w.split.shows_sidebar() && w.tree.has_focus() && w.keyboard_mode() == Mode::Tree, format!("{:?}", w.keyboard_mode()));
        press(&w, gdk::Key::Escape);
        sleep(100).await;
        r.check("Esc in the tree returns to the page", !w.tree.has_focus() && w.split.shows_sidebar(), "");

        // ---- the tree's width
        r.check("the tree starts at the usual width", w.tree_width() == crate::settings::TREE_DEFAULT, w.tree_width());
        press(&w, gdk::Key::greater);
        r.check("> widens the tree, and it is kept", w.tree_width() == crate::settings::TREE_DEFAULT + 32 && crate::settings::get().tree_width == w.tree_width(), w.tree_width());
        press(&w, gdk::Key::less);
        press(&w, gdk::Key::less);
        r.check("< narrows it", w.tree_width() == crate::settings::TREE_DEFAULT - 32, w.tree_width());
        w.set_tree_width(5_000.0);
        let widest = w.tree_width();
        r.check("  never wider than its limit, and the page keeps room", widest <= crate::settings::TREE_MAX && i64::from(widest) <= i64::from(w.window.width()) - 320, format!("{widest} of {}", w.window.width()));
        w.set_tree_width(10.0);
        r.check("  never narrower than its limit", w.tree_width() == crate::settings::TREE_MIN, w.tree_width());
        let mut s = crate::settings::get();
        s.tree_width = 400;
        crate::settings::set(s);
        super::super::apply_settings();
        wait_until(2_000, || (396..=404).contains(&w.tree.widget.width())).await;
        let laid = w.tree.widget.width();
        let mut l = crate::settings::get();
        l.tree_left = true;
        crate::settings::set(l);
        super::super::apply_settings();
        let at_edge = || w.tree_handle.halign() == gtk::Align::End && w.tree_handle.compute_bounds(&w.tree.widget).is_some_and(|b| (b.x() + b.width() - w.tree.widget.width() as f32).abs() < 2.0);
        let edge = wait_until(2_000, at_edge).await;
        let where_ = w.tree_handle.compute_bounds(&w.tree.widget).map(|b| format!("{:?} x {} w {} of {}", w.tree_handle.halign(), b.x(), b.width(), w.tree.widget.width()));
        r.check("  on the left, the drag edge faces the page", edge, format!("{where_:?}"));
        let mut l = crate::settings::get();
        l.tree_left = false;
        crate::settings::set(l);
        super::super::apply_settings();
        sleep(200).await;
        r.check("a width from Settings applies to the window", w.tree_width() == 400 && (396..=404).contains(&laid), format!("{} laid out {laid}", w.tree_width()));
        r.note("shot wide tree", shot(&w, &shots.join("3b-wide-tree.png")).await);
        crate::settings::set(crate::settings::Settings::default());
        super::super::apply_settings();
        // One click on a file opens it (ADR-0013).
        let row = w.tree.position_of("CONTRIBUTING.md");
        nav(&w, || {
            if let Some(row) = row {
                w.tree.click(row);
            }
        })
        .await;
        r.check("one click on a file opens it", current(&w) == "CONTRIBUTING.md", current(&w));
        let notes = w.tree.position_of("notes");
        let open_before = notes.and_then(|p| w.tree.row(p)).is_some_and(|r| r.is_expanded());
        if let Some(p) = notes {
            w.tree.click(p);
        }
        let open_after = notes.and_then(|p| w.tree.row(p)).is_some_and(|r| r.is_expanded());
        r.check("one click on a folder opens or closes it", open_before != open_after, format!("{open_before} -> {open_after}"));
        if let Some(p) = notes {
            w.tree.click(p);
        }
        w.open_file(Path::new("README.md"), None, None, true);
        ready(&w).await;

        // ---- find
        ctrl(&w, gdk::Key::f);
        w.find_entry.set_text("folder");
        sleep(400).await;
        r.check("status bar: find keys", w.status.mode() == Some(Mode::Find), format!("{:?}", w.status.mode()));
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
        r.check("status bar: Esc closes it", w.status.mode() == Some(Mode::Dialog), format!("{:?}", w.status.mode()));
        r.note("shot keys", shot(&w, &shots.join("5-keys.png")).await);
        if let Some(d) = w.window.visible_dialog() {
            d.close();
        }
        sleep(300).await;

        // ---- the disk: a save in place reloads the page, and walks nothing
        let scans = w.scans.get();
        let loads = w.loads.get();
        let readme_text = std::fs::read_to_string(root.join("README.md")).unwrap_or_default();
        let _ = std::fs::write(root.join("README.md"), &readme_text);
        let reloaded = wait_until(4_000, || w.loads.get() > loads).await;
        sleep(600).await;
        r.check("a save reloads the page without walking the folder", reloaded && w.scans.get() == scans, format!("walks {scans} -> {}", w.scans.get()));
        let _ = std::fs::write(root.join("build/churn.md"), "# Build output\n");
        sleep(800).await;
        r.check("an ignored folder is not watched", w.scans.get() == scans, format!("walks {scans} -> {}", w.scans.get()));
        ready(&w).await;

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
        let mut s = crate::settings::get();
        s.width = crate::settings::Width::Wide;
        crate::settings::set(s);
        super::super::apply_settings();
        sleep(500).await;
        let still = js(&w, "String(document.body.classList.contains('gone') && !!document.querySelector('h1'))").await;
        r.check("a settings change keeps the removed file's last version", still == "true", &still);
        crate::settings::set(crate::settings::Settings::default());
        super::super::apply_settings();
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

        // ---- settings (ADR-0014)
        ctrl(&w, gdk::Key::comma);
        sleep(300).await;
        r.check("Ctrl+, opens settings", w.window.visible_dialog().is_some_and(|d| d.is::<adw::PreferencesDialog>()), "");
        r.note("shot settings", shot(&w, &shots.join("8-settings.png")).await);
        if let Some(d) = w.window.visible_dialog() {
            d.close();
        }
        sleep(200).await;
        let mut s = crate::settings::get();
        s.tree_left = true;
        s.key_hints = false;
        s.text_size = 125;
        s.width = crate::settings::Width::Wide;
        s.web_pictures = false;
        let loads = w.loads.get();
        crate::settings::set(s);
        super::super::apply_settings();
        r.check("tree on the left", w.split.sidebar_position() == gtk::PackType::Start, "");
        r.check("key hints off hides the hints, not the bar", w.status.widget.is_visible() && !w.status.hints_shown(), "");
        r.check("text size sets the zoom", (w.web.zoom_level() - 1.25).abs() < 0.01, w.web.zoom_level());
        wait_until(5_000, || w.loads.get() > loads).await;
        ready(&w).await;
        let width = js(&w, "getComputedStyle(document.querySelector('.page')).maxWidth").await;
        r.check("page width wide", width == "944px", &width);
        let mut s = crate::settings::get();
        s.gitignore = false;
        crate::settings::set(s);
        super::super::apply_settings();
        let gone_line = wait_until(3_000, || w.tree.left_out_text().is_none()).await;
        r.check("with .gitignore off nothing is left out", gone_line, format!("{:?}", w.tree.left_out_text()));
        let mut s = crate::settings::get();
        s.gitignore = true;
        crate::settings::set(s);
        super::super::apply_settings();
        let csp = js(&w, "document.querySelector('meta[http-equiv]').content").await;
        r.check("web pictures off leaves https out", !csp.contains("https:"), &csp);
        let saved = std::fs::read_to_string(
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_default().join("omamv/settings.toml"),
        )
        .unwrap_or_default();
        r.check("settings are saved", saved.contains("tree_side = \"left\"") && saved.contains("text_size = 125"), saved.lines().count());
        crate::settings::set(crate::settings::Settings::default());
        super::super::apply_settings();
        ready(&w).await;
        r.check("defaults back", w.split.sidebar_position() == gtk::PackType::End && w.status.hints_shown(), "");
        ctrl(&w, gdk::Key::plus);
        let zoom = w.web.zoom_level();
        let mut s = crate::settings::get();
        s.key_hints = false;
        crate::settings::set(s);
        super::super::apply_settings();
        r.check("a passing Ctrl+plus survives other settings", (w.web.zoom_level() - zoom).abs() < 0.01, format!("{zoom} -> {}", w.web.zoom_level()));
        crate::settings::set(crate::settings::Settings::default());
        super::super::apply_settings();

        // ---- a folder remembers where the reader was (ADR-0019)
        let entries = |d: &Path| {
            let mut v: Vec<String> = std::fs::read_dir(d).map(|r| r.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
            v.sort();
            v
        };
        let before = entries(&root);
        nav(&w, || w.open_file(Path::new("docs/other.md"), None, None, true)).await;
        w.js("document.getElementById('second').scrollIntoView({block:'start',behavior:'instant'})");
        let kept = wait_until(3_000, || crate::places::get(&root).is_some_and(|s| s.file == Path::new("docs/other.md") && s.y > 0)).await;
        let left_at = js(&w, "String(Math.round(window.scrollY))").await;
        r.check("the file read and the place in it are kept for the folder", kept, format!("{:?}", crate::places::get(&root)));
        r.check("  in omamv's own state, never in the folder", entries(&root) == before, format!("{:?}", entries(&root)));
        w.open_folder(&root.join("notes"), None);
        let fresh = wait_until(4_000, || current(&w) == "todo.txt").await;
        r.check("a folder never read opens its first file", fresh, current(&w));

        // ---- "Open a file by name" when a folder opens (ADR-0014)
        let mut s = crate::settings::get();
        s.ask_on_open = true;
        crate::settings::set(s);
        w.open_folder(&root.join("docs"), None);
        let asked = wait_until(4_000, || w.keyboard_mode() == Mode::QuickOpen).await;
        r.check("a folder can open on quick open instead of a file", asked && w.current.borrow().is_none(), format!("{:?} {}", w.keyboard_mode(), current(&w)));
        if let Some(q) = w.quick.borrow().clone() {
            q.entry.emit_by_name::<()>("stop-search", &[]);
        }
        sleep(300).await;
        r.check("Esc leaves the clean page, no file of omamv's choosing", w.current.borrow().is_none() && w.window.visible_dialog().is_none(), current(&w));
        w.open_folder(&root.join("docs/adr"), Some(PathBuf::from("0001-open-a-folder.md")));
        let named = wait_until(4_000, || current(&w) == "0001-open-a-folder.md").await;
        r.check("a file asked for still opens", named && w.window.visible_dialog().is_none(), current(&w));
        crate::settings::set(crate::settings::Settings::default());
        super::super::apply_settings();

        // ---- a folder with nothing to show (ADR-0012)
        w.open_folder(&root.join("src"), None);
        let said = wait_until(3_000, || w.showing() == "empty").await;
        r.check("an empty folder says so", said, w.showing());
        r.note("shot empty", shot(&w, &shots.join("9-empty.png")).await);
        w.open_folder(&root, None);
        let back = wait_until(3_000, || w.showing() == "doc").await && ready(&w).await;
        sleep(400).await;
        let at = js(&w, "String(Math.round(window.scrollY))").await;
        let same = at.parse::<i64>().ok().zip(left_at.parse::<i64>().ok()).is_some_and(|(a, b)| b > 0 && (a - b).abs() <= 4);
        r.check("a folder read before opens on its last file, where it was left", back && current(&w) == "docs/other.md" && same, format!("{} at {at}, left at {left_at}", current(&w)));

        // ---- start screen
        ctrl(&w, gdk::Key::O);
        key(&w, gdk::Key::O, gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
        sleep(200).await;
        r.note("shot start", shot(&w, &shots.join("8-start.png")).await);
        ctrl(&w, gdk::Key::comma);
        sleep(300).await;
        r.check("Ctrl+, works on the start screen too", w.window.visible_dialog().is_some_and(|d| d.is::<adw::PreferencesDialog>()), "");
        if let Some(d) = w.window.visible_dialog() {
            d.close();
        }
        sleep(200).await;
        press(&w, gdk::Key::Escape);
        sleep(100).await;

        r.note("failed", r.failed);
        r.note("took ms", t0.elapsed().as_millis());
        if let Err(e) = std::fs::write(&report, r.lines.join("\n") + "\n") {
            eprintln!("self-check: report not written to {}: {e}", report.display());
        }
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
