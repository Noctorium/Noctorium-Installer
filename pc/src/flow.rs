//! The install itself, in two halves, with nothing in it that knows whether there is a window.
//!
//! Split where a person would want it split. [`discover`] only asks questions -- what the latest release
//! is, which file belongs here, what it should hash to -- and changes nothing, so a window can show what
//! is about to happen and wait to be told to go on. [`carry_out`] is everything that touches the machine.
//!
//! The console front end runs the two back to back; the window puts a button between them. Both get the
//! same sequence and the same failures, which is the point of it living here rather than in either.

use crate::fetch;
use crate::github::{self, Asset, Problem, Release, Wanted};
use crate::install::{self, Installer};

/// What is about to happen, settled before anything is downloaded.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The release's tag, as published: `v0.4.1`.
    pub tag: String,
    /// The one file of that release that belongs on this machine.
    pub asset: Asset,
    /// What that file must hash to, taken from the release's own `SHA256SUMS.txt`.
    pub published: String,
    pub installer: Installer,
    /// How this machine will be asked for administrator rights, if it has to be.
    pub escalation: Option<&'static str>,
}

impl Plan {
    pub fn megabytes(&self) -> f64 {
        self.asset.size as f64 / 1_048_576.0
    }

    /// Whether going ahead will put a password prompt in front of somebody.
    pub fn needs_password(&self) -> bool {
        self.escalation.is_some()
    }

    /// The version as somebody reads it, without the tag's leading v.
    pub fn version(&self) -> &str {
        self.tag.strip_prefix('v').unwrap_or(&self.tag)
    }
}

/// How far [`carry_out`] has got.
#[derive(Debug, Clone)]
pub enum Step {
    Downloading {
        done: u64,
        total: u64,
    },
    /// The download is complete and hashes to what the release said it would.
    Verified,
    /// About to hand the file over. The command is given so it can be shown before it runs.
    Installing {
        command: String,
    },
}

const NO_PACKAGE_MANAGER: &str =
    "This machine has neither apt nor dnf, so there is no package manager to install with. Download \
     the .deb or .rpm from the releases page and install it the way this distribution expects.";

/// Works out what would be installed, without installing it or writing anything to disk.
pub fn discover() -> Result<Plan, Problem> {
    let installer = install::detect().ok_or_else(|| Problem::Local(NO_PACKAGE_MANAGER.into()))?;
    let wanted = match installer {
        Installer::WindowsSetup => Wanted::WindowsSetup,
        Installer::Apt => Wanted::DebianPackage,
        Installer::Dnf => Wanted::FedoraPackage,
    };

    let release = Release::from_json(&fetch::latest_release()?)?;
    let asset = release
        .asset_for(wanted)
        .ok_or(Problem::NothingForThisMachine)?
        .clone();
    let checksums = release.checksums().ok_or(Problem::NoChecksums)?;

    // Fetched here rather than after the download, so a release whose checksums cannot be read is found
    // out about before three hundred megabytes have been pulled down for nothing.
    let listing = fetch::text(&checksums.url)?;
    let published =
        github::published_checksum(&listing, &asset.name).ok_or(Problem::NoChecksums)?;

    Ok(Plan {
        tag: release.tag,
        asset,
        published,
        installer,
        escalation: install::escalation(),
    })
}

/// Downloads what [`discover`] found, checks it, installs it, and clears up after itself.
pub fn carry_out(plan: &Plan, report: &mut dyn FnMut(Step)) -> Result<(), Problem> {
    // Downloaded into a folder of this program's own, so a half-finished download is never left in the
    // middle of somebody's Downloads folder.
    let directory = std::env::temp_dir().join("noctorium-installer");
    std::fs::create_dir_all(&directory)
        .map_err(|e| Problem::Local(format!("Could not make a folder to download into: {e}")))?;

    let (file, actual) = fetch::download(
        &plan.asset.url,
        &directory,
        &plan.asset.name,
        plan.asset.size,
        |done, total| report(Step::Downloading { done, total }),
    )?;

    if actual != plan.published {
        // Removed rather than left about: a file that failed its checksum is the one file nobody should
        // be able to run by accident afterwards.
        let _ = std::fs::remove_file(&file);
        return Err(Problem::WrongChecksum {
            name: plan.asset.name.clone(),
        });
    }
    report(Step::Verified);

    let (program, args) = install::command_for(plan.installer, &file, plan.escalation);
    report(Step::Installing {
        command: format!("{program} {}", args.join(" "))
            .trim_end()
            .to_string(),
    });
    install::run(&program, &args)?;

    // The downloaded package is several hundred megabytes and has done its job.
    let _ = std::fs::remove_file(&file);
    Ok(())
}
