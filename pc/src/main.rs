//! Installs Noctorium on this machine.
//!
//! One small program that does one thing: ask GitHub what the latest release is, download the file for
//! this machine, check it against the checksum published beside it, and hand it to whatever installs
//! software here. It is what somebody downloads once; everything after that is the application's own
//! updater.
//!
//! It is deliberately a console program. An installer that puts up a window has to carry a window
//! toolkit, and this is three hundred kilobytes of download-and-verify that would become several
//! megabytes of framework to draw a progress bar that the console already draws.

mod fetch;
mod github;
mod install;

use github::{Problem, Release, Wanted};
use install::Installer;
use std::process::ExitCode;

fn main() -> ExitCode {
    println!("Noctorium installer {}\n", env!("CARGO_PKG_VERSION"));
    match run() {
        Ok(()) => {
            println!("\nNoctorium is installed.");
            pause_if_double_clicked();
            ExitCode::SUCCESS
        }
        Err(problem) => {
            eprintln!("\n{problem}");
            pause_if_double_clicked();
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Problem> {
    let installer = install::detect().ok_or_else(|| {
        Problem::Local(
            "This machine has neither apt nor dnf, so there is no package manager to install with. \
             Download the .deb or .rpm from the releases page and install it the way this distribution \
             expects."
                .into(),
        )
    })?;
    let wanted = match installer {
        Installer::WindowsSetup => Wanted::WindowsSetup,
        Installer::Apt => Wanted::DebianPackage,
        Installer::Dnf => Wanted::FedoraPackage,
    };

    println!("Asking GitHub for the latest release...");
    let release = Release::from_json(&fetch::latest_release()?)?;
    let asset = release
        .asset_for(wanted)
        .ok_or(Problem::NothingForThisMachine)?;
    let checksums = release.checksums().ok_or(Problem::NoChecksums)?;
    println!(
        "  {} — {} ({:.0} MB)\n",
        release.tag,
        asset.name,
        asset.size as f64 / 1_048_576.0
    );

    // Downloaded next to each other in a folder of this program's own, so a half-finished download is
    // never left in the middle of somebody's Downloads folder.
    let directory = std::env::temp_dir().join("noctorium-installer");
    std::fs::create_dir_all(&directory)
        .map_err(|e| Problem::Local(format!("Could not make a folder to download into: {e}")))?;

    let listing = fetch::text(&checksums.url)?;
    let published =
        github::published_checksum(&listing, &asset.name).ok_or(Problem::NoChecksums)?;

    println!("Downloading {}", asset.name);
    let (file, actual) = fetch::download(&asset.url, &directory, &asset.name, asset.size)?;

    if actual != published {
        // Removed rather than left about: a file that failed its checksum is the one file nobody should
        // be able to run by accident afterwards.
        let _ = std::fs::remove_file(&file);
        return Err(Problem::WrongChecksum {
            name: asset.name.clone(),
        });
    }
    println!("  checksum matches the one published with the release");

    let escalation = install::escalation();
    let (program, args) = install::command_for(installer, &file, escalation);
    if escalation.is_some() {
        println!("\nInstalling. This needs administrator rights, so you will be asked for your password.");
    } else {
        println!("\nInstalling.");
    }
    println!("  {program} {}", args.join(" "));
    install::run(&program, &args)?;

    // The downloaded package is several hundred megabytes and has done its job.
    let _ = std::fs::remove_file(&file);
    Ok(())
}

/// Holds the window open when there is nobody to read the output otherwise.
///
/// A console program started from Explorer gets its own window, which closes the instant it exits --
/// taking the only explanation of what went wrong with it. Started from a terminal there is no need,
/// and waiting would be an annoyance, so this only applies where it helps.
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
