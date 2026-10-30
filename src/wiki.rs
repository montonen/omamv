//! Obsidian's `[[wikilinks]]` (ADR-0018): `[[Note]]`, `[[folder/Note]]`,
//! `[[Note#Heading]]`, `[[#Heading]]`, `[[Note|shown text]]`, and
//! `![[picture.png]]`. A target is found the way Obsidian finds it: by its
//! path, or by its name alone anywhere in the folder.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What a page needs to know to resolve its links: where it is, and what
/// else the folder holds.
#[derive(Clone)]
pub struct Links {
    pub root: PathBuf,
    /// The page's file, from the root.
    pub rel: PathBuf,
    /// Every file the folder shows, from the root.
    pub files: Arc<Vec<PathBuf>>,
}

/// A wikilink's target split up: the file part (may be empty: this page)
/// and the heading (without `#`; block references `#^id` are dropped).
pub fn split(target: &str) -> (&str, Option<&str>) {
    let (file, heading) = match target.split_once('#') {
        Some((f, h)) => (f, Some(h)),
        None => (target, None),
    };
    (file.trim(), heading.map(str::trim).filter(|h| !h.is_empty() && !h.starts_with('^')))
}

/// The text a link shows when it has no `|shown text`: Obsidian's way,
/// `Note › Heading`, or just the heading on its own page.
pub fn label(target: &str) -> String {
    match split(target) {
        ("", Some(h)) => h.to_owned(),
        (f, Some(h)) => format!("{f} › {h}"),
        (f, None) => f.to_owned(),
    }
}

fn lower(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

/// The file a note link names, from the root. `name` may leave out `.md`.
pub fn resolve(name: &str, links: &Links) -> Option<PathBuf> {
    let name = name.trim().trim_start_matches('/');
    if name.is_empty() {
        return None;
    }
    let shown = crate::folder::is_shown(Path::new(name));
    let wanted: Vec<String> = if shown { vec![name.to_lowercase()] } else { vec![format!("{name}.md").to_lowercase(), name.to_lowercase()] };
    let here = links.rel.parent().unwrap_or(Path::new(""));
    let by = |p: &Path| links.files.iter().find(|f| lower(f) == lower(p)).cloned();
    for w in &wanted {
        // Beside the page, then from the root.
        if let Some(f) = by(&here.join(w)).or_else(|| by(Path::new(w))) {
            return Some(f);
        }
    }
    // By its name (or the end of its path) anywhere: the one closest to
    // this page, then the shortest path.
    let shared = |f: &Path| f.components().zip(here.components()).take_while(|(a, b)| a == b).count();
    links
        .files
        .iter()
        .filter(|f| {
            let l = lower(f);
            wanted.iter().any(|w| l.ends_with(&format!("/{w}")))
        })
        .min_by(|a, b| {
            shared(b)
                .cmp(&shared(a))
                .then_with(|| a.components().count().cmp(&b.components().count()))
                .then_with(|| a.cmp(b))
        })
        .cloned()
}

/// A picture `![[name]]` names, from the root: beside the page, then from
/// the root, if it is there and inside the folder.
pub fn picture(name: &str, links: &Links) -> Option<PathBuf> {
    let name = name.trim().trim_start_matches('/');
    let here = links.rel.parent().unwrap_or(Path::new(""));
    [here.join(name), PathBuf::from(name)].into_iter().find(|p| {
        p.components().all(|c| matches!(c, std::path::Component::Normal(_))) && links.root.join(p).is_file()
    })
}

/// The address a link takes on the page: from the folder's root, so it
/// works from any page (`/docs/a%20b.md#heading`).
pub fn href(file: Option<&Path>, heading: Option<&str>) -> String {
    let mut out = String::new();
    if let Some(f) = file {
        for c in f.components() {
            out.push('/');
            out.push_str(&escape(&c.as_os_str().to_string_lossy()));
        }
    }
    if let Some(h) = heading {
        out.push('#');
        out.push_str(&escape(&crate::render::slug(h)));
    }
    out
}

fn escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(rel: &str, files: &[&str]) -> Links {
        Links { root: PathBuf::from("/nowhere"), rel: rel.into(), files: Arc::new(files.iter().map(PathBuf::from).collect()) }
    }

    #[test]
    fn targets_split_into_file_and_heading() {
        assert_eq!(split("#Parked by the panel"), ("", Some("Parked by the panel")));
        assert_eq!(split("Ideas#Top-down lake view"), ("Ideas", Some("Top-down lake view")));
        assert_eq!(split("Note#^block1"), ("Note", None));
        assert_eq!(label("#Weather flavour"), "Weather flavour");
        assert_eq!(label("Ideas#Lake"), "Ideas › Lake");
    }

    #[test]
    fn resolved_like_obsidian() {
        let l = links(
            "Projects/alpha/ADR/Index.md",
            &[
                "Projects/alpha/ADR/0004-tree.md",
                "Projects/beta/ADR/0004-tree.md",
                "Projects/alpha/Project.md",
                "Projects/Gamma/Project.md",
                "Projects/Gamma/Research/2026-01-02-01-x.md",
                "Notes/Inbox.md",
            ],
        );
        // Path from a folder above: the copy closest to this page.
        assert_eq!(resolve("ADR/0004-tree", &l), Some("Projects/alpha/ADR/0004-tree.md".into()));
        // A name alone, anywhere; the nearer one wins.
        assert_eq!(resolve("Project", &l), Some("Projects/alpha/Project.md".into()));
        assert_eq!(resolve("inbox", &l), Some("Notes/Inbox.md".into()));
        assert_eq!(resolve("Research/2026-01-02-01-x", &l), Some("Projects/Gamma/Research/2026-01-02-01-x.md".into()));
        assert_eq!(resolve("0004-tree.md", &l), Some("Projects/alpha/ADR/0004-tree.md".into()));
        assert_eq!(resolve("Nowhere", &l), None);
        // From the root, when the folder opened is the project.
        let p = links("ADR/Index.md", &["ADR/0004-tree.md", "Project.md"]);
        assert_eq!(resolve("ADR/0004-tree", &p), Some("ADR/0004-tree.md".into()));
        assert_eq!(resolve("Project", &p), Some("Project.md".into()));
    }

    #[test]
    fn addresses_are_escaped_and_headings_slugged() {
        assert_eq!(href(Some(Path::new("Research/a b.md")), Some("Top-down lake view")), "/Research/a%20b.md#top-down-lake-view");
        assert_eq!(href(None, Some("Parked by the design panel 2026-10-25")), "#parked-by-the-design-panel-2026-10-25");
    }
}
