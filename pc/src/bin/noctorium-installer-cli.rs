//! Installs Noctorium from a terminal.
//!
//! The same install the window does, with the window taken out and the questions asked in text: which
//! product, which format, and whether to go ahead, every one with a default that `--yes` takes. Built as
//! a console program, so on Windows it runs in the console it was started from -- cmd, PowerShell,
//! Windows Terminal -- and waits for it, rather than detaching the way a window program does.
//!
//! Everything it does is in the library; this is only the door.

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let outcome =
        noctorium_installer::cli::main(&arguments, noctorium_installer::cli::Launch::Terminal);
    noctorium_installer::cli::hold_if_double_clicked();
    outcome
}
