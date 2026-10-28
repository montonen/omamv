//! Following the disk (ADR-0009). Only the folders the scan walked are
//! watched, each on its own: build output, `.git` and whatever `.gitignore`
//! leaves out cost nothing, and a new folder is added at the next scan.
//!
//! Reading a file shows up as an access event; counting those would reload
//! a page because it was read, forever. Only creations, removals, renames and
//! writes count.

use std::collections::HashSet;
use std::path::PathBuf;

use notify::{EventKind, RecursiveMode, Watcher, event::ModifyKind};

/// What happened to a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Its content was written.
    Written,
    /// It was made, removed or renamed: the tree may change.
    Moved,
}

#[derive(Debug, Clone)]
pub struct Change {
    pub path: PathBuf,
    pub kind: Kind,
}

pub struct Watch {
    watcher: notify::RecommendedWatcher,
    watched: HashSet<PathBuf>,
    /// The open file's repository folder (`.git`), for its state (ADR-0022).
    extra: Option<PathBuf>,
}

impl Watch {
    /// A watcher with nothing watched yet; changes arrive on the channel.
    pub fn new() -> Option<(Watch, async_channel::Receiver<Change>)> {
        let (tx, rx) = async_channel::unbounded();
        let watcher = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
            let Ok(ev) = ev else { return };
            let Some(kind) = kind_of(&ev.kind) else { return };
            for path in ev.paths {
                let _ = tx.send_blocking(Change { path, kind });
            }
        })
        .ok()?;
        Some((Watch { watcher, watched: HashSet::new(), extra: None }, rx))
    }

    /// Watch exactly `dirs`, and the extra folder: add the new ones, drop
    /// the gone ones.
    pub fn follow(&mut self, dirs: &[PathBuf]) {
        let mut dirs = dirs.to_vec();
        dirs.extend(self.extra.clone());
        let dirs = &dirs[..];
        let wanted: HashSet<&PathBuf> = dirs.iter().collect();
        let gone: Vec<PathBuf> = self.watched.iter().filter(|d| !wanted.contains(d)).cloned().collect();
        for d in gone {
            let _ = self.watcher.unwatch(&d);
            self.watched.remove(&d);
        }
        for d in dirs {
            if !self.watched.contains(d) && self.watcher.watch(d, RecursiveMode::NonRecursive).is_ok() {
                self.watched.insert(d.clone());
            }
        }
    }

    /// Watch `dir` as well as the scan's folders, in place of the last extra.
    pub fn set_extra(&mut self, dir: Option<PathBuf>, dirs: &[PathBuf]) {
        if self.extra != dir {
            self.extra = dir;
            self.follow(dirs);
        }
    }

    pub fn count(&self) -> usize {
        self.watched.len()
    }
}

pub fn kind_of(kind: &EventKind) -> Option<Kind> {
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)) => Some(Kind::Moved),
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Any) => Some(Kind::Written),
        _ => None,
    }
}

/// Does this change touch what the tree shows: a shown file, or a folder,
/// made, removed or renamed?
pub fn touches_tree(change: &Change) -> bool {
    change.kind == Kind::Moved && (crate::folder::is_shown(&change.path) || change.path.extension().is_none() || change.path.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, DataChange, MetadataKind, RenameMode};

    #[test]
    fn reading_is_not_a_change() {
        assert_eq!(kind_of(&EventKind::Access(AccessKind::Read)), None);
        assert_eq!(kind_of(&EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any))), None);
        assert_eq!(kind_of(&EventKind::Create(CreateKind::File)), Some(Kind::Moved));
        assert_eq!(kind_of(&EventKind::Modify(ModifyKind::Name(RenameMode::To))), Some(Kind::Moved));
        assert_eq!(kind_of(&EventKind::Modify(ModifyKind::Data(DataChange::Content))), Some(Kind::Written));
    }

    #[test]
    fn a_save_is_not_a_tree_change() {
        let save = Change { path: PathBuf::from("/x/a.md"), kind: Kind::Written };
        assert!(!touches_tree(&save));
        let new = Change { path: PathBuf::from("/x/b.md"), kind: Kind::Moved };
        assert!(touches_tree(&new));
        let build = Change { path: PathBuf::from("/x/out.o"), kind: Kind::Moved };
        assert!(!touches_tree(&build));
    }
}
