//! omamv — the Omarchy Markdown Viewer: a read-only, keyboard-first viewer
//! for a folder of Markdown and text files.

mod folder;
mod fuzzy;
mod recent;
mod render;
mod scheme;
mod theme;
mod ui;
mod watch;

use std::io::IsTerminal;
use std::path::PathBuf;

use adw::prelude::*;
use gtk::{gio, glib};

pub const APP_ID: &str = "se.montonen.Omamv";

const USAGE: &str = "\
omamv — read a folder of Markdown, rendered

Usage: omamv [options] [folder | file]...

  omamv .            the current folder
  omamv docs/        that folder
  omamv README.md    its folder, with the file shown
  omamv              the start screen: recent folders

Options:
  -w, --wait      Stay in the terminal until the window closes
  -h, --help      Show this help
  -V, --version   Show the version

omamv only reads. Press ? in the window for the keys.
";

/// Set on the copy of omamv that a terminal launch hands over to.
const DETACHED: &str = "OMAMV_DETACHED";

fn main() -> glib::ExitCode {
    let mut wait = false;
    let mut paths: Vec<String> = Vec::new();
    for arg in std::env::args_os().skip(1) {
        let Some(arg) = arg.to_str().map(str::to_owned) else {
            eprintln!("omamv: a path that is not UTF-8 cannot be shown");
            return glib::ExitCode::FAILURE;
        };
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("omamv {}", env!("CARGO_PKG_VERSION"));
                return glib::ExitCode::SUCCESS;
            }
            "-w" | "--wait" => wait = true,
            "--" => {}
            other if other.starts_with('-') && other.len() > 1 => {
                eprint!("omamv: unknown option {other}\n\n{USAGE}");
                return glib::ExitCode::FAILURE;
            }
            other => paths.push(other.to_owned()),
        }
    }
    for p in &paths {
        if !std::path::Path::new(p).exists() {
            eprintln!("omamv: {p}: no such file or folder");
            return glib::ExitCode::FAILURE;
        }
    }

    // Like `code .`: from a terminal, hand over to a copy of omamv and give
    // the prompt back at once.
    if !wait && std::env::var_os(DETACHED).is_none() && std::io::stdout().is_terminal() && !ui::debug::driven() {
        if detach() {
            return glib::ExitCode::SUCCESS;
        }
    }

    let mut flags = gio::ApplicationFlags::HANDLES_COMMAND_LINE;
    if ui::debug::driven() {
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder().application_id(APP_ID).flags(flags).build();
    app.connect_startup(|app| ui::startup(app));
    app.connect_activate(|app| ui::open(app, None));
    app.connect_command_line(|app, cmd| {
        let cwd = cmd.cwd().unwrap_or_else(|| PathBuf::from("."));
        let targets: Vec<PathBuf> = cmd
            .arguments()
            .into_iter()
            .skip(1)
            .filter_map(|a| a.to_str().map(str::to_owned))
            .filter(|a| !a.starts_with('-'))
            .map(|a| cwd.join(a))
            .collect();
        if targets.is_empty() {
            ui::open(app, None);
        }
        for t in targets {
            ui::open(app, Some(t.canonicalize().unwrap_or(t)));
        }
        glib::ExitCode::SUCCESS
    });
    // The arguments were checked above; GApplication hands them to the first
    // instance, which may be another process already running.
    let mut argv = vec!["omamv".to_owned()];
    argv.extend(paths);
    app.run_with_args(&argv)
}

/// Start a copy of omamv in its own process group, away from the terminal,
/// with the same arguments. False if it could not be started; then this
/// process carries on itself.
fn detach() -> bool {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let Ok(exe) = std::env::current_exe() else { return false };
    Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env(DETACHED, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .is_ok()
}
