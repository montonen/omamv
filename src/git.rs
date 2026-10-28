//! The open file in git (ADR-0022): its state, and the last commit that
//! changed it. Read-only, through libgit2: omamv runs no `git`, and asks
//! nothing that writes (a status never refreshes the index).

use std::path::{Path, PathBuf};

/// Commits looked through for the last one that changed the file; past
/// that, a long history goes unnamed rather than slow.
const WALK: usize = 2_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Clean,
    Modified,
    Staged,
    /// Staged, and changed again since.
    StagedModified,
    /// Not in git yet.
    New,
    Ignored,
    Conflicted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Last {
    pub id: String,
    /// Seconds since the epoch.
    pub when: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    /// The repository's own folder (`.git`, or where a worktree's points).
    pub git_dir: PathBuf,
    pub state: State,
    pub last: Option<Last>,
}

fn state_of(s: git2::Status) -> State {
    use git2::Status as S;
    if s.contains(S::CONFLICTED) {
        State::Conflicted
    } else if s.contains(S::IGNORED) {
        State::Ignored
    } else if s.contains(S::WT_NEW) {
        State::New
    } else {
        let staged = s.intersects(S::INDEX_NEW | S::INDEX_MODIFIED | S::INDEX_RENAMED | S::INDEX_TYPECHANGE | S::INDEX_DELETED);
        let worktree = s.intersects(S::WT_MODIFIED | S::WT_RENAMED | S::WT_TYPECHANGE | S::WT_DELETED);
        match (staged, worktree) {
            (true, true) => State::StagedModified,
            (true, false) => State::Staged,
            (false, true) => State::Modified,
            (false, false) => State::Clean,
        }
    }
}

/// The newest commit, within `WALK`, whose tree has a different `rel` from
/// its first parent's.
fn last_change(repo: &git2::Repository, rel: &Path) -> Option<Last> {
    let mut walk = repo.revwalk().ok()?;
    walk.push_head().ok()?;
    walk.set_sorting(git2::Sort::TIME).ok()?;
    let entry = |c: &git2::Commit| c.tree().ok().and_then(|t| t.get_path(rel).ok()).map(|e| e.id());
    for oid in walk.take(WALK) {
        let commit = repo.find_commit(oid.ok()?).ok()?;
        let here = entry(&commit);
        let before = commit.parent(0).ok().and_then(|p| entry(&p));
        if here.is_some() && here != before {
            let id = commit.as_object().short_id().ok().and_then(|b| b.as_str().ok().map(str::to_owned)).unwrap_or_default();
            return Some(Last { id, when: commit.time().seconds() });
        }
    }
    None
}

/// `file` (absolute) in the repository it is in, if any.
pub fn look(file: &Path) -> Option<Look> {
    let repo = git2::Repository::discover(file.parent()?).ok()?;
    let work = repo.workdir()?.canonicalize().ok()?;
    let full = file.canonicalize().ok()?;
    let rel = full.strip_prefix(&work).ok()?.to_path_buf();
    let state = state_of(repo.status_file(&rel).ok()?);
    let last = match state {
        State::New | State::Ignored => None,
        _ => last_change(&repo, &rel),
    };
    Some(Look { git_dir: repo.path().to_path_buf(), state, last })
}

/// The header's few words: "modified · a1b2c3d 2 h ago".
pub fn words(look: &Look, now: i64) -> String {
    let state = match look.state {
        State::Clean => "",
        State::Modified => "modified",
        State::Staged => "staged",
        State::StagedModified => "staged, modified since",
        State::New => "new, not in git",
        State::Ignored => "ignored by git",
        State::Conflicted => "conflicted",
    };
    let last = look
        .last
        .as_ref()
        .map(|l| format!("{} {} ago", l.id, crate::ui::quickopen::age(u64::try_from(now - l.when).unwrap_or(0))))
        .map(|s| s.replace(" now ago", " just now"));
    match (state, last) {
        ("", Some(l)) => l,
        ("", None) => "in git".into(),
        (s, Some(l)) => format!("{s} · {l}"),
        (s, None) => s.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_read_as_git_says_them() {
        use git2::Status as S;
        assert_eq!(state_of(S::CURRENT), State::Clean);
        assert_eq!(state_of(S::WT_MODIFIED), State::Modified);
        assert_eq!(state_of(S::INDEX_MODIFIED), State::Staged);
        assert_eq!(state_of(S::INDEX_MODIFIED | S::WT_MODIFIED), State::StagedModified);
        assert_eq!(state_of(S::WT_NEW), State::New);
        assert_eq!(state_of(S::IGNORED), State::Ignored);
    }

    #[test]
    fn the_header_words() {
        let at = |state, last: Option<i64>| Look { git_dir: PathBuf::new(), state, last: last.map(|when| Last { id: "a1b2c3d".into(), when }) };
        assert_eq!(words(&at(State::Clean, Some(1000 - 7200)), 1000), "a1b2c3d 2 h ago");
        assert_eq!(words(&at(State::Modified, Some(990)), 1000), "modified · a1b2c3d just now");
        assert_eq!(words(&at(State::New, None), 1000), "new, not in git");
    }

    #[test]
    fn a_folder_outside_git_has_nothing_to_say() {
        let dir = std::env::temp_dir().join(format!("omamv-git-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("a.md");
        let _ = std::fs::write(&f, "# a\n");
        // The temp folder is in no repository on any sane machine.
        assert!(look(&f).is_none_or(|l| !l.git_dir.starts_with(&dir)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
