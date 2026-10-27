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
}

/// Is this a file omamv shows?
pub fn is_shown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("txt"))
}

/// Is this file Markdown (rendered), rather than text (shown as it is)?
pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

pub fn scan(root: &Path) -> Scan {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut truncated = false;
    let walk = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .parents(true)
        .follow_links(false)
        .require_git(false)
        .build();
    for entry in walk.flatten() {
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
    let tree = build(&files);
    let mut order = Vec::with_capacity(files.len());
    flatten(&tree, &mut order);
    Scan { tree, order, truncated }
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
        let scan = scan(&dir);
        assert_eq!(scan.order, [p("notes/todo.txt"), p("README.md")]);
        assert_eq!(scan.tree.dirs.len(), 1, "src, images and the hidden and ignored folders are left out");
        assert_eq!(first_file(&scan), Some(p("README.md")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn first_file_without_a_readme() {
        let scan = Scan { tree: build(&[p("docs/a.md")]), order: vec![p("docs/a.md")], truncated: false };
        assert_eq!(first_file(&scan), Some(p("docs/a.md")));
    }
}
