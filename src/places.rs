//! Where the reader was in each folder (ADR-0019): the file read last and the
//! place in it. omamv's own state, kept beside the recent folders in
//! `$XDG_STATE_HOME/omamv/places`; never a file in a folder it shows
//! (ADR-0002).
//!
//! One line a folder: `when⇥folder⇥file⇥heading id⇥offset⇥y`, newest first.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Folders remembered; the oldest is forgotten first.
pub const KEEP: usize = 500;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spot {
    /// From the folder's root.
    pub file: PathBuf,
    pub id: String,
    pub off: i64,
    pub y: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    when: u64,
    folder: PathBuf,
    spot: Spot,
}

fn file() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute())
        .unwrap_or_else(|| std::env::home_dir().unwrap_or_default().join(".local/state"));
    state.join("omamv/places")
}

fn parse(text: &str) -> Vec<Line> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            let when = f.next()?.parse().ok()?;
            let folder = PathBuf::from(f.next()?);
            let file = PathBuf::from(f.next()?);
            let id = f.next().unwrap_or("").to_owned();
            let off = f.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let y = f.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            (folder.is_absolute() && !file.as_os_str().is_empty()).then_some(Line { when, folder, spot: Spot { file, id, off, y } })
        })
        .collect()
}

fn write(lines: &[Line]) {
    let text: String = lines
        .iter()
        .map(|l| format!("{}\t{}\t{}\t{}\t{}\t{}\n", l.when, l.folder.display(), l.spot.file.display(), l.spot.id, l.spot.off, l.spot.y))
        .collect();
    let f = file();
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = f.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &f);
    }
}

fn clean(s: &str) -> bool {
    !s.contains(['\t', '\n', '\r'])
}

/// The file read last in `folder`, and where.
pub fn get(folder: &Path) -> Option<Spot> {
    parse(&std::fs::read_to_string(file()).unwrap_or_default()).into_iter().find(|l| l.folder == folder).map(|l| l.spot)
}

/// Remember `spot` for `folder`. A failure to write is not worth telling anyone.
pub fn set(folder: &Path, spot: &Spot) {
    let names = [folder.to_string_lossy(), spot.file.to_string_lossy()];
    if !names.iter().all(|n| clean(n)) || !clean(&spot.id) {
        return;
    }
    let mut lines = parse(&std::fs::read_to_string(file()).unwrap_or_default());
    if lines.first().is_some_and(|l| l.folder == folder && l.spot == *spot) {
        return;
    }
    lines.retain(|l| l.folder != folder);
    let when = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    lines.insert(0, Line { when, folder: folder.to_path_buf(), spot: spot.clone() });
    lines.truncate(KEEP);
    write(&lines);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_writes_and_skips_nonsense() {
        let lines = parse("5\t/a b\tdocs/x.md\tintro\t12\t300\nbad\n6\trelative\tx.md\n7\t/c\tREADME.md\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spot, Spot { file: "docs/x.md".into(), id: "intro".into(), off: 12, y: 300 });
        assert_eq!(lines[1].spot, Spot { file: "README.md".into(), ..Spot::default() });
    }
}
