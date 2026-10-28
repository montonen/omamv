//! The gallery (ADR-0024): the ten files changed most recently on disk, each
//! as a card with the top of its page drawn small. Built off the main thread:
//! nothing here touches the window.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::render::esc;
use crate::theme::Palette;
use crate::wiki::Links;

/// Cards in a gallery: the keys 1 to 9, and 0.
pub const CARDS: usize = 10;

/// What a card shows of a file at most; the rest is never read.
const READ_AT_MOST: u64 = 256 * 1024;

/// The `.md` and `.txt` files among `files` (from `root`), newest on disk
/// first, at most `CARDS`; the tree's order among equals.
pub fn newest(root: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    let mut dated: Vec<(Option<SystemTime>, &PathBuf)> = files
        .iter()
        .filter(|f| !crate::folder::is_html(f))
        .map(|f| (root.join(f).metadata().and_then(|m| m.modified()).ok(), f))
        .collect();
    dated.sort_by_key(|d| std::cmp::Reverse(d.0));
    dated.into_iter().take(CARDS).map(|(_, f)| f.clone()).collect()
}

/// The key that opens card `i` (0-based): 1 … 9, then 0.
pub fn key(i: usize) -> char {
    char::from_digit(((i + 1) % 10) as u32, 10).unwrap_or('?')
}

fn read_top(path: &Path) -> String {
    use std::io::Read;
    let mut bytes = Vec::new();
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(READ_AT_MOST).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The gallery page for `cards` (from `root`), in order. `width` is the
/// page's own reading width in pixels, which each card scales to its own.
pub fn page(root: &Path, cards: &[PathBuf], sel: usize, all: Arc<Vec<PathBuf>>, palette: &Palette, width: u32, now: SystemTime) -> String {
    let mut html = String::from(r#"<div class="om-gallery">"#);
    for (i, rel) in cards.iter().enumerate() {
        let full = root.join(rel);
        let links = Links { root: root.to_path_buf(), rel: rel.clone(), files: all.clone() };
        let body = crate::render::preview(rel, &read_top(&full), palette, &links);
        let age = full
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .map(|d| crate::ui::quickopen::age(d.as_secs()))
            .unwrap_or_default();
        let secs = now.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let git = crate::git::look(&full).map(|l| crate::git::words(&l, secs)).unwrap_or_default();
        // The folder it is in, and its git state; a file at the root has no folder to name.
        let dir = rel.parent().map(|p| p.display().to_string()).unwrap_or_default();
        let sub = [dir.as_str(), git.as_str()].iter().filter(|p| !p.is_empty()).map(|p| esc(p)).collect::<Vec<_>>().join(" · ");
        html.push_str(&format!(
            r#"<div class="om-card{selected}" data-i="{i}"><a class="om-card-hit" href="{href}" aria-label="{name}"></a><div class="om-card-head"><kbd>{key}</kbd><span class="om-card-name">{name}</span><span class="om-card-age">{age}</span></div><div class="om-card-sub">{sub}</div><div class="om-card-view"><div class="om-card-page" style="width:{w}px">{body}</div></div></div>"#,
            href = esc(&crate::wiki::href(Some(rel), None)),
            key = key(i),
            selected = if i == sel { " om-sel" } else { "" },
            name = esc(&crate::folder::file_name(rel)),
            age = esc(&age),
            w = width + 64,
        ));
    }
    for _ in cards.len()..CARDS {
        html.push_str(r#"<div class="om-card om-card-empty"></div>"#);
    }
    html.push_str("</div>");
    let title = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    crate::render::wrap(&format!("Gallery · {title}"), &html, palette)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_run_one_to_nine_then_zero() {
        let keys: String = (0..CARDS).map(key).collect();
        assert_eq!(keys, "1234567890");
    }

    #[test]
    fn the_newest_ten_and_no_html() {
        let dir = std::env::temp_dir().join(format!("omamv-gallery-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut files = Vec::new();
        for i in 0..12 {
            let f = PathBuf::from(format!("n{i:02}.md"));
            let _ = std::fs::write(dir.join(&f), "# x\n");
            let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 + i * 60);
            let _ = std::fs::File::options().write(true).open(dir.join(&f)).and_then(|h| h.set_modified(t));
            files.push(f);
        }
        let _ = std::fs::write(dir.join("r.html"), "<p>");
        files.push("r.html".into());
        let ten = newest(&dir, &files);
        assert_eq!(ten.len(), CARDS);
        assert_eq!(ten[0], PathBuf::from("n11.md"));
        assert!(!ten.iter().any(|f| f.ends_with("r.html")));
        let page = page(&dir, &ten, 1, Arc::new(files), &crate::theme::fallback(true), 704, SystemTime::now());
        assert_eq!(page.matches(r#"class="om-card-hit""#).count(), CARDS);
        assert!(page.contains(r#"<div class="om-card om-sel" data-i="1"><a class="om-card-hit" href="/n10.md""#), "the second card is the one picked");
        assert!(page.contains(r#"href="/n11.md""#) && page.contains("<kbd>0</kbd>"));
        // A card holds the page's own links; it is never itself one around them.
        assert!(!page.contains(r#"<a class="om-card""#));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
