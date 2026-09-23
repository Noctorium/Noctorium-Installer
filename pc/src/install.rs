//! Handing the downloaded package to whatever installs software on this machine.

use crate::github::Problem;
use std::path::Path;
use std::process::Command;

/// What this machine installs software with, and therefore which file it wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installer {
    /// Windows: run the installer and let it do its own asking.
    WindowsSetup,
    /// Debian and its relatives: apt, so dependencies are resolved rather than merely reported.
    Apt,
    /// Fedora and its relatives.
    Dnf,
}

/// Which of the above this machine has, decided by looking rather than by guessing from the distribution.
///
/// A machine can have both apt and dnf -- and one of them broken -- so the order matters: whichever
/// package manager owns the system is the one whose tool is on PATH first in practice, and apt is asked
/// about first because a Debian family machine with dnf installed is much likelier than the reverse.
pub fn detect() -> Option<Installer> {
    if cfg!(windows) {
        return Some(Installer::WindowsSetup);
    }
    [("apt-get", Installer::Apt), ("dnf", Installer::Dnf)]
        .into_iter()
        .find(|(tool, _)| on_path(tool))
        .map(|(_, installer)| installer)
}

fn on_path(tool: &str) -> bool {
    // `which` is not everywhere, but PATH is: this asks the same question without depending on a program
    // to answer it.
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|dir| {
                let candidate = dir.join(tool);
                candidate.is_file()
            })
        })
        .unwrap_or(false)
}

/// The command that will install [file], as a program and its arguments.
///
/// Built rather than run, so the caller can print it before anything happens -- an installer that asks
/// for a password should have said what it is about to do first -- and so it can be tested without
/// installing anything.
pub fn command_for(
    installer: Installer,
    file: &Path,
    escalate_with: Option<&str>,
) -> (String, Vec<String>) {
    let path = file.to_string_lossy().to_string();
    match installer {
        Installer::WindowsSetup => (path, vec![]),
        Installer::Apt => {
            // `apt-get install ./file.deb` rather than `dpkg -i`: dpkg reports missing dependencies and
            // stops, apt fetches them. The leading ./ is what tells apt this is a file and not a name.
            let args = vec!["install".to_string(), "-y".to_string(), local_path(&path)];
            escalated("apt-get", args, escalate_with)
        }
        Installer::Dnf => {
            let args = vec!["install".to_string(), "-y".to_string(), local_path(&path)];
            escalated("dnf", args, escalate_with)
        }
    }
}

/// A path apt will read as a file. Without a directory in front of it, `noctorium.deb` is a package name.
fn local_path(path: &str) -> String {
    if path.starts_with('/') || path.starts_with("./") {
        path.to_string()
    } else {
        format!("./{path}")
    }
}

fn escalated(
    program: &str,
    args: Vec<String>,
    escalate_with: Option<&str>,
) -> (String, Vec<String>) {
    match escalate_with {
        Some(sudo) => {
            let mut all = vec![program.to_string()];
            all.extend(args);
            (sudo.to_string(), all)
        }
        None => (program.to_string(), args),
    }
}

/// How this machine asks for administrator rights, or nothing if it is already running with them.
///
/// pkexec first: it puts up the desktop's own password window, which is what somebody double clicking an
/// installer expects. sudo is the fallback for a terminal. Running as root already, neither is wanted.
pub fn escalation() -> Option<&'static str> {
    if cfg!(windows) {
        return None;
    }
    // Only root has uid 0, and only root can write to /usr without help.
    let already_root = std::env::var("USER").map(|u| u == "root").unwrap_or(false)
        || std::env::var("HOME").map(|h| h == "/root").unwrap_or(false);
    if already_root {
        return None;
    }
    ["pkexec", "sudo"].into_iter().find(|tool| on_path(tool))
}

/// Runs the install and waits for it.
///
/// The exit status is reported rather than interpreted: an installer that was cancelled and one that
/// failed both come back non-zero, and telling somebody their own cancellation was an error is worse
/// than saying plainly what happened.
pub fn run(program: &str, args: &[String]) -> Result<(), Problem> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| Problem::Local(format!("Could not start {program}: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Problem::Local(format!(
            "{program} finished with {}. Nothing was installed, or the install was cancelled.",
            status
                .code()
                .map(|c| format!("exit code {c}"))
                .unwrap_or_else(|| "no exit code".into())
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn windows_runs_the_installer_itself() {
        let (program, args) = command_for(
            Installer::WindowsSetup,
            &PathBuf::from(r"C:\Temp\Noctorium-setup.exe"),
            None,
        );
        assert_eq!(program, r"C:\Temp\Noctorium-setup.exe");
        assert!(args.is_empty(), "the installer asks its own questions");
    }

    #[test]
    fn a_deb_goes_through_apt_so_dependencies_are_fetched_rather_than_reported() {
        let (program, args) =
            command_for(Installer::Apt, &PathBuf::from("/tmp/noctorium.deb"), None);
        assert_eq!(program, "apt-get");
        assert_eq!(args, vec!["install", "-y", "/tmp/noctorium.deb"]);
    }

    #[test]
    fn a_bare_file_name_is_made_into_a_path_so_apt_does_not_read_it_as_a_package() {
        let (_, args) = command_for(Installer::Apt, &PathBuf::from("noctorium.deb"), None);
        assert_eq!(args.last().unwrap(), "./noctorium.deb");
    }

    #[test]
    fn asking_for_rights_puts_the_tool_in_front_and_keeps_the_order() {
        let (program, args) =
            command_for(Installer::Dnf, &PathBuf::from("/tmp/n.rpm"), Some("pkexec"));
        assert_eq!(program, "pkexec");
        assert_eq!(args, vec!["dnf", "install", "-y", "/tmp/n.rpm"]);
    }
}
