//! Recent folders, for the start screen. omamv's own state, kept under
//! `$XDG_STATE_HOME/omamv/recent`: it never writes to a folder it shows
//! (ADR-0002).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const KEEP: usize = 9;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recent {
    pub path: PathBuf,
    /// Seconds since the epoch.
    pub when: u64,
}

fn file() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute())
        .unwrap_or_else(|| std::env::home_dir().unwrap_or_default().join(".local/state"));
    state.join("omamv/recent")
}

pub fn load() -> Vec<Recent> {
    parse(&std::fs::read_to_string(file()).unwrap_or_default())
        .into_iter()
        .filter(|r| r.path.is_dir())
        .collect()
}

fn parse(text: &str) -> Vec<Recent> {
    text.lines()
        .filter_map(|line| {
            let (when, path) = line.split_once('\t')?;
            Some(Recent { path: PathBuf::from(path), when: when.parse().ok()? })
        })
        .take(KEEP)
        .collect()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Put `path` first. A failure to write is not worth telling anyone about.
pub fn touch(path: &Path) {
    let mut list = parse(&std::fs::read_to_string(file()).unwrap_or_default());
    list.retain(|r| r.path != path);
    list.insert(0, Recent { path: path.to_path_buf(), when: now() });
    list.truncate(KEEP);
    let text: String = list.iter().map(|r| format!("{}\t{}\n", r.when, r.path.display())).collect();
    let f = file();
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = f.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &f);
    }
}

/// "today", "yesterday", "3 days ago", "5 weeks ago".
pub fn when(then: u64) -> String {
    let days = now().saturating_sub(then) / 86_400;
    match days {
        0 => "today".into(),
        1 => "yesterday".into(),
        2..=13 => format!("{days} days ago"),
        14..=59 => format!("{} weeks ago", days / 7),
        _ => format!("{} months ago", days / 30),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_writes() {
        let list = parse("100\t/a\nbad line\n200\t/b c\n");
        assert_eq!(list, [Recent { path: "/a".into(), when: 100 }, Recent { path: "/b c".into(), when: 200 }]);
    }

    #[test]
    fn says_when_in_words() {
        assert_eq!(when(now()), "today");
        assert_eq!(when(now() - 86_400 * 3), "3 days ago");
    }
}
