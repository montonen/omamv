//! The folder: which files omamv shows, in which order (ADR-0003, ADR-0004).
//!
//! Only `.md` and `.txt` files, and only folders that hold one somewhere
//! below them. Hidden files and anything `.gitignore` leaves out are skipped,
//! as `rg` and `fd` skip them. Folders come first, then files, each sorted
//! by name without regard to case: the order of the tree, and of `]` / `[`.

use std::path::{Path, PathBuf};

/// A walk stops after this many files, so a home folder opened by mistake
/// cannot keep the tree busy for minutes.
pub const MAX_FILES: usize = 20_000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dir {
    /// The folder's name; empty for the root.
    pub name: String,
    /// Path from the root, empty for the root.
    pub rel: PathBuf,
    pub dirs: Vec<Dir>,
    /// Paths from the root.
    pub files: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct Scan {
    pub tree: Dir,
    /// Every file in tree order.
    pub order: Vec<PathBuf>,
    /// The walk stopped at `MAX_FILES`.
    pub truncated: bool,
    /// Every folder the walk went into, the root included: what is watched.
    pub dirs: Vec<PathBuf>,
    /// Folders left out by the rules, counted where they were met (not the
    /// folders inside them): by `.gitignore`, and for starting with a dot.
    pub ignored: usize,
    pub hidden: usize,
}

/// Is this a file omamv shows? Markdown and text (ADR-0003), and HTML pages
/// when the setting is on (ADR-0016, off by default).
pub fn is_shown(path: &Path) -> bool {
    let text = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("txt"));
    text || (is_html(path) && crate::settings::get().html)
}

/// Is this file Markdown (rendered), rather than text (shown as it is)?
pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

/// Is this an HTML page (shown as its author made it, without scripts)?
pub fn is_html(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
}

/// What a walk leaves out (ADR-0014).
#[derive(Clone, Copy, Debug)]
pub struct Skip {
    pub hidden: bool,
    pub gitignored: bool,
}

impl Default for Skip {
    fn default() -> Self {
        Skip { hidden: true, gitignored: true }
    }
}

/// The line under the tree when folders were left out, or `None`.
pub fn left_out_line(ignored: usize, hidden: usize) -> Option<String> {
    let folders = |n: usize| if n == 1 { "1 folder".to_string() } else { format!("{n} folders") };
    match (ignored, hidden) {
        (0, 0) => None,
        (i, 0) => Some(format!("{} left out by .gitignore", folders(i))),
        (0, h) => Some(format!("{} hidden", folders(h))),
        (i, h) => Some(format!("{} left out by .gitignore, {h} hidden", folders(i))),
    }
}

pub fn scan(root: &Path, skip: Skip) -> Scan {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut truncated = false;
    let walk = ignore::WalkBuilder::new(root)
        .hidden(skip.hidden)
        .git_ignore(skip.gitignored)
        .git_exclude(skip.gitignored)
        .git_global(skip.gitignored)
        .ignore(skip.gitignored)
        .parents(skip.gitignored)
        // .git itself is never anyone's reading.
        .filter_entry(|e| e.file_name() != ".git")
        .follow_links(false)
        .require_git(false)
        .build();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in walk.flatten() {
        if entry.file_type().is_some_and(|t| t.is_dir()) {
            dirs.push(entry.path().to_path_buf());
            continue;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) || !is_shown(entry.path()) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(root) {
            files.push(rel.to_path_buf());
        }
        if files.len() >= MAX_FILES {
            truncated = true;
            break;
        }
    }
    let (ignored, hidden) = if truncated { (0, 0) } else { left_out(&dirs, skip) };
    let tree = build(&files);
    let mut order = Vec::with_capacity(files.len());
    flatten(&tree, &mut order);
    Scan { tree, order, truncated, dirs, ignored, hidden }
}

/// The folders right below the walked ones that the walk did not go into.
/// `.git` is never anyone's reading and is not counted. A dot-folder counts
/// as hidden while hidden folders are skipped, else as left out by .gitignore.
fn left_out(dirs: &[PathBuf], skip: Skip) -> (usize, usize) {
    let walked: std::collections::HashSet<&Path> = dirs.iter().map(PathBuf::as_path).collect();
    let (mut ignored, mut hidden) = (0, 0);
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let path = e.path();
            let name = e.file_name();
            if name == ".git" || walked.contains(path.as_path()) {
                continue;
            }
            if skip.hidden && name.to_string_lossy().starts_with('.') {
                hidden += 1;
            } else {
                ignored += 1;
            }
        }
    }
    (ignored, hidden)
}

/// The tree of these files: folders appear only because a file is in them.
pub fn build(files: &[PathBuf]) -> Dir {
    let mut root = Dir::default();
    for file in files {
        let mut dir = &mut root;
        let parts: Vec<_> = file.components().collect();
        let Some((_, folders)) = parts.split_last() else { continue };
        for part in folders {
            let name = part.as_os_str().to_string_lossy().into_owned();
            let at = match dir.dirs.iter().position(|d| d.name == name) {
                Some(at) => at,
                None => {
                    let rel = dir.rel.join(&name);
                    dir.dirs.push(Dir { name, rel, ..Dir::default() });
                    dir.dirs.len() - 1
                }
            };
            dir = &mut dir.dirs[at];
        }
        dir.files.push(file.clone());
    }
    sort(&mut root);
    root
}

fn sort(dir: &mut Dir) {
    dir.dirs.sort_by_key(|d| sort_key(&d.name));
    dir.files.sort_by_key(|f| sort_key(&file_name(f)));
    for d in &mut dir.dirs {
        sort(d);
    }
}

fn sort_key(name: &str) -> (String, String) {
    (name.to_lowercase(), name.to_owned())
}

/// Folders first, then the files beside them: the order a reader meets them
/// in the tree.
pub fn flatten(dir: &Dir, out: &mut Vec<PathBuf>) {
    for d in &dir.dirs {
        flatten(d, out);
    }
    out.extend(dir.files.iter().cloned());
}

/// `files` (from `root`) with the time each was last written, the newest
/// first; the given order among equals. Read now, not at the scan: a file
/// rewritten since counts as new (quick open and the gallery, ADR-0017/0024).
pub fn by_age(root: &Path, files: impl IntoIterator<Item = PathBuf>) -> Vec<(Option<std::time::SystemTime>, PathBuf)> {
    let mut dated: Vec<_> = files.into_iter().map(|f| (root.join(&f).metadata().and_then(|m| m.modified()).ok(), f)).collect();
    dated.sort_by_key(|d| std::cmp::Reverse(d.0));
    dated
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The file to show when a folder opens: a README at the top, then an index,
/// then the first file in tree order.
pub fn first_file(scan: &Scan) -> Option<PathBuf> {
    let top = &scan.tree.files;
    for want in ["readme.md", "index.md", "readme.txt"] {
        if let Some(f) = top.iter().find(|f| file_name(f).to_lowercase() == want) {
            return Some(f.clone());
        }
    }
    top.first().cloned().or_else(|| scan.order.first().cloned())
}

/// `~/code/x` for a path under the home folder.
pub fn tilde(path: &Path) -> String {
    if let Some(home) = std::env::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        if rest.as_os_str().is_empty() {
            return "~".into();
        }
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn folders_come_first_and_names_sort_without_case() {
        let tree = build(&[p("README.md"), p("b.md"), p("docs/z.md"), p("docs/adr/1.md"), p("A.txt")]);
        let mut order = Vec::new();
        flatten(&tree, &mut order);
        assert_eq!(order, [p("docs/adr/1.md"), p("docs/z.md"), p("A.txt"), p("b.md"), p("README.md")]);
    }

    #[test]
    fn only_markdown_and_text_and_only_their_folders() {
        let dir = std::env::temp_dir().join(format!("omamv-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (f, body) in [
            ("README.md", "# hi"),
            ("notes/todo.txt", "x"),
            ("src/main.rs", "fn main(){}"),
            ("images/a.png", ""),
            (".hidden/secret.md", "no"),
            ("build/out.md", "ignored"),
            (".gitignore", "build/\n"),
        ] {
            let path = dir.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        let all = scan(&dir, Skip { hidden: false, gitignored: false });
        assert_eq!(all.order.len(), 4, "hidden and ignored files too: {:?}", all.order);
        assert_eq!((all.ignored, all.hidden), (0, 0), "nothing left out when nothing is skipped");
        let scan = scan(&dir, Skip::default());
        assert_eq!(scan.order, [p("notes/todo.txt"), p("README.md")]);
        assert_eq!((scan.ignored, scan.hidden), (1, 1), "build/ by .gitignore, .hidden for its dot");
        assert_eq!(scan.tree.dirs.len(), 1, "src, images and the hidden and ignored folders are left out");
        assert!(scan.dirs.contains(&dir.join("src")), "watched: every folder walked");
        assert!(!scan.dirs.iter().any(|d| d.ends_with("build") || d.ends_with(".hidden")), "not what is skipped");
        assert_eq!(first_file(&scan), Some(p("README.md")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_left_out_line_reads_well() {
        assert_eq!(left_out_line(0, 0), None);
        assert_eq!(left_out_line(1, 0).unwrap(), "1 folder left out by .gitignore");
        assert_eq!(left_out_line(46, 2).unwrap(), "46 folders left out by .gitignore, 2 hidden");
        assert_eq!(left_out_line(0, 3).unwrap(), "3 folders hidden");
    }

    #[test]
    fn first_file_without_a_readme() {
        let scan = Scan { tree: build(&[p("docs/a.md")]), order: vec![p("docs/a.md")], truncated: false, dirs: Vec::new(), ignored: 0, hidden: 0 };
        assert_eq!(first_file(&scan), Some(p("docs/a.md")));
    }
}

#[cfg(test)]
mod probe {
    /// `OMAMV_PROBE=<dir> cargo test probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe() {
        let dir = std::env::var("OMAMV_PROBE").unwrap();
        let t = std::time::Instant::now();
        let s = super::scan(std::path::Path::new(&dir), super::Skip::default());
        eprintln!("{} files, {} folders in {:?}, truncated {}", s.order.len(), s.dirs.len(), t.elapsed(), s.truncated);
    }
}
