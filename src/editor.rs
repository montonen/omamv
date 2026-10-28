//! `e`: the open file in the reader's editor (ADR-0021). omamv still never
//! writes; it hands the file to the program the reader chose, and only when
//! they press the key.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Omarchy's own launcher: the editor picked in Omarchy's defaults, in a
/// terminal when it is a terminal editor.
const OMARCHY: &str = "omarchy-launch-editor";

/// The words of a command line: spaces split, quotes keep spaces together.
pub fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
                any = false;
            }
            (None, c) => word.push(c),
        }
    }
    if any || !word.is_empty() {
        out.push(word);
    }
    out
}

/// A program by name on `PATH`, or in Omarchy's own folder: an app started
/// from the launcher may have a shorter `PATH` than a terminal.
fn find(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let p = PathBuf::from(program);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain([PathBuf::from("/usr/share/omarchy/bin")])
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

/// The command `e` runs for `setting` (empty: Omarchy's default), without
/// the file. `Err` says why there is none.
pub fn command(setting: &str) -> Result<Vec<String>, String> {
    let mut words = words(setting);
    if words.is_empty() {
        words = vec![OMARCHY.to_owned()];
    }
    match find(&words[0]) {
        Some(p) => {
            words[0] = p.to_string_lossy().into_owned();
            Ok(words)
        }
        None if setting.trim().is_empty() => Err("no editor set, and Omarchy's is not here (Ctrl+, sets one)".into()),
        None => Err(format!("editor {} not found (Ctrl+, sets another)", words[0])),
    }
}

/// Open `file` in the editor; the answer is the editor's name for the note.
/// The editor runs on its own: closing omamv leaves it open.
pub fn open(setting: &str, file: &Path) -> Result<String, String> {
    let words = command(setting)?;
    let name = if setting.trim().is_empty() {
        "Omarchy's editor".to_owned()
    } else {
        Path::new(&words[0]).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    };
    let mut cmd = Command::new(&words[0]);
    cmd.args(&words[1..]).arg(file).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn().map_err(|e| format!("{name} did not start: {e}"))?;
    // Reaped when it ends, so it never lingers as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lines_split_like_a_shell() {
        assert_eq!(words("code --wait"), ["code", "--wait"]);
        assert_eq!(words(r#"  "/opt/My Editor/bin/ed"  -n 'a b' "" "#), ["/opt/My Editor/bin/ed", "-n", "a b", ""]);
        assert!(words("   ").is_empty());
    }

    #[test]
    fn a_missing_editor_says_so() {
        let e = command("no-such-editor-omamv").unwrap_err();
        assert!(e.contains("no-such-editor-omamv not found"), "{e}");
        let sh = command("sh -c true").unwrap();
        assert!(sh[0].ends_with("/sh") && sh[1..] == ["-c", "true"], "{sh:?}");
    }
}
