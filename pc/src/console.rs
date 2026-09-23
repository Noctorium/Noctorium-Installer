//! The installer without a window: the same sequence, printed.
//!
//! Kept for the machines a window cannot be opened on -- a server over ssh, a live environment with no
//! display -- and for anybody who would rather watch it in a terminal. `--cli` asks for it by name.

use crate::flow::{self, Step};
use crate::github::Problem;
use std::io::Write;

/// Runs the whole thing, printing as it goes.
pub fn run() -> Result<(), Problem> {
    println!("Noctorium installer {}\n", env!("CARGO_PKG_VERSION"));

    println!("Asking GitHub for the latest release...");
    let plan = flow::discover()?;
    println!(
        "  {} — {} ({:.0} MB)\n",
        plan.tag,
        plan.asset.name,
        plan.megabytes()
    );

    println!("Downloading {}", plan.asset.name);
    flow::carry_out(&plan, &mut |step| match step {
        Step::Downloading { done, total } => report(done, total),
        Step::Verified => println!("\n  checksum matches the one published with the release"),
        Step::Installing { command } => {
            if plan.needs_password() {
                println!(
                    "\nInstalling. This needs administrator rights, so you will be asked for your \
                     password."
                );
            } else {
                println!("\nInstalling.");
            }
            println!("  {command}");
        }
    })?;

    println!("\nNoctorium is installed.");
    Ok(())
}

/// One line, rewritten in place, rather than a page of them.
fn report(written: u64, total: u64) {
    if total > 0 {
        let percent = (written as f64 / total as f64 * 100.0).min(100.0);
        print!(
            "\r  {:.0}% of {:.0} MB",
            percent,
            total as f64 / 1_048_576.0
        );
    } else {
        print!("\r  {:.0} MB", written as f64 / 1_048_576.0);
    }
    let _ = std::io::stdout().flush();
}

/// Holds the window open when there is nobody to read the output otherwise.
///
/// A console program started from Explorer gets its own window, which closes the instant it exits --
/// taking the only explanation of what went wrong with it. Reached now only when the window could not be
/// opened at all, which is the moment an explanation matters most.
pub fn pause_if_double_clicked() {
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
