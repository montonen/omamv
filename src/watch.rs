//! Following the disk (ADR-0009): inotify on the whole folder, reduced to
//! what matters to a reader.
//!
//! Reading a file shows up as an access event; counting those would reload
//! a page because it was read, forever. Only creations, removals, renames and
//! writes count, and nothing under `.git`.

use std::path::{Path, PathBuf};

use notify::{EventKind, RecursiveMode, Watcher, event::ModifyKind};

/// A path under the folder changed.
#[derive(Debug, Clone)]
pub struct Change {
    pub path: PathBuf,
}

/// Watch `root`; changes arrive on the returned channel until the watcher is
/// dropped.
pub fn start(root: &Path) -> Option<(notify::RecommendedWatcher, async_channel::Receiver<Change>)> {
    let (tx, rx) = async_channel::unbounded();
    let mut watcher = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
        let Ok(ev) = ev else { return };
        if !counts(&ev.kind) {
            return;
        }
        for path in ev.paths {
            if path.components().any(|c| c.as_os_str() == ".git") {
                continue;
            }
            let _ = tx.send_blocking(Change { path });
        }
    })
    .ok()?;
    watcher.watch(root, RecursiveMode::Recursive).ok()?;
    Some((watcher, rx))
}

pub fn counts(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_) | ModifyKind::Any)
    )
}

/// Does this change touch what the tree shows: a shown file, or a folder?
pub fn touches_tree(path: &Path) -> bool {
    crate::folder::is_shown(path) || path.extension().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, DataChange, MetadataKind};

    #[test]
    fn reading_is_not_a_change() {
        assert!(!counts(&EventKind::Access(AccessKind::Read)));
        assert!(!counts(&EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any))));
        assert!(counts(&EventKind::Create(CreateKind::File)));
        assert!(counts(&EventKind::Modify(ModifyKind::Data(DataChange::Content))));
    }
}
