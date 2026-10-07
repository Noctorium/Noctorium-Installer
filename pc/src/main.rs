//! Installs Noctorium on this machine, in a window.
//!
//! One small program that does one thing: ask GitHub what the latest release is, download the file for
//! this machine, check it against the checksum published beside it, and hand it to whatever installs
//! software here. It is what somebody downloads once; everything after that is the application's own
//! updater.
//!
//! It opens a window. That costs a few megabytes of toolkit on top of what is otherwise a download and a
//! checksum -- but this is the first thing anybody sees of Noctorium, often before they have any reason
//! to trust it, and a console window full of scrolling text is not what somebody who has just downloaded
//! a music player is expecting. The terminal version is `noctorium-installer-cli`, built from the same
//! library; this one still falls back to it behind `--cli`, and on a machine with no display at all.
//!
//! A Mac is always the second case. There is no window for one -- the toolkit is not even built there,
//! see Cargo.toml -- so on a Mac this program is the terminal installer under another name, and the
//! release ships only `noctorium-installer-cli` for it.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(target_os = "macos"))]
mod gui;

use noctorium_installer::cli::{self, Launch};
use std::process::ExitCode;

const USAGE: &str = "\
Noctorium installer

With no arguments it opens a window, which offers Noctorium, the Noctorium CLI and Noctorium Stats, any
or all of them. On a machine with no display it installs in the terminal instead, without asking
anything, which --cli also asks for directly. For menus, a dry run and the rest, use
noctorium-installer-cli, which is this installer for a terminal.

  --cli     install here in the terminal rather than in a window, taking the defaults
  --help    this

  With --cli, the options of noctorium-installer-cli are understood too: --product, --format,
  --wizard, --version, --download-only, --dry-run, --list, --no-color.

  On Windows Noctorium is installed from its .msi, with
  msiexec /i <msi> /passive /norestart MSIFASTINSTALL=7, and INSTALLDIR=<folder> when it is installed
  already. The window has a box to tick for the setup's own wizard instead, to choose the folder.

  GITHUB_TOKEN           raises GitHub's rate limit, and reads a repository that is not public
  NOCTORIUM_PRODUCT      with --cli, what --product would say: desktop, cli, stats, both or all
  NOCTORIUM_REPOSITORY   the repository to install from, as owner/name
  NOCTORIUM_NO_PATH      unpack the Noctorium CLI, and leave your PATH, and Windows' list of installed
                         apps, as they are

It exits 0 when everything chosen is installed and 1 when it is not, including when the window was
closed without installing anything, and 2 when --cli was given options it could not understand.
";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let said = |flag: &str| arguments.iter().any(|argument| argument == flag);

    if said("--help") || said("-h") {
        attach_console();
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let console = said("--cli") || said("--console");
    if !console && there_is_a_display() {
        match open_window() {
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
    // Everything but the switch into the terminal is handed on, so `--cli --format appimage` works. With
    // nothing else -- which is how this has always been run -- it installs with the defaults, as before.
    let rest: Vec<String> = arguments
        .into_iter()
        .filter(|argument| argument != "--cli" && argument != "--console")
        .collect();
    let outcome = cli::main(&rest, Launch::Window);
    pause_if_double_clicked();
    outcome
}

/// Whether there is anything to put a window on.
///
/// Windows always has one. On Linux this is the difference between a desktop and an ssh session, and
/// getting it wrong means a program that appears to do nothing at all. A Mac never has one for this
/// program, whatever DISPLAY says -- XQuartz sets it -- because the window is not built there.
fn there_is_a_display() -> bool {
    if cfg!(windows) {
        return true;
    }
    if cfg!(target_os = "macos") {
        return false;
    }
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Opens the window and returns whether everything chosen ended up installed.
#[cfg(not(target_os = "macos"))]
fn open_window() -> Result<bool, String> {
    gui::run()
}

/// Never reached, since a Mac has no display as far as this program is concerned; here so that the one
/// place a window is opened does not have to be written twice.
#[cfg(target_os = "macos")]
fn open_window() -> Result<bool, String> {
    Err("there is no window installer for macOS".into())
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

/// Holds the window open when there is nobody to read the output otherwise.
///
/// A console program started from Explorer gets its own window, which closes the instant it exits --
/// taking the only explanation of what went wrong with it. Reached now only when the window could not be
/// opened at all, which is the moment an explanation matters most.
fn pause_if_double_clicked() {
    if !cfg!(windows) || std::env::var_os("NOCTORIUM_NO_PAUSE").is_some() {
        return;
    }
    // Arguments mean somebody typed this, and a terminal that was already open does not need holding.
    if std::env::args().len() > 1 {
        return;
    }
    println!("\nPress Enter to close.");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}
