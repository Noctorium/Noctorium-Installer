//! Installs Noctorium on this machine.
//!
//! One small program that does one thing: ask GitHub what the latest release is, download the file for
//! this machine, check it against the checksum published beside it, and hand it to whatever installs
//! software here. It is what somebody downloads once; everything after that is the application's own
//! updater.
//!
//! It opens a window. That costs a few megabytes of toolkit on top of what is otherwise a download and a
//! checksum -- but this is the first thing anybody sees of Noctorium, often before they have any reason
//! to trust it, and a console window full of scrolling text is not what somebody who has just downloaded
//! a music player is expecting. The console version is still here behind `--cli`, and is what runs on a
//! machine with no display at all.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod console;
mod fetch;
mod flow;
mod github;
mod gui;
mod install;

use std::process::ExitCode;

const USAGE: &str = "\
Noctorium installer

With no arguments it opens a window. On a machine with no display it explains itself in the terminal
instead, which --cli also asks for directly.

  --cli     install here in the terminal rather than in a window
  --help    this

  GITHUB_TOKEN           raises GitHub's rate limit, and reads a repository that is not public
  NOCTORIUM_REPOSITORY   the repository to install from, as owner/name

It exits 0 when Noctorium is installed and 1 when it is not, including when the window was closed
without installing anything.
";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let said = |flag: &str| arguments.iter().any(|argument| argument == flag);

    if said("--help") || said("-h") {
        attach_console();
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    if !said("--cli") && !said("--console") && there_is_a_display() {
        match gui::run() {
            Ok(true) => return ExitCode::SUCCESS,
            // The window was closed without Noctorium being installed -- either it failed, and the
            // window said why, or somebody thought better of it. Neither is worth repeating here.
            Ok(false) => return ExitCode::FAILURE,
            Err(why) => {
                // A display that answered but would not give us a window. Rare, and recoverable: the
                // whole install works perfectly well as text.
                attach_console();
                eprintln!("No window could be opened ({why}), so this is the terminal version.\n");
            }
        }
    }

    attach_console();
    match console::run() {
        Ok(()) => {
            console::pause_if_double_clicked();
            ExitCode::SUCCESS
        }
        Err(problem) => {
            eprintln!("\n{problem}");
            console::pause_if_double_clicked();
            ExitCode::FAILURE
        }
    }
}

/// Whether there is anything to put a window on.
///
/// Windows always has one. Elsewhere this is the difference between a desktop and an ssh session, and
/// getting it wrong means a program that appears to do nothing at all.
fn there_is_a_display() -> bool {
    if cfg!(windows) {
        return true;
    }
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Puts this program's output back into the terminal it was started from.
///
/// On Windows it is built as a window program, so that double clicking it does not flash up a console
/// before the window appears. The cost is that it starts with no console at all, and `--cli` would
/// otherwise print into nothing. Borrowing the console of whoever started it fixes that. Output that was
/// redirected to a file or a pipe arrives either way, which is why `> log.txt` has always worked.
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // Fails when there is no console to attach to -- started from Explorer, say -- which is not a
    // problem, because in that case there is nobody reading either.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

#[cfg(not(windows))]
fn attach_console() {}
