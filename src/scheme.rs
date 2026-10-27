//! `omamv://`, where every page and picture comes from (ADR-0010).
//!
//! `omamv://assets/<name>` is omamv's own CSS and scripts. `omamv://w<N>/<path>`
//! is a file in window N's folder: `.md` and `.txt` files are rendered off the
//! main thread, anything else (pictures) is handed over as it is. Nothing
//! outside the folder is served.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use gtk::{gio, glib};
use webkit6::URISchemeRequest;

use crate::theme::Palette;

const MERMAID: &[u8] = include_bytes!("../assets/mermaid.min.js");
const APP_JS: &[u8] = include_bytes!("../assets/app.js");
const PAGE_CSS: &[u8] = include_bytes!("../assets/page.css");

thread_local! {
    static ROOTS: RefCell<HashMap<String, PathBuf>> = RefCell::new(HashMap::new());
}

fn palette_cell() -> &'static Mutex<Arc<Palette>> {
    static CELL: OnceLock<Mutex<Arc<Palette>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(Arc::new(crate::theme::fallback(true))))
}

/// The palette pages are drawn in from now on.
pub fn set_palette(p: Palette) {
    if let Ok(mut cell) = palette_cell().lock() {
        *cell = Arc::new(p);
    }
}

pub fn palette() -> Arc<Palette> {
    palette_cell().lock().map(|c| c.clone()).unwrap_or_else(|_| Arc::new(crate::theme::fallback(true)))
}

/// Let window `host` serve files from `root`; `None` stops it.
pub fn set_root(host: &str, root: Option<PathBuf>) {
    ROOTS.with(|r| match root {
        Some(root) => {
            r.borrow_mut().insert(host.to_owned(), root);
        }
        None => {
            r.borrow_mut().remove(host);
        }
    });
}

pub fn register(ctx: &webkit6::WebContext) {
    if let Some(sm) = ctx.security_manager() {
        sm.register_uri_scheme_as_secure("omamv");
        sm.register_uri_scheme_as_local("omamv");
    }
    ctx.register_uri_scheme("omamv", handle);
}

/// `omamv://w3/docs/a%20b.md`
pub fn uri(host: &str, rel: &Path) -> String {
    let parts: Vec<String> = rel
        .components()
        .map(|c| glib::Uri::escape_string(&c.as_os_str().to_string_lossy(), None, false).to_string())
        .collect();
    format!("omamv://{host}/{}", parts.join("/"))
}

/// The host and the file a `omamv://` address names, without query or fragment.
pub fn parse(uri: &str) -> Option<(String, PathBuf, Option<String>)> {
    let rest = uri.strip_prefix("omamv://")?;
    let (rest, fragment) = match rest.split_once('#') {
        Some((r, f)) => (r, Some(f.to_owned())),
        None => (rest, None),
    };
    let rest = rest.split('?').next().unwrap_or("");
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let path = glib::Uri::unescape_string(path, None::<&str>)?.to_string();
    let rel = PathBuf::from(path);
    // Never a way up and out of the folder.
    if rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return None;
    }
    Some((host.to_owned(), rel, fragment))
}

fn not_found(req: &URISchemeRequest, why: &str) {
    if std::env::var_os("OMAMV_DEBUG").is_some() {
        eprintln!("omamv: {} not served: {why}", req.uri().unwrap_or_default());
    }
    req.finish_error(&mut glib::Error::new(gio::IOErrorEnum::NotFound, why));
}

fn finish(req: &URISchemeRequest, bytes: Vec<u8>, mime: &str) {
    let len = bytes.len() as i64;
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(bytes));
    req.finish(&stream, len, Some(mime));
}

fn handle(req: &URISchemeRequest) {
    let uri = req.uri().map(|u| u.to_string()).unwrap_or_default();
    let Some((host, rel, _)) = parse(&uri) else {
        return not_found(req, "not an address omamv knows");
    };
    if host == "assets" {
        return match rel.to_str() {
            Some("app.js") => finish(req, APP_JS.to_vec(), "text/javascript"),
            Some("page.css") => finish(req, PAGE_CSS.to_vec(), "text/css"),
            Some("mermaid.js") => finish(req, MERMAID.to_vec(), "text/javascript"),
            _ => not_found(req, "no such asset"),
        };
    }
    let Some(root) = ROOTS.with(|r| r.borrow().get(&host).cloned()) else {
        return not_found(req, "no window for this address");
    };
    let full = root.join(&rel);
    let inside = full.canonicalize().ok().zip(root.canonicalize().ok()).is_some_and(|(f, r)| f.starts_with(r));
    if !inside {
        return not_found(req, "outside the folder");
    }
    if crate::folder::is_shown(&full) {
        let req = req.clone();
        let palette = palette();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let page = std::fs::read(&full).map(|bytes| {
                let text = String::from_utf8_lossy(&bytes);
                crate::render::page(&full, &text, &palette)
            });
            let _ = tx.send_blocking(page);
        });
        glib::spawn_future_local(async move {
            match rx.recv().await {
                Ok(Ok(html)) => finish(&req, html.into_bytes(), "text/html"),
                Ok(Err(e)) => not_found(&req, &e.to_string()),
                Err(_) => not_found(&req, "the page could not be drawn"),
            }
        });
        return;
    }
    let mime = match full.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        Some("ico") => "image/x-icon",
        _ => return not_found(req, "omamv shows pictures and text only"),
    };
    match std::fs::read(&full) {
        Ok(bytes) => finish(req, bytes, mime),
        Err(e) => not_found(req, &e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_round_trip() {
        let u = uri("w1", Path::new("docs/a b#1.md"));
        assert_eq!(u, "omamv://w1/docs/a%20b%231.md");
        let (host, rel, frag) = parse(&format!("{u}?y=3#keys")).unwrap();
        assert_eq!((host.as_str(), rel, frag.as_deref()), ("w1", PathBuf::from("docs/a b#1.md"), Some("keys")));
    }

    #[test]
    fn no_way_out_of_the_folder() {
        assert!(parse("omamv://w1/../etc/passwd").is_none());
        assert!(parse("omamv://w1/docs/%2e%2e/%2e%2e/x").is_none());
        assert!(parse("omamv://w1//etc/passwd").is_some_and(|(_, r, _)| r == Path::new("etc/passwd")) || parse("omamv://w1//etc/passwd").is_none());
    }
}
