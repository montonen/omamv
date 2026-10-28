//! Pictures for the README and montonen.se (debug builds only):
//! `OMAMV_SHOTS=<plan>` runs a window through the plan's steps and
//! `OMAMV_SHOTS_OUT=<dir>` gets the pictures. `scripts/shots.sh` sets it up on
//! GTK's Broadway display, over public folders only.
//!
//! A plan is one step a line; `#` starts a comment.
//!
//! ```text
//! size 1040 560          the window's size, in points
//! open manual/07.md      a file from the folder's root, and wait until drawn
//! key o                  a key, as the window takes it: `key p ctrl`, `key S ctrl shift`
//! type theme             into the dialog's search entry
//! hold ctrl              modifiers held, for the status bar: `hold` lets go
//! js <script>            run in the page
//! tree 320               the folder tree's width; `tree hide` / `tree show`
//! wait 500               milliseconds
//! shot reading           the window as it is now, into <dir>/reading.png
//! ```

#[cfg(debug_assertions)]
pub use imp::maybe_start;

#[cfg(not(debug_assertions))]
pub fn maybe_start(_: &std::rc::Rc<super::window::Win>) {}

/// A plan is running: a window of its own, as the self-check's.
pub fn driven() -> bool {
    cfg!(debug_assertions) && std::env::var_os("OMAMV_SHOTS").is_some()
}

#[cfg(debug_assertions)]
mod imp {
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::time::Duration;

    use adw::prelude::*;
    use gtk::{gdk, glib};
    use webkit6::prelude::*;

    use super::super::window::Win;

    thread_local! {
        static STARTED: Cell<bool> = const { Cell::new(false) };
    }

    pub fn maybe_start(win: &Rc<Win>) {
        let Some(plan) = std::env::var_os("OMAMV_SHOTS").map(PathBuf::from) else { return };
        let out = std::env::var_os("OMAMV_SHOTS_OUT").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("shots"));
        if STARTED.with(|s| s.replace(true)) {
            return;
        }
        glib::spawn_future_local(run(win.clone(), plan, out));
    }

    async fn sleep(ms: u64) {
        glib::timeout_future(Duration::from_millis(ms)).await;
    }

    async fn ready(w: &Win) {
        for _ in 0..200 {
            if w.ready.get() {
                break;
            }
            sleep(25).await;
        }
        // Diagrams, pictures and fonts settle after the page says it is ready.
        sleep(700).await;
    }

    fn mods(words: &[&str]) -> gdk::ModifierType {
        let mut m = gdk::ModifierType::empty();
        for w in words {
            m |= match *w {
                "ctrl" => gdk::ModifierType::CONTROL_MASK,
                "shift" => gdk::ModifierType::SHIFT_MASK,
                "alt" => gdk::ModifierType::ALT_MASK,
                _ => gdk::ModifierType::empty(),
            };
        }
        m
    }

    unsafe extern "C" {
        /// GTK's own, for Broadway; not in the Rust bindings omamv uses.
        fn gdk_broadway_display_set_surface_scale(display: *mut gdk::ffi::GdkDisplay, scale: std::ffi::c_int);
    }

    /// Broadway draws at the scale a connected browser asks for, and here
    /// none is connected: `OMAMV_SHOTS_SCALE` (2 by default) sets it.
    fn set_scale() {
        let Some(display) = gdk::Display::default() else { return };
        if display.type_().name() != "GdkBroadwayDisplay" {
            return;
        }
        let scale = std::env::var("OMAMV_SHOTS_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(2);
        // SAFETY: a live GdkBroadwayDisplay, checked by its type just above.
        unsafe { gdk_broadway_display_set_surface_scale(display.as_ptr(), scale) };
    }

    async fn run(w: Rc<Win>, plan: PathBuf, out: PathBuf) {
        set_scale();
        let _ = std::fs::create_dir_all(&out);
        let text = std::fs::read_to_string(&plan).unwrap_or_default();
        while w.root().is_none() {
            sleep(20).await;
        }
        ready(&w).await;
        let mut failed = 0;
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let (step, rest) = line.split_once(' ').unwrap_or((line, ""));
            let words: Vec<&str> = rest.split_whitespace().collect();
            eprintln!("shots: {line}");
            match step {
                "size" => {
                    let n = |i: usize| words.get(i).and_then(|v| v.parse().ok()).unwrap_or(800);
                    w.window.set_size_request(n(0), n(1));
                    w.window.set_default_size(n(0), n(1));
                    sleep(400).await;
                }
                "open" => {
                    w.open_file(Path::new(rest), None, None, true);
                    w.web.grab_focus();
                    sleep(50).await;
                    ready(&w).await;
                }
                "key" => {
                    let Some(name) = words.first() else { continue };
                    let Some(k) = gdk::Key::from_name(*name) else {
                        eprintln!("shots: no key {name}");
                        failed += 1;
                        continue;
                    };
                    if !w.key(k, mods(&words[1..])) {
                        // Not the window's: the dialog's entry has it.
                        if let Some(q) = w.quick.borrow().clone() {
                            q.entry.emit_by_name::<()>("activate", &[]);
                        }
                    }
                    sleep(400).await;
                    if w.window.visible_dialog().is_none() {
                        ready(&w).await;
                    }
                }
                "type" => {
                    if let Some(q) = w.quick.borrow().clone() {
                        q.entry.set_text(rest);
                    }
                    sleep(400).await;
                }
                "hold" => w.modifiers(mods(&words)),
                "js" => w.js(rest),
                "tree" => match rest {
                    "hide" => w.split.set_show_sidebar(false),
                    "show" => w.split.set_show_sidebar(true),
                    px => w.set_tree_width(px.parse().unwrap_or(248.0)),
                },
                "wait" => sleep(rest.parse().unwrap_or(500)).await,
                "shot" => {
                    let path = out.join(format!("{rest}.png"));
                    match render(&w, &path).await {
                        Ok(size) => eprintln!("shots: {} ({size})", path.display()),
                        Err(e) => {
                            eprintln!("shots: no picture {rest}: {e}");
                            failed += 1;
                        }
                    }
                }
                other => {
                    eprintln!("shots: no step {other}");
                    failed += 1;
                }
            }
        }
        eprintln!("shots: done, {failed} failed");
        std::process::exit(i32::from(failed > 0));
    }

    /// The window as it is now, at its scale: its own parts drawn there and
    /// then (the last frame Broadway kept is often blank), the page as
    /// WebKit draws it, and an open dialog over both.
    async fn render(w: &Win, path: &Path) -> Result<String, String> {
        sleep(300).await;
        let window: gtk::Widget = w.window.clone().upcast();
        let (width, height) = (window.width(), window.height());
        if width < 2 {
            return Err("the window has no size".into());
        }
        let scale = window.scale_factor() as f32;
        let page = if w.web.is_drawable() {
            w.web.snapshot_future(webkit6::SnapshotRegion::Visible, webkit6::SnapshotOptions::NONE).await.ok()
        } else {
            None
        };
        // A dialog's sheet waits for its opening animation, which needs
        // frames Broadway does not send: shown here as it ends.
        if let Some(d) = w.window.visible_dialog() {
            let d: gtk::Widget = d.upcast();
            let mut stack = vec![d];
            while let Some(x) = stack.pop() {
                if x.parent().is_some_and(|p| p.type_().name() == "AdwFloatingSheet") && x.type_().name() == "AdwGizmo" {
                    x.set_child_visible(true);
                    x.set_opacity(1.0);
                }
                let mut c = x.first_child();
                while let Some(k) = c {
                    c = k.next_sibling();
                    stack.push(k);
                }
            }
        }
        window.allocate(width, height, -1, None);
        let snap = gtk::Snapshot::new();
        snap.scale(scale, scale);
        let bg = gdk::RGBA::parse(crate::scheme::palette().background.as_str()).unwrap_or(gdk::RGBA::BLACK);
        snap.append_color(&bg, &gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32));
        let mut child = window.first_child();
        while let Some(c) = child {
            window.snapshot_child(&c, &snap);
            child = c.next_sibling();
        }
        if let (Some(t), Some(b)) = (page, w.web.compute_bounds(&window)) {
            snap.push_clip(&b);
            snap.append_texture(&t, &b);
            snap.pop();
        }
        if let Some(d) = w.window.visible_dialog() {
            let d: gtk::Widget = d.upcast();
            if let (Some(parent), Some(at)) = (d.parent(), d.parent().and_then(|p| p.compute_bounds(&window))) {
                // A dialog dims what is behind it.
                snap.append_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.25), &gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32));
                snap.save();
                snap.translate(&gtk::graphene::Point::new(at.x(), at.y()));
                parent.snapshot_child(&d, &snap);
                snap.restore();
            }
        }
        let node = snap.to_node().ok_or("nothing to draw")?;
        let renderer = gtk::gsk::CairoRenderer::new();
        renderer.realize(gdk::Surface::NONE).map_err(|e| e.to_string())?;
        let texture = renderer.render_texture(&node, None);
        renderer.unrealize();
        texture.save_to_png(path).map_err(|e| e.to_string())?;
        Ok(format!("{}×{}", texture.width(), texture.height()))
    }
}
