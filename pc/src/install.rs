//! Handing the downloaded file to whatever installs software on this machine, or putting it in place.
//!
//! Packages go to the package manager, always: apt, dnf, zypper and pacman resolve dependencies, record
//! what they installed, and can remove it again, and an installer that copies files into /opt behind
//! their back leaves a machine whose package database is a lie. The per-user formats -- an AppImage, the
//! Noctorium CLI -- have no package manager, so for those this is the installer, and does the little an
//! installer does: put the file somewhere permanent, make it runnable, and tell the desktop or the shell
//! where it is.
//!
//! A Mac has no package manager of its own either, and installing an application there has only ever
//! meant copying it out of its disk image into Applications. This does that, the way a person dragging it
//! across would, minus the window to drag it in.
//!
//! Windows has Windows Installer, and the .msi goes straight to it, the way Noctorium's own updater hands
//! it over: with a progress bar and nothing to click, into the folder Noctorium is already in.

use crate::archive;
use crate::github::{Problem, Wanted};
use crate::system::{Os, PackageManager};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Flathub, where the runtime every Flatpak bundle is built against comes from.
pub const FLATHUB: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";
/// The Flatpak's application ID, which is also how it is started.
pub const FLATPAK_ID: &str = "app.noctorium.Noctorium";

/// The mark, as a PNG, for the menu entry an AppImage gets. The window's copy is decoded pixels; this is
/// the file itself, because a desktop reads PNGs and not raw RGBA.
const ICON: &[u8] = include_bytes!("../assets/noctorium-mark-128.png");

/// The application inside the Mac's disk image, and its name once installed.
pub const MAC_APP: &str = "Noctorium.app";

/// What marks the line this installer adds to a shell profile on a Mac, so that it is added once, and so
/// that whoever reads the file later knows where it came from.
pub const PROFILE_MARK: &str = "# Added by the Noctorium installer";
/// The line itself. `$HOME` rather than the path it stands for, so the profile still reads right if the
/// home folder is ever renamed or the file is copied to another Mac.
pub const PROFILE_LINE: &str = r#"export PATH="$HOME/.local/bin:$PATH""#;

/// The name Noctorium is listed under in Windows' Apps & features, which is how the folder it is in is
/// found again.
pub const WINDOWS_DISPLAY_NAME: &str = "Noctorium";

/// Noctorium Stats, as it is named everywhere a person sees it: the Start menu, Apps & features, the
/// folder it is unpacked into on Windows.
pub const STATS_NAME: &str = "Noctorium Stats";
/// Its program on Windows, which is also the name Windows lists it under while it runs.
pub const STATS_WINDOWS_PROGRAM: &str = "Noctorium Stats.exe";
/// Its program on Linux, and the command ~/.local/bin is given for it.
pub const STATS_COMMAND: &str = "noctorium-stats";
/// The application in the Mac's zip, and its name once installed.
pub const STATS_MAC_APP: &str = "Noctorium Stats.app";
/// Where Windows keeps Noctorium Stats' entry in Apps & features: under this user's own list, since it is
/// installed for this user alone and needs no administrator to put it there or take it away.
pub const STATS_UNINSTALL_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\NoctoriumStats";

/// How one file gets installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Windows: the .msi, handed to msiexec.
    WindowsMsi,
    /// Windows, for a release with no .msi: run the setup .exe and let it do its own asking.
    WindowsSetup,
    /// macOS: Noctorium.app copied out of the disk image into /Applications, or into ~/Applications for
    /// an account that cannot write there.
    MacDiskImage,
    /// Debian and its relatives: apt, so dependencies are resolved rather than merely reported.
    Apt,
    /// Fedora and its relatives.
    Dnf,
    /// openSUSE.
    Zypper,
    /// Arch and its relatives.
    Pacman,
    /// One file in ~/Applications, and a menu entry pointing at it.
    AppImage,
    /// A bundle handed to Flatpak, installed for this user only.
    Flatpak,
    /// The terminal player, unpacked into the user's own programs folder and put on their PATH.
    CliWindows,
    /// The terminal player, unpacked under ~/.local/share and linked from ~/.local/bin.
    CliLinux,
    /// The terminal player on a Mac: as on Linux, and then ~/.local/bin put on the PATH in ~/.zprofile
    /// when it is not there already, which on a Mac it never is to begin with.
    CliMac,
    /// Noctorium Stats on Windows: unpacked into the user's own programs folder beside the Noctorium CLI,
    /// given a Start menu shortcut, and listed in Apps & features to be uninstalled from there.
    StatsWindows,
    /// Noctorium Stats on Linux: unpacked under ~/.local/share, linked from ~/.local/bin, and given the
    /// menu entry and icon it comes with.
    StatsLinux,
    /// Noctorium Stats on a Mac: Noctorium Stats.app copied out of its zip into Applications, beside
    /// Noctorium.
    StatsMac,
}

impl Method {
    /// The native package for a package manager.
    pub fn for_package_manager(manager: PackageManager) -> Method {
        match manager {
            PackageManager::Apt => Method::Apt,
            PackageManager::Dnf => Method::Dnf,
            PackageManager::Zypper => Method::Zypper,
            PackageManager::Pacman => Method::Pacman,
        }
    }

    /// How the terminal player is installed on [os].
    pub fn cli_for(os: Os) -> Method {
        match os {
            Os::Windows => Method::CliWindows,
            Os::MacOs => Method::CliMac,
            Os::Linux | Os::Other(_) => Method::CliLinux,
        }
    }

    /// How Noctorium Stats is installed on [os].
    pub fn stats_for(os: Os) -> Method {
        match os {
            Os::Windows => Method::StatsWindows,
            Os::MacOs => Method::StatsMac,
            Os::Linux | Os::Other(_) => Method::StatsLinux,
        }
    }

    /// The release file this takes.
    pub fn wanted(self) -> Wanted {
        match self {
            Method::WindowsMsi => Wanted::WindowsMsi,
            Method::WindowsSetup => Wanted::WindowsSetup,
            Method::MacDiskImage => Wanted::MacDiskImage,
            Method::Apt => Wanted::DebianPackage,
            Method::Dnf | Method::Zypper => Wanted::RpmPackage,
            Method::Pacman => Wanted::ArchPackage,
            Method::AppImage => Wanted::AppImage,
            Method::Flatpak => Wanted::Flatpak,
            Method::CliWindows => Wanted::CliWindows,
            Method::CliLinux => Wanted::CliLinux,
            Method::CliMac => Wanted::CliMac,
            Method::StatsWindows => Wanted::StatsWindows,
            Method::StatsLinux => Wanted::StatsLinux,
            Method::StatsMac => Wanted::StatsMac,
        }
    }

    /// Whether it writes where only root can. Everything else is installed for this user alone -- or, on
    /// a Mac, into the /Applications that an administrator can write to without asking for anything.
    pub fn needs_root(self) -> bool {
        matches!(
            self,
            Method::Apt | Method::Dnf | Method::Zypper | Method::Pacman
        )
    }

    /// The format, as a menu shows it.
    pub fn describe(self) -> &'static str {
        match self {
            Method::WindowsMsi => "Windows Installer package (.msi)",
            Method::WindowsSetup => "Windows installer (.exe)",
            Method::MacDiskImage => "macOS disk image (.dmg)",
            Method::Apt => "Debian package (.deb)",
            Method::Dnf | Method::Zypper => "RPM package (.rpm)",
            Method::Pacman => "Arch Linux package (.pkg.tar.zst)",
            Method::AppImage => "AppImage",
            Method::Flatpak => "Flatpak",
            Method::CliWindows => "folder of its own, on your PATH",
            Method::CliLinux | Method::CliMac => "folder of its own, linked into ~/.local/bin",
            Method::StatsWindows => "folder of its own, in the Start menu",
            Method::StatsLinux => "folder of its own, in the applications menu",
            Method::StatsMac => "Noctorium Stats.app, in Applications",
        }
    }

    /// What it means in practice, in a few words, for the line beside it in a menu.
    pub fn explain(self) -> &'static str {
        match self {
            Method::WindowsMsi => "Windows Installer, with a progress bar and nothing to click",
            Method::WindowsSetup => "the usual installer, which asks its own questions",
            Method::MacDiskImage => "Noctorium.app, copied into Applications as if dragged there",
            Method::Apt => "through apt, which fetches what it needs",
            Method::Dnf => "through dnf, which fetches what it needs",
            Method::Zypper => "through zypper, which fetches what it needs",
            Method::Pacman => "through pacman, with mpv as a dependency",
            Method::AppImage => "one file in ~/Applications, no password, uses this machine's mpv",
            Method::Flatpak => "sandboxed, for you alone, with its own mpv",
            Method::CliWindows
            | Method::CliLinux
            | Method::CliMac
            | Method::StatsWindows
            | Method::StatsLinux => "for you alone, no administrator needed",
            Method::StatsMac => "Noctorium Stats.app, copied into Applications as if dragged there",
        }
    }
}

/// Where the per-user formats go, worked out once from the environment.
///
/// Each is optional because a machine can lack the variable it comes from -- a service account with no
/// HOME -- and that only matters if somebody asks for a format that needs it.
#[derive(Debug, Clone, Default)]
pub struct Places {
    pub home: Option<PathBuf>,
    /// `$XDG_DATA_HOME`, or `~/.local/share`.
    pub data: Option<PathBuf>,
    /// `%LOCALAPPDATA%\Programs`, where per-user Windows programs live.
    pub programs: Option<PathBuf>,
    /// `%APPDATA%\Microsoft\Windows\Start Menu\Programs`, this user's own Start menu.
    pub start_menu: Option<PathBuf>,
    /// The folder a Mac application goes in: `/Applications` when this user can write to it, and
    /// `~/Applications` when not. Nothing anywhere else.
    pub applications: Option<PathBuf>,
    /// The folder Noctorium is installed in already, on Windows, as the entry Apps & features lists it
    /// under says; nothing when it is not installed, and nothing anywhere else.
    ///
    /// The .msi does not remember a folder chosen at the first install. Given nothing, it would put the
    /// new version in Program Files and leave whoever chose somewhere else wondering where it went, so
    /// this is passed to it, as Noctorium's own updater passes it.
    pub installed: Option<PathBuf>,
    /// Whether the user's PATH -- and on a Mac the shell profile that sets it -- is to be left as it is:
    /// `NOCTORIUM_NO_PATH` in the environment.
    ///
    /// For trying the install out somewhere it can do no harm: with LOCALAPPDATA or HOME pointed at a
    /// folder of its own, every file the Noctorium CLI is unpacked to lands in there, but the PATH is the
    /// account's own and would still be changed. With this set the CLI is unpacked and nothing else.
    /// Windows' list of installed programs is in the registry too, where no variable can point elsewhere,
    /// so with this set Noctorium Stats is not entered in it either: unpacked, and given its shortcut in
    /// whatever Start menu APPDATA says, and nothing more.
    pub leave_path: bool,
}

impl Places {
    pub fn here() -> Places {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from);
        // The specification says a relative XDG_DATA_HOME is to be ignored, and it is.
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|d| d.is_absolute())
            .or_else(|| home.as_ref().map(|h| h.join(".local").join("share")));
        let programs = std::env::var_os("LOCALAPPDATA")
            .filter(|l| !l.is_empty())
            .map(|l| PathBuf::from(l).join("Programs"));
        // Read from the variable rather than asked of the shell, like everything else here, so that a
        // trial install with APPDATA pointed elsewhere puts its shortcut elsewhere too.
        let start_menu = std::env::var_os("APPDATA")
            .filter(|a| !a.is_empty() && cfg!(windows))
            .map(|a| {
                PathBuf::from(a)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Start Menu")
                    .join("Programs")
            });
        // /Applications is where anybody looks for an application, and every administrator on a Mac can
        // write to it without being asked for a password. A standard account cannot, and for that macOS
        // keeps ~/Applications, which Spotlight finds just the same and nobody else can touch. Decided
        // here, before anything happens, so the plan can say which.
        let applications = if cfg!(target_os = "macos") {
            let shared = PathBuf::from("/Applications");
            if crate::system::writable(&shared) {
                Some(shared)
            } else {
                home.as_ref().map(|h| h.join("Applications"))
            }
        } else {
            None
        };
        Places {
            home,
            data,
            programs,
            start_menu,
            applications,
            installed: crate::system::installed_folder(WINDOWS_DISPLAY_NAME),
            leave_path: std::env::var_os("NOCTORIUM_NO_PATH").is_some_and(|v| !v.is_empty()),
        }
    }

    fn need(&self, what: &Option<PathBuf>, name: &str) -> Result<PathBuf, Problem> {
        what.clone().ok_or_else(|| {
            Problem::Local(format!(
                "{name} is not set, so there is nowhere in your own folders to install this. Choose a \
                 package instead, or set {name}."
            ))
        })
    }

    pub fn appimage(&self) -> Result<PathBuf, Problem> {
        Ok(self
            .need(&self.home, "HOME")?
            .join("Applications")
            .join("Noctorium.AppImage"))
    }

    pub fn menu_entry(&self) -> Result<PathBuf, Problem> {
        Ok(self
            .need(&self.data, "HOME")?
            .join("applications")
            .join("noctorium.desktop"))
    }

    pub fn icon(&self) -> Result<PathBuf, Problem> {
        Ok(self
            .need(&self.data, "HOME")?
            .join("icons/hicolor/128x128/apps/noctorium.png"))
    }

    /// The folder Noctorium.app goes in on a Mac.
    pub fn mac_applications(&self) -> Result<PathBuf, Problem> {
        self.need(&self.applications, "HOME")
    }

    /// The login profile of zsh, which has been every Mac's shell since Catalina. Every Terminal window is
    /// a login shell and reads it, after /etc/zprofile has had path_helper build the system's PATH -- so a
    /// line here puts ~/.local/bin first, where in ~/.zshenv path_helper would push it behind the system's
    /// own folders.
    pub fn shell_profile(&self) -> Result<PathBuf, Problem> {
        Ok(self.need(&self.home, "HOME")?.join(".zprofile"))
    }

    /// Where the Noctorium CLI is unpacked to.
    ///
    /// On a Mac the same folder as on Linux, rather than in ~/Library/Application Support. Noctorium keeps
    /// its own data under ~/.local/share on a Mac too, so the CLI's program sits beside it; the link to it
    /// is in ~/.local/bin, beside both; and a path with no space in it is one that a link, a PATH and a
    /// shell script can all be trusted to read whole. It is also one sentence in the README, not two.
    pub fn cli_folder(&self, windows: bool) -> Result<PathBuf, Problem> {
        if windows {
            Ok(self
                .need(&self.programs, "LOCALAPPDATA")?
                .join("Noctorium CLI"))
        } else {
            Ok(self.need(&self.data, "HOME")?.join("noctorium-cli"))
        }
    }

    /// `~/.local/bin`, which systemd's file hierarchy and most distributions' default profiles put on
    /// PATH for every user. macOS does not, which is what [Places::shell_profile] is for.
    pub fn user_bin(&self) -> Result<PathBuf, Problem> {
        Ok(self.need(&self.home, "HOME")?.join(".local").join("bin"))
    }

    /// Where Noctorium Stats is unpacked to: beside the Noctorium CLI, named as each system names its
    /// programs' folders.
    pub fn stats_folder(&self, windows: bool) -> Result<PathBuf, Problem> {
        if windows {
            Ok(self.need(&self.programs, "LOCALAPPDATA")?.join(STATS_NAME))
        } else {
            Ok(self.need(&self.data, "HOME")?.join(STATS_COMMAND))
        }
    }

    /// Noctorium Stats' shortcut, at the top of this user's Start menu rather than in a folder of its
    /// own: one program needs no folder to find it in, and Windows 11 shows the top level first.
    pub fn stats_shortcut(&self) -> Result<PathBuf, Problem> {
        Ok(self
            .need(&self.start_menu, "APPDATA")?
            .join(format!("{STATS_NAME}.lnk")))
    }

    /// Noctorium Stats' menu entry, beside the AppImage's.
    pub fn stats_menu_entry(&self) -> Result<PathBuf, Problem> {
        Ok(self
            .need(&self.data, "HOME")?
            .join("applications")
            .join(format!("{STATS_COMMAND}.desktop")))
    }

    /// The icon theme its icon goes into, in the folder for the icon's own size.
    pub fn icon_theme(&self) -> Result<PathBuf, Problem> {
        Ok(self.need(&self.data, "HOME")?.join("icons").join("hicolor"))
    }

    /// A path as somebody would type it: `~/Applications/...` rather than the whole of /home.
    pub fn show(&self, path: &Path) -> String {
        if !cfg!(windows) {
            if let Some(rest) = self.home.as_ref().and_then(|h| path.strip_prefix(h).ok()) {
                return format!("~/{}", rest.display());
            }
        }
        path.display().to_string()
    }
}

/// One thing done to install a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A program, run and waited for. The command is the whole of what it does, so it can be shown.
    Run { program: String, args: Vec<String> },
    /// The .msi handed to msiexec, into [folder] when Noctorium is installed there already, and with a
    /// progress bar and nothing to click unless [wizard] asks for the setup's own pages -- which is where
    /// somebody who wants to choose the folder chooses it.
    InstallMsi {
        msi: PathBuf,
        folder: Option<PathBuf>,
        wizard: bool,
    },
    /// An AppImage copied into place, made executable and given a menu entry.
    PlaceAppImage {
        from: PathBuf,
        to: PathBuf,
        entry: PathBuf,
        icon: PathBuf,
    },
    /// The Noctorium CLI unpacked, and then linked from [link] on Linux and a Mac, or -- when [path] says
    /// so -- its folder put on the user's PATH on Windows.
    UnpackCli {
        archive: PathBuf,
        into: PathBuf,
        link: Option<PathBuf>,
        path: bool,
    },
    /// The Flatpak taken out if it is installed already, keeping its data, so a bundle can go in.
    ClearFlatpak { id: String },
    /// Noctorium.app copied out of a disk image into [into], in place of any copy already there.
    PlaceApp { image: PathBuf, into: PathBuf },
    /// The line that puts ~/.local/bin on PATH, added once to the end of a shell profile.
    AddToProfile { profile: PathBuf },
    /// Noctorium Stats unpacked into [into], in place of any older copy, and on Linux linked from [link].
    UnpackStats {
        archive: PathBuf,
        into: PathBuf,
        link: Option<PathBuf>,
    },
    /// A Start menu shortcut at [shortcut] to the program unpacked in [folder].
    StartMenuShortcut { folder: PathBuf, shortcut: PathBuf },
    /// Noctorium Stats entered in Apps & features as [version], where it can be uninstalled the way any
    /// Windows program is: its folder, its shortcut and the entry itself taken away.
    ListInstalled {
        folder: PathBuf,
        shortcut: PathBuf,
        version: String,
    },
    /// The menu entry and icon unpacked in [folder] put where the desktop looks for them -- [entry], and
    /// the theme at [icons] -- pointing at the program beside them.
    MenuEntry {
        folder: PathBuf,
        entry: PathBuf,
        icons: PathBuf,
    },
    /// The application [app] copied out of the zip [archive] into [into], in place of any copy already
    /// there.
    PlaceZippedApp {
        archive: PathBuf,
        into: PathBuf,
        app: String,
    },
}

/// What installing [file] with [method] takes, as steps that can be shown before any of them is run.
///
/// Built rather than run, so the caller can print it before anything happens -- an installer that asks
/// for a password should have said what it is about to do first -- and so it can be tested without
/// installing anything. [wizard] is for the .msi alone, and asks for its own pages rather than a progress
/// bar; the setup .exe has nothing else to offer.
pub fn actions_for(
    method: Method,
    file: &Path,
    escalate_with: Option<&str>,
    places: &Places,
    wizard: bool,
) -> Result<Vec<Action>, Problem> {
    let path = file.to_string_lossy().to_string();
    let actions = match method {
        Method::WindowsMsi => vec![Action::InstallMsi {
            msi: file.to_path_buf(),
            folder: places.installed.clone(),
            wizard,
        }],
        Method::WindowsSetup => vec![Action::Run {
            program: path,
            args: vec![],
        }],
        // `apt-get install ./file.deb` rather than `dpkg -i`: dpkg reports missing dependencies and
        // stops, apt fetches them. The leading ./ is what tells apt this is a file and not a name.
        Method::Apt => vec![escalated(
            "apt-get",
            &["install", "-y", &local_path(&path)],
            escalate_with,
        )],
        Method::Dnf => vec![escalated("dnf", &["install", "-y", &path], escalate_with)],
        // The package is not signed with a key zypper knows -- it is checked against the release's
        // checksum instead, which has already happened by the time this runs -- and without the flag
        // zypper stops to ask about it even with --non-interactive.
        Method::Zypper => vec![escalated(
            "zypper",
            &[
                "--non-interactive",
                "install",
                "--allow-unsigned-rpm",
                &path,
            ],
            escalate_with,
        )],
        Method::Pacman => vec![escalated(
            "pacman",
            &["-U", "--noconfirm", &path],
            escalate_with,
        )],
        Method::AppImage => vec![Action::PlaceAppImage {
            from: file.to_path_buf(),
            to: places.appimage()?,
            entry: places.menu_entry()?,
            icon: places.icon()?,
        }],
        // A bundle carries the application but not the runtime it was built against, and Flatpak fetches
        // that from whichever remote has it -- so Flathub is added first if it is not there already,
        // for this user only, which needs no password.
        //
        // Then any copy already installed is taken out. Flatpak will not install a bundle over an
        // application it already has -- "already installed", and --reinstall does not apply to bundles
        // -- so replacing an older one means removing it first. Its data in ~/.var/app stays; only
        // --delete-data would touch that.
        Method::Flatpak => vec![
            Action::Run {
                program: "flatpak".into(),
                args: [
                    "remote-add",
                    "--user",
                    "--if-not-exists",
                    "flathub",
                    FLATHUB,
                ]
                .map(String::from)
                .to_vec(),
            },
            Action::ClearFlatpak {
                id: FLATPAK_ID.into(),
            },
            Action::Run {
                program: "flatpak".into(),
                args: ["install", "--user", "-y", "--bundle", &path]
                    .map(String::from)
                    .to_vec(),
            },
        ],
        Method::CliWindows => vec![Action::UnpackCli {
            archive: file.to_path_buf(),
            into: places.cli_folder(true)?,
            link: None,
            path: !places.leave_path,
        }],
        Method::CliLinux | Method::CliMac => vec![Action::UnpackCli {
            archive: file.to_path_buf(),
            into: places.cli_folder(false)?,
            link: Some(places.user_bin()?.join("noctorium")),
            path: false,
        }],
        Method::MacDiskImage => vec![Action::PlaceApp {
            image: file.to_path_buf(),
            into: places.mac_applications()?,
        }],
        // Entered in Apps & features by the plan, which knows the version, rather than here.
        Method::StatsWindows => {
            let folder = places.stats_folder(true)?;
            vec![
                Action::UnpackStats {
                    archive: file.to_path_buf(),
                    into: folder.clone(),
                    link: None,
                },
                Action::StartMenuShortcut {
                    folder,
                    shortcut: places.stats_shortcut()?,
                },
            ]
        }
        // On PATH as well as in the menu: it is a window, but one somebody may well start from the
        // terminal they were already in, as they would the Noctorium CLI beside it.
        Method::StatsLinux => {
            let folder = places.stats_folder(false)?;
            vec![
                Action::UnpackStats {
                    archive: file.to_path_buf(),
                    into: folder.clone(),
                    link: Some(places.user_bin()?.join(STATS_COMMAND)),
                },
                Action::MenuEntry {
                    folder,
                    entry: places.stats_menu_entry()?,
                    icons: places.icon_theme()?,
                },
            ]
        }
        Method::StatsMac => vec![Action::PlaceZippedApp {
            archive: file.to_path_buf(),
            into: places.mac_applications()?,
            app: STATS_MAC_APP.into(),
        }],
    };
    Ok(actions)
}

/// The step that enters Noctorium Stats in Apps & features as [version], for a plan to add on Windows --
/// unless NOCTORIUM_NO_PATH asks for the account to be left as it is, which the registry is part of.
///
/// Separate from [actions_for] because the version is the release's, which the plan has and the rest of
/// the actions do not need.
pub fn list_installed(places: &Places, version: &str) -> Result<Option<Action>, Problem> {
    if places.leave_path {
        return Ok(None);
    }
    Ok(Some(Action::ListInstalled {
        folder: places.stats_folder(true)?,
        shortcut: places.stats_shortcut()?,
        version: version.to_string(),
    }))
}

/// The step that puts ~/.local/bin on PATH on a Mac, for a plan to add when it is not there already.
///
/// Separate from [actions_for] because whether it is needed depends on the PATH this program was started
/// with, which is the caller's to read, and because a step that would change nothing should not be shown
/// as if it were about to.
pub fn add_user_bin_to_path(places: &Places) -> Result<Action, Problem> {
    Ok(Action::AddToProfile {
        profile: places.shell_profile()?,
    })
}

/// A path apt will read as a file. Without a directory in front of it, `noctorium.deb` is a package name.
fn local_path(path: &str) -> String {
    if path.starts_with('/') || path.starts_with("./") {
        path.to_string()
    } else {
        format!("./{path}")
    }
}

fn escalated(program: &str, args: &[&str], escalate_with: Option<&str>) -> Action {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    match escalate_with {
        Some(sudo) => {
            let mut all = vec![program.to_string()];
            all.extend(args);
            Action::Run {
                program: sudo.to_string(),
                args: all,
            }
        }
        None => Action::Run {
            program: program.to_string(),
            args,
        },
    }
}

impl Action {
    /// The step as somebody reads it: the exact command for a program, a sentence for the rest.
    pub fn describe(&self, places: &Places) -> String {
        match self {
            Action::Run { program, args } => command_line(program, args),
            // msiexec by its name rather than by the whole of %SystemRoot%\System32, which is where it is
            // run from and which nobody needs telling.
            Action::InstallMsi {
                msi,
                folder,
                wizard,
            } => format!(
                "msiexec {}",
                msiexec_arguments(msi, folder.as_deref(), *wizard)
            ),
            Action::PlaceAppImage { to, entry, .. } => format!(
                "copy it to {}, make it executable, and add it to the menu as {}",
                places.show(to),
                places.show(entry)
            ),
            Action::UnpackCli {
                into,
                link: Some(link),
                ..
            } => format!(
                "unpack it into {} and link {} to it",
                places.show(into),
                places.show(link)
            ),
            Action::UnpackCli {
                into,
                link: None,
                path: true,
                ..
            } => format!(
                "unpack it into {} and add that folder to your PATH",
                places.show(into)
            ),
            Action::UnpackCli {
                into,
                link: None,
                path: false,
                ..
            } => format!(
                "unpack it into {}, and leave your PATH as it is (NOCTORIUM_NO_PATH is set)",
                places.show(into)
            ),
            Action::ClearFlatpak { id } => format!(
                "flatpak uninstall --user -y {id}, if an older one is installed (its settings stay)"
            ),
            Action::PlaceApp { into, .. } => format!(
                "copy {MAC_APP} from the disk image into {}, replacing any older copy there",
                places.show(into)
            ),
            Action::AddToProfile { profile } => format!(
                "put ~/.local/bin on your PATH, with a line at the end of {}",
                places.show(profile)
            ),
            Action::UnpackStats {
                into,
                link: Some(link),
                ..
            } => format!(
                "unpack it into {} and link {} to it",
                places.show(into),
                places.show(link)
            ),
            Action::UnpackStats {
                into, link: None, ..
            } => format!("unpack it into {}", places.show(into)),
            Action::StartMenuShortcut { shortcut, .. } => {
                format!("add it to the Start menu, as {}", places.show(shortcut))
            }
            Action::ListInstalled { .. } => format!(
                "list it in Windows' installed apps, where it can be uninstalled (HKCU\\{STATS_UNINSTALL_KEY})"
            ),
            Action::MenuEntry { entry, icons, .. } => format!(
                "add it to the applications menu as {}, with its icon in {}",
                places.show(entry),
                places.show(icons)
            ),
            Action::PlaceZippedApp { into, app, .. } => format!(
                "copy {app} out of the archive into {}, replacing any older copy there",
                places.show(into)
            ),
        }
    }

    /// Does it, and says anything worth saying about how it went -- which so far is only Windows asking
    /// to be restarted.
    pub fn perform(&self) -> Result<Option<String>, Problem> {
        let done = |result: Result<(), Problem>| result.map(|()| None);
        match self {
            Action::Run { program, args } => done(run(program, args)),
            Action::InstallMsi {
                msi,
                folder,
                wizard,
            } => {
                // Asked again, because the download took long enough for somebody to have opened it.
                refuse_if_open_on_windows()?;
                run_msiexec(&msiexec_arguments(msi, folder.as_deref(), *wizard))
            }
            Action::PlaceAppImage {
                from,
                to,
                entry,
                icon,
            } => done(place_appimage(from, to, entry, icon)),
            Action::UnpackCli {
                archive,
                into,
                link,
                path,
            } => done(install_cli(archive, into, link.as_deref(), *path)),
            Action::ClearFlatpak { id } => done(clear_flatpak(id)),
            Action::PlaceApp { image, into } => done(place_app(image, into)),
            Action::AddToProfile { profile } => done(add_to_profile(profile)),
            Action::UnpackStats {
                archive,
                into,
                link,
            } => done(install_stats(archive, into, link.as_deref())),
            Action::StartMenuShortcut { folder, shortcut } => {
                done(start_menu_shortcut(folder, shortcut))
            }
            Action::ListInstalled {
                folder,
                shortcut,
                version,
            } => done(list_in_installed_apps(folder, shortcut, version)),
            Action::MenuEntry {
                folder,
                entry,
                icons,
            } => done(menu_entry(folder, entry, icons)),
            Action::PlaceZippedApp { archive, into, app } => {
                done(place_zipped_app(archive, into, app))
            }
        }
    }
}

/// Anything that would make installing with [method] fail, found out before the download rather than
/// after it.
///
/// On a Mac, and for the .msi on Windows, that is Noctorium being open. A running application reads its
/// own files as it goes -- a Java one more than most, loading classes from its jars as they are first
/// wanted -- and swapping them out from under it is how it crashes; finding that out after three hundred
/// megabytes have come down would waste them. Windows will not replace a file that is open at all, and
/// with nothing to click, there is nobody to ask whether to close it.
///
/// Noctorium Stats is one native program, which a Mac and Linux replace from under itself without
/// noticing -- the running copy keeps the file it opened -- but which Windows will not let anything move
/// while it runs, so it is asked about there and nowhere else.
pub fn check_before_download(method: Method) -> Result<(), Problem> {
    match method {
        Method::MacDiskImage => refuse_if_running(),
        Method::WindowsMsi => refuse_if_open_on_windows(),
        Method::StatsWindows => refuse_if_stats_is_open(),
        _ => Ok(()),
    }
}

fn refuse_if_stats_is_open() -> Result<(), Problem> {
    if crate::system::running(STATS_WINDOWS_PROGRAM) {
        return Err(Problem::Local(
            "Noctorium Stats is open. Close it and run this again: Windows cannot replace a program \
             while it is running. Nothing has been changed."
                .into(),
        ));
    }
    Ok(())
}

/// The program Noctorium is started as on Windows, which is jpackage's launcher in its install folder.
/// Capitalised, as it is on disk, which is what tells it from the Noctorium CLI's `noctorium.exe`.
const WINDOWS_PROGRAM: &str = "Noctorium.exe";

fn refuse_if_open_on_windows() -> Result<(), Problem> {
    if crate::system::running(WINDOWS_PROGRAM) {
        return Err(Problem::Local(
            "Noctorium is open. Quit it -- from its menu, or from its icon by the clock if it is still \
             there -- and run this again: Windows cannot replace a program while it is running. Nothing \
             has been changed."
                .into(),
        ));
    }
    Ok(())
}

/// What msiexec is given, as one line, with the same arguments Noctorium's own updater gives it:
/// `/i "<msi>" /passive /norestart MSIFASTINSTALL=7`, and `INSTALLDIR="<folder>"` when Noctorium is
/// installed already. `/passive` is a progress bar and nothing to click, since saying yes here was the
/// decision and the permission prompt Windows puts up is still there to refuse at; `/norestart` leaves a
/// restart, should one be wanted, for later; `MSIFASTINSTALL=7` skips the System Restore point and all of
/// the costing but the files', which is most of an msi's own overhead. The wizard is the same without
/// `/passive`, and starts on the folder Noctorium is already in.
///
/// One line rather than a list of arguments, because msiexec reads its own command line and not the way a
/// list is quoted for it: Rust, like every C runtime, would quote `INSTALLDIR=C:\Program Files\Noctorium`
/// whole, and msiexec wants the quotes around the value. No Windows path can contain a double quote, so a
/// pair of them is all the quoting a path needs -- spaces, apostrophes and accents included.
///
/// The folder loses the backslash it ends with, which is how Windows Installer writes InstallLocation:
/// before a closing quote it is where command-line parsers disagree with each other, and Windows Installer
/// puts it back on a folder itself. A drive's own root keeps it, since `C:` alone is a different place.
pub fn msiexec_arguments(msi: &Path, folder: Option<&Path>, wizard: bool) -> String {
    let mut line = format!("/i \"{}\"", msi.to_string_lossy());
    if !wizard {
        line.push_str(" /passive");
    }
    line.push_str(" /norestart MSIFASTINSTALL=7");
    if let Some(folder) = folder {
        let folder = folder.to_string_lossy();
        let mut trimmed = folder.trim_end_matches(['\\', '/']);
        if trimmed.len() == 2 && trimmed.ends_with(':') {
            trimmed = &folder[..3.min(folder.len())];
        }
        line.push_str(&format!(" INSTALLDIR=\"{trimmed}\""));
    }
    line
}

/// msiexec, from System32 rather than from whatever PATH says: an msiexec.exe somewhere else on it is not
/// one to hand three hundred megabytes and administrator rights to.
fn msiexec() -> PathBuf {
    let windows = std::env::var_os("SystemRoot")
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    windows.join("System32").join("msiexec.exe")
}

/// What Windows Installer's exit status means, for somebody: done, done with a restart wanted, or why it
/// was not done.
pub fn msiexec_outcome(code: Option<i32>) -> Result<Option<String>, Problem> {
    match code {
        Some(0) => Ok(None),
        Some(3010) => Ok(Some(
            "Windows says it needs restarting to finish installing it. Noctorium can be started \
             before then."
                .into(),
        )),
        Some(1602) => Err(Problem::Local(
            "The install was cancelled, and nothing was changed.".into(),
        )),
        Some(1618) => Err(Problem::Local(
            "Windows is installing something else already. Let that finish, then try again.".into(),
        )),
        Some(code) => Err(Problem::Local(format!(
            "msiexec finished with exit code {code}. Nothing was installed, or the install was \
             cancelled."
        ))),
        None => Err(Problem::Local(
            "msiexec ended without an exit code. Nothing was installed, or the install was cancelled."
                .into(),
        )),
    }
}

#[cfg(windows)]
fn run_msiexec(arguments: &str) -> Result<Option<String>, Problem> {
    let program = msiexec();
    let status = verbatim(&program, arguments)
        .status()
        .map_err(|e| Problem::Local(format!("Could not start {}: {e}", program.display())))?;
    msiexec_outcome(status.code())
}

/// [program], to be given [arguments] exactly as they are written, with nothing quoted or escaped.
#[cfg(windows)]
fn verbatim(program: &Path, arguments: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut command = Command::new(program);
    command.raw_arg(arguments);
    command
}

#[cfg(not(windows))]
fn run_msiexec(_: &str) -> Result<Option<String>, Problem> {
    Err(Problem::Local(format!(
        "{} is Windows' own, and this is not Windows.",
        msiexec().display()
    )))
}

/// What `pgrep -f` looks for: the program inside any Noctorium.app, wherever it was started from -- the
/// one in /Applications, one in ~/Applications, or one still on the disk image.
const RUNNING_APP: &str = "Noctorium.app/Contents/MacOS";

fn refuse_if_running() -> Result<(), Problem> {
    // pgrep says 0 when it found something and 1 when it did not. Anything else, or no pgrep, is not
    // reason enough to refuse an install.
    let running = Command::new("pgrep")
        .args(["-f", RUNNING_APP])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if running {
        return Err(Problem::Local(
            "Noctorium is open. Quit it -- Command-Q, or Quit Noctorium from its menu -- and run this \
             again: an application cannot safely be replaced while it is running. Nothing has been \
             changed."
                .into(),
        ));
    }
    Ok(())
}

/// Copies Noctorium.app out of the disk image at [image] into the folder [into].
///
/// The image is attached read-only at a folder of this program's own and detached again on every way out
/// of here, failures included. The quarantine flag is taken off the copy, so its first start is not
/// stopped to ask about something downloaded from the internet: the download was checked against the
/// release's own checksum before it got this far, which is a better answer to that question than a
/// dialog.
fn place_app(image: &Path, into: &Path) -> Result<(), Problem> {
    // Asked again, because the download took long enough for somebody to have opened it meanwhile.
    refuse_if_running()?;
    let mounted = Mounted::attach(image)?;
    let source = app_in(&mounted.point, MAC_APP).ok_or_else(|| {
        Problem::Local(format!(
            "The disk image {} has no {MAC_APP} in it. It is not the shape this installer expects; the \
             releases page has it to open by hand.",
            file_name(image)
        ))
    })?;
    let installed = replace_app(&source, into, MAC_APP, &ditto)?;
    clear_quarantine(&installed);
    Ok(())
}

/// Copies the application [app] out of the zip at [archive] into the folder [into]: Noctorium Stats.app,
/// which is published as a zip rather than a disk image.
///
/// Unpacked by `ditto -x -k`, the Mac's own way of opening one -- what the Finder does on a double click
/// -- which keeps the bundle's links, permissions and extended attributes as they were made, and reads
/// the resource forks a Mac's own zips keep in a `__MACOSX` folder rather than leaving that folder about.
/// Then put in place as Noctorium.app is, and the quarantine flag taken off it, for the same reasons.
fn place_zipped_app(archive: &Path, into: &Path, app: &str) -> Result<(), Problem> {
    let installed = unzip_and_replace_app(archive, into, app, &ditto_unzip, &ditto)?;
    clear_quarantine(&installed);
    Ok(())
}

/// The part of [place_zipped_app] that can be tried anywhere: [unzip] unpacks the archive into a folder,
/// [copy] copies a bundle, and whatever was unpacked is cleared away again on every way out.
fn unzip_and_replace_app(
    archive: &Path,
    into: &Path,
    app: &str,
    unzip: &dyn Fn(&Path, &Path) -> Result<(), Problem>,
    copy: &dyn Fn(&Path, &Path) -> Result<(), Problem>,
) -> Result<PathBuf, Problem> {
    let unpacked = fresh_folder("noctorium-installer-zip")?;
    let outcome = unzip(archive, &unpacked).and_then(|()| {
        let source = app_in(&unpacked, app).ok_or_else(|| {
            Problem::Local(format!(
                "{} has no {app} in it. It is not the shape this installer expects; the releases page \
                 has it to open by hand.",
                file_name(archive)
            ))
        })?;
        replace_app(&source, into, app, copy)
    });
    let _ = fs::remove_dir_all(&unpacked);
    outcome
}

/// Unpacks a zip with `ditto`, as the Finder would.
fn ditto_unzip(archive: &Path, into: &Path) -> Result<(), Problem> {
    let output = Command::new("ditto")
        .args(["-x", "-k"])
        .arg(archive)
        .arg(into)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Problem::Local(format!("Could not start ditto to open the archive: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Problem::Local(format!(
            "ditto could not open {}: {}",
            file_name(archive),
            what_it_said(&output)
        )))
    }
}

/// A disk image attached at a folder of this program's own, and detached again when this is dropped.
struct Mounted {
    point: PathBuf,
}

impl Mounted {
    fn attach(image: &Path) -> Result<Mounted, Problem> {
        let point = fresh_folder("noctorium-installer-image")?;
        // -nobrowse keeps it off the desktop and out of the Finder's sidebar, -noautoopen stops a Finder
        // window opening onto it, and reading is all that is needed of it. A mount point of this
        // program's own, rather than one in /Volumes, cannot meet a Noctorium image somebody already has
        // open. Standard input is closed, so an image that wanted a licence agreed to would be refused
        // rather than wait for an answer nobody is going to type.
        let attached = Command::new("hdiutil")
            .args([
                "attach",
                "-nobrowse",
                "-readonly",
                "-noautoopen",
                "-mountpoint",
            ])
            .arg(&point)
            .arg(image)
            .stdin(Stdio::null())
            .output();
        let failure = match attached {
            Ok(output) if output.status.success() => return Ok(Mounted { point }),
            Ok(output) => format!(
                "hdiutil could not open the disk image {}: {}",
                file_name(image),
                what_it_said(&output)
            ),
            Err(e) => format!("Could not start hdiutil to open the disk image: {e}"),
        };
        let _ = fs::remove_dir(&point);
        Err(Problem::Local(failure))
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        let detach = |force: bool| {
            let mut command = Command::new("hdiutil");
            command.arg("detach");
            if force {
                command.arg("-force");
            }
            command
                .arg(&self.point)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        // A detach that fails is nearly always something still holding a file open on the image, briefly;
        // -force is what hdiutil itself suggests then. If even that fails the image stays attached, out
        // of sight, until the Mac restarts -- untidy, and nothing worse.
        if !detach(false) {
            detach(true);
        }
        let _ = fs::remove_dir(&self.point);
    }
}

/// A new, empty folder in the temporary folder, made here and nowhere else: `create_dir` fails on one
/// that already exists, which is what makes it fresh rather than something an earlier run left behind.
fn fresh_folder(label: &str) -> Result<PathBuf, Problem> {
    let base = std::env::temp_dir();
    let mut last = None;
    for attempt in 0..100 {
        let folder = base.join(format!("{label}-{}-{attempt}", std::process::id()));
        match fs::create_dir(&folder) {
            Ok(()) => return Ok(folder),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(couldnt("make", &folder, e)),
        }
    }
    Err(Problem::Local(format!(
        "Could not make a folder in {} to open the download in: {}",
        base.display(),
        last.map(|e| e.to_string()).unwrap_or_default()
    )))
}

/// The application at the top of a mounted disk image or an unpacked zip: [name] -- Noctorium.app, or
/// Noctorium Stats.app -- or failing that the only application there.
///
/// Looked for rather than assumed, like the CLI's launcher, so an image that names it differently still
/// installs -- as [name], whatever it was called in the image. The link to Applications beside it is a
/// link and not an application, and is passed over, and so is the `__MACOSX` folder a zip made on a Mac
/// may carry.
pub fn app_in(mount: &Path, name: &str) -> Option<PathBuf> {
    let named = mount.join(name);
    if named.is_dir() {
        return Some(named);
    }
    let apps: Vec<PathBuf> = fs::read_dir(mount)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        })
        .collect();
    match apps.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// Puts the application at [source] into [folder] as [name], in place of any copy already there, and
/// says where it went.
///
/// The new copy is made beside the old one and renamed into place, so there is never a moment without a
/// whole Noctorium.app: a copy that fails half way leaves the old one exactly as it was, and a rename that
/// fails puts it back. [copy] does the copying -- `ditto` on a Mac, plain Rust in the tests.
fn replace_app(
    source: &Path,
    folder: &Path,
    name: &str,
    copy: &dyn Fn(&Path, &Path) -> Result<(), Problem>,
) -> Result<PathBuf, Problem> {
    fs::create_dir_all(folder).map_err(|e| couldnt("make", folder, e))?;
    let target = folder.join(name);
    let partial = folder.join(format!("{name}.partial"));
    let previous = folder.join(format!("{name}.old"));
    archive::remove_if_there(&partial)?;
    archive::remove_if_there(&previous)?;

    if let Err(problem) = copy(source, &partial) {
        let _ = fs::remove_dir_all(&partial);
        return Err(problem);
    }

    // Asked of the link itself, so a Noctorium.app that is a link to somewhere else is moved aside as a
    // link rather than mistaken for nothing.
    let had_one = fs::symlink_metadata(&target).is_ok();
    if had_one {
        if let Err(e) = fs::rename(&target, &previous) {
            let _ = fs::remove_dir_all(&partial);
            // Refused outright is what macOS does to a terminal that has not been allowed to change other
            // developers' applications, since Ventura.
            let permission = if e.kind() == io::ErrorKind::PermissionDenied {
                " If macOS said this terminal was prevented from modifying apps, allow it under System \
                 Settings, Privacy & Security, App Management, and run this again."
            } else {
                ""
            };
            return Err(Problem::Local(format!(
                "Could not move the {name} already in {} aside to replace it: {e}. It is still there, \
                 untouched.{permission}",
                folder.display()
            )));
        }
    }
    if let Err(e) = fs::rename(&partial, &target) {
        if had_one {
            let _ = fs::rename(&previous, &target);
        }
        let _ = fs::remove_dir_all(&partial);
        return Err(couldnt(&format!("move the new {name} into"), folder, e));
    }

    // Best effort, like the CLI's. What is left is clutter, not a broken install.
    let _ = fs::remove_dir_all(&previous);
    Ok(target)
}

/// Copies an application bundle with `ditto`, the copier made for them: it keeps everything a bundle
/// carries -- its links, its extended attributes, its code signature -- exactly as it was.
fn ditto(from: &Path, to: &Path) -> Result<(), Problem> {
    let app = file_name(from);
    let output = Command::new("ditto")
        .arg(from)
        .arg(to)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Problem::Local(format!("Could not start ditto to copy {app}: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Problem::Local(format!(
            "ditto could not copy {app} into {}: {}",
            to.parent().unwrap_or(to).display(),
            what_it_said(&output)
        )))
    }
}

/// Takes the quarantine flag off [path] and everything in it.
///
/// Normally there is none to take off: macOS flags what a browser or a mail program saves, not what a
/// program like this downloads. But the flag travels with copies, and if it is there, from wherever, the
/// first start stops to ask whether something downloaded from the internet should be opened -- or, for
/// an application that is not notarised, refuses to open it at all. A failure is the usual answer when
/// there was no flag to take off, and changes nothing that matters, so it is not reported.
fn clear_quarantine(path: &Path) {
    let _ = Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// What a program that failed said for itself, or how it ended when it said nothing.
fn what_it_said(output: &Output) -> String {
    let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !said.is_empty() {
        return said;
    }
    output
        .status
        .code()
        .map(|c| format!("exit code {c}"))
        .unwrap_or_else(|| "no exit code".into())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn couldnt(what: &str, path: &Path, e: io::Error) -> Problem {
    Problem::Local(format!("Could not {what} {}: {e}", path.display()))
}

/// What to add to the end of a shell profile that reads [existing] so that it puts ~/.local/bin on PATH,
/// or nothing when it already does.
///
/// Nothing when the mark is there, which is this installer having been run before, and nothing when the
/// line itself is, which is somebody having written it by hand. Otherwise the mark and the line, after a
/// blank line to keep them apart from whatever is above, and after a line break first if the file's last
/// line was never ended -- or the mark would be glued onto it.
pub fn profile_addition(existing: &str) -> Option<String> {
    if existing
        .lines()
        .any(|line| line.trim() == PROFILE_MARK || line.trim() == PROFILE_LINE)
    {
        return None;
    }
    let mut addition = String::new();
    if !existing.is_empty() && !existing.ends_with('\n') {
        addition.push('\n');
    }
    if !existing.trim().is_empty() {
        addition.push('\n');
    }
    addition.push_str(PROFILE_MARK);
    addition.push('\n');
    addition.push_str(PROFILE_LINE);
    addition.push('\n');
    Some(addition)
}

/// Adds the line to [profile], making the file if there is none, unless it is there already.
///
/// Appended rather than rewritten, so a profile that is a link into somebody's dotfiles stays a link and
/// nothing else in it is touched. Read as bytes, so a profile with something in it that is not UTF-8 is
/// still read rather than refused.
fn add_to_profile(profile: &Path) -> Result<(), Problem> {
    let failed = |e: io::Error| {
        Problem::Local(format!(
            "The Noctorium CLI is installed, but {} could not be changed to put ~/.local/bin on your \
             PATH: {e}. Add {PROFILE_LINE} to it yourself, or run the CLI as ~/.local/bin/noctorium.",
            profile.display()
        ))
    };
    let existing = match fs::read(profile) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(failed(e)),
    };
    let Some(addition) = profile_addition(&existing) else {
        return Ok(());
    };
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(profile)
        .map_err(failed)?;
    file.write_all(addition.as_bytes()).map_err(failed)
}

/// Uninstalls the Flatpak for this user if it is there, and does nothing if it is not.
///
/// Asked first rather than uninstalled blind, because "not installed" comes back as a failure, and a
/// first install would otherwise begin with an error message about something that was never there.
fn clear_flatpak(id: &str) -> Result<(), Problem> {
    let installed = Command::new("flatpak")
        .args(["info", "--user", id])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !installed {
        return Ok(());
    }
    run(
        "flatpak",
        &["uninstall", "--user", "-y", id].map(String::from),
    )
}

/// A command line as a shell would need it typed: quoted where it has to be, and only there.
pub fn command_line(program: &str, args: &[String]) -> String {
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .map(quoted)
        .collect::<Vec<_>>()
        .join(" ")
}

fn quoted(word: &str) -> String {
    let plain = !word.is_empty()
        && word.chars().all(|c| {
            // A backslash and a tilde are ordinary in a Windows path -- PRODUC~1 is a short name, not
            // somebody's home -- and special to a Unix shell.
            c.is_ascii_alphanumeric()
                || "-_./:=@%+,".contains(c)
                || (cfg!(windows) && (c == '\\' || c == '~'))
        });
    if plain {
        word.to_string()
    } else if cfg!(windows) {
        format!("\"{word}\"")
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
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

/// Copies the AppImage into place and gives it a menu entry.
///
/// Copied to a name beside the destination and renamed over it, so an AppImage that is running right now
/// -- which is the usual state of the one being replaced -- keeps running from the file it opened, and
/// the next start gets the new one.
fn place_appimage(from: &Path, to: &Path, entry: &Path, icon: &Path) -> Result<(), Problem> {
    let local = |what: &str, path: &Path, e: std::io::Error| {
        Problem::Local(format!("Could not {what} {}: {e}", path.display()))
    };
    if let Some(folder) = to.parent() {
        std::fs::create_dir_all(folder).map_err(|e| local("make", folder, e))?;
    }
    let partial = to.with_extension("AppImage.partial");
    std::fs::copy(from, &partial).map_err(|e| local("write", &partial, e))?;
    make_executable(&partial).map_err(|e| local("make executable", &partial, e))?;
    std::fs::rename(&partial, to).map_err(|e| local("put the AppImage at", to, e))?;

    for folder in [entry.parent(), icon.parent()].into_iter().flatten() {
        std::fs::create_dir_all(folder).map_err(|e| local("make", folder, e))?;
    }
    std::fs::write(icon, ICON).map_err(|e| local("write", icon, e))?;
    std::fs::write(entry, desktop_entry(to, icon)).map_err(|e| local("write", entry, e))?;

    // The menu notices a new entry by itself on most desktops; this is for the ones that keep a cache.
    // Its absence, or a complaint from it, changes nothing that matters.
    if let Some(folder) = entry.parent() {
        if crate::system::on_path("update-desktop-database") {
            let _ = Command::new("update-desktop-database")
                .arg(folder)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    Ok(())
}

/// The menu entry for an AppImage at [exec], with its icon at [icon].
///
/// Written out in full rather than templated, because the one thing in it that varies -- the path -- has
/// to be escaped twice over: once as an argument in the Exec line, where quotes, backticks, dollars and
/// backslashes are special, and then again as a value in the file, where the backslash is. A home folder
/// with a space in it is ordinary; one with a dollar sign is rare but not impossible.
pub fn desktop_entry(exec: &Path, icon: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Noctorium\n\
         GenericName=Music player\n\
         Comment=One music player for YouTube Music and SoundCloud\n\
         Exec={}\n\
         Icon={}\n\
         Terminal=false\n\
         Categories=AudioVideo;Audio;Player;\n\
         StartupWMClass=app-noctorium-MainKt\n\
         X-AppImage-Integrate=false\n",
        exec_argument(&exec.to_string_lossy()),
        icon.to_string_lossy().replace('\\', "\\\\")
    )
}

fn exec_argument(path: &str) -> String {
    let mut quoted = String::from("\"");
    for c in path.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            // A field code everywhere in the Exec line, quoted or not.
            '%' => quoted.push_str("%%"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    // And then the file's own escaping, in which a backslash is written as two.
    quoted.replace('\\', "\\\\")
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Unpacks the Noctorium CLI and makes `noctorium` something a new terminal can run: through [link], or
/// by putting its folder on the user's PATH when [path] says to.
fn install_cli(
    archive: &Path,
    into: &Path,
    link: Option<&Path>,
    path: bool,
) -> Result<(), Problem> {
    archive::install_folder(archive, into, "the Noctorium CLI")?;
    // For the same reason as the application's: a flagged launcher, or a flagged Java runtime under it,
    // would be stopped by Gatekeeper the first time it was run.
    if cfg!(target_os = "macos") {
        clear_quarantine(into);
    }
    let launcher = archive::launcher_in(into, cfg!(windows)).ok_or_else(|| {
        Problem::Local(format!(
            "The Noctorium CLI was unpacked into {}, but there is no launcher in it ({}). The archive \
             is not the shape this installer expects; the releases page has it to unpack by hand.",
            into.display(),
            if cfg!(windows) {
                "noctorium.exe"
            } else {
                "bin/noctorium"
            }
        ))
    })?;

    match link {
        Some(link) => link_launcher(&launcher, link, "the Noctorium CLI"),
        None if path => {
            // The folder the launcher is in, which is the install folder itself for the published
            // archive and its bin folder for one laid out the other way.
            let folder = launcher.parent().unwrap_or(into);
            add_to_user_path(folder)
        }
        None => Ok(()),
    }
}

/// Links [link] to [launcher], which is [product]'s program, in place of a link this installer made
/// before.
#[cfg(unix)]
fn link_launcher(launcher: &Path, link: &Path, product: &str) -> Result<(), Problem> {
    let local = |what: &str, path: &Path, e: std::io::Error| {
        Problem::Local(format!("Could not {what} {}: {e}", path.display()))
    };
    make_executable(launcher).map_err(|e| local("make executable", launcher, e))?;
    if let Some(folder) = link.parent() {
        std::fs::create_dir_all(folder).map_err(|e| local("make", folder, e))?;
    }
    // A link this installer made before is replaced. Anything else by that name is somebody's own, and
    // it is not this program's to delete.
    if let Ok(existing) = std::fs::symlink_metadata(link) {
        if !existing.file_type().is_symlink() {
            return Err(Problem::Local(format!(
                "{} is already there and is not a link to {product}, so it has been left alone. It is \
                 unpacked as {}; move that file aside and run this again, or run it from where it is.",
                link.display(),
                launcher.display()
            )));
        }
        std::fs::remove_file(link).map_err(|e| local("replace", link, e))?;
    }
    std::os::unix::fs::symlink(launcher, link).map_err(|e| local("make the link", link, e))
}

#[cfg(not(unix))]
fn link_launcher(_: &Path, _: &Path, _: &str) -> Result<(), Problem> {
    Ok(())
}

// ---------------------------------------------------------------- Noctorium Stats

/// The program inside an unpacked Noctorium Stats.
///
/// Looked for rather than assumed, as the CLI's launcher is: the archive is published with it at the top,
/// and one that puts it in a `bin` folder, or names it in the other system's way, still installs.
pub fn stats_program_in(folder: &Path, windows: bool) -> Option<PathBuf> {
    let candidates: &[&str] = if windows {
        &[
            STATS_WINDOWS_PROGRAM,
            "noctorium-stats.exe",
            "bin/Noctorium Stats.exe",
            "bin/noctorium-stats.exe",
        ]
    } else {
        &[STATS_COMMAND, "bin/noctorium-stats"]
    };
    candidates
        .iter()
        .map(|relative| folder.join(relative))
        .find(|candidate| candidate.is_file())
}

/// Unpacks Noctorium Stats into [into], in place of an older copy, and on Linux links [link] to it.
fn install_stats(archive: &Path, into: &Path, link: Option<&Path>) -> Result<(), Problem> {
    // Asked again, because the download took long enough for somebody to have opened it.
    if cfg!(windows) {
        refuse_if_stats_is_open()?;
    }
    unpack_stats(archive, into, link, cfg!(windows))
}

/// The part of [install_stats] that does not ask Windows what is running, which a test cannot answer for,
/// and is told which system's program to look for, so either can be tried on any.
fn unpack_stats(
    archive: &Path,
    into: &Path,
    link: Option<&Path>,
    windows: bool,
) -> Result<(), Problem> {
    archive::install_folder(archive, into, STATS_NAME)?;
    let program = stats_program_in(into, windows).ok_or_else(|| {
        Problem::Local(format!(
            "Noctorium Stats was unpacked into {}, but its program is not in it ({}). The archive is \
             not the shape this installer expects; the releases page has it to unpack by hand.",
            into.display(),
            if windows {
                STATS_WINDOWS_PROGRAM
            } else {
                STATS_COMMAND
            }
        ))
    })?;
    match link {
        Some(link) => link_launcher(&program, link, STATS_NAME),
        None => Ok(()),
    }
}

/// The Start menu shortcut at [shortcut], to the program in [folder], which starts in that folder.
fn start_menu_shortcut(folder: &Path, shortcut: &Path) -> Result<(), Problem> {
    let program = stats_program_in(folder, true).ok_or_else(|| {
        Problem::Local(format!(
            "There is no {STATS_WINDOWS_PROGRAM} in {} to make a shortcut to.",
            folder.display()
        ))
    })?;
    if let Some(parent) = shortcut.parent() {
        fs::create_dir_all(parent).map_err(|e| couldnt("make", parent, e))?;
    }
    shortcut::create(
        &program,
        folder,
        "Your listening, in figures, from your Noctorium account",
        shortcut,
    )
    .map_err(|why| {
        Problem::Local(format!(
            "Noctorium Stats is installed in {}, but its Start menu shortcut could not be made: {why}. \
             It can be started from that folder.",
            folder.display()
        ))
    })
}

/// What Apps & features is told about Noctorium Stats, as the registry values under
/// [STATS_UNINSTALL_KEY]: the names Windows reads, and their values.
///
/// The uninstall line is what Windows runs when somebody chooses Uninstall there. It is PowerShell, which
/// every Windows has, told to close Noctorium Stats if it is open -- Windows will not delete a running
/// program's folder -- and then take away the folder, the shortcut and this entry. Its settings and its
/// sign-in are its own and stay, as Noctorium's do when it is uninstalled.
pub fn installed_app_values(
    folder: &Path,
    shortcut: &Path,
    version: &str,
    kilobytes: u32,
) -> Vec<(&'static str, Value)> {
    let program = folder.join(STATS_WINDOWS_PROGRAM);
    let uninstall = format!(
        "\"{}\" {}",
        powershell().display(),
        uninstall_arguments("Noctorium Stats", folder, shortcut, STATS_UNINSTALL_KEY)
    );
    vec![
        ("DisplayName", Value::Text(STATS_NAME.into())),
        ("DisplayVersion", Value::Text(version.into())),
        ("Publisher", Value::Text("Noctorium".into())),
        (
            "DisplayIcon",
            Value::Text(format!("{},0", program.display())),
        ),
        ("InstallLocation", Value::Text(folder.display().to_string())),
        ("UninstallString", Value::Text(uninstall.clone())),
        ("QuietUninstallString", Value::Text(uninstall)),
        (
            "URLInfoAbout",
            Value::Text("https://noctorium.vercel.app".into()),
        ),
        ("NoModify", Value::Number(1)),
        ("NoRepair", Value::Number(1)),
        ("EstimatedSize", Value::Number(kilobytes)),
    ]
}

/// A registry value, as far as Apps & features needs one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Text(String),
    Number(u32),
}

/// PowerShell's arguments for taking Noctorium Stats away again: the process called [process] closed, then
/// [folder], [shortcut] and the key [key] under HKEY_CURRENT_USER removed, each as far as it is there.
///
/// One line for `-Command`, with every path in single quotes, in which PowerShell reads nothing but a
/// doubled quote -- no `$`, no backtick -- and a Windows path cannot hold the double quote that would end
/// the argument. So a folder with an apostrophe, a dollar or an accent in it is removed as surely as one
/// without.
///
/// The entry goes last, and only once the folder has: an uninstall that could not delete the program --
/// something still holding it open -- leaves the entry where it was to be tried again, and says so with
/// its exit status, rather than leaving a program behind with nothing in the list to remove it by.
pub fn uninstall_arguments(process: &str, folder: &Path, shortcut: &Path, key: &str) -> String {
    let quoted = |text: &str| format!("'{}'", text.replace('\'', "''"));
    let folder = quoted(&folder.to_string_lossy());
    format!(
        "-NoProfile -NonInteractive -WindowStyle Hidden -Command \"\
         Stop-Process -Name {} -Force -ErrorAction SilentlyContinue; \
         Start-Sleep -Milliseconds 500; \
         Remove-Item -LiteralPath {folder} -Recurse -Force -ErrorAction SilentlyContinue; \
         Remove-Item -LiteralPath {} -Force -ErrorAction SilentlyContinue; \
         if (Test-Path -LiteralPath {folder}) {{ exit 1 }}; \
         Remove-Item -LiteralPath {} -Recurse -Force -ErrorAction SilentlyContinue; \
         exit 0\"",
        quoted(process),
        quoted(&shortcut.to_string_lossy()),
        quoted(&format!("HKCU:\\{key}")),
    )
}

/// Windows PowerShell, from System32 rather than from PATH, for the same reason msiexec is.
fn powershell() -> PathBuf {
    let windows = std::env::var_os("SystemRoot")
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    windows
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe")
}

/// The size of everything in [folder], in bytes, for Apps & features to show in kilobytes.
#[cfg(windows)]
fn bytes_in(folder: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(folder) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => bytes_in(&entry.path()),
            Ok(_) => entry.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

/// Enters Noctorium Stats in this user's list of installed programs.
#[cfg(windows)]
fn list_in_installed_apps(folder: &Path, shortcut: &Path, version: &str) -> Result<(), Problem> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, REG_DWORD, REG_SZ,
    };
    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() };
    let failed = |code: u32| {
        Problem::Local(format!(
            "Noctorium Stats is installed, but could not be listed among Windows' installed apps \
             (Windows error {code}). It works all the same; to remove it, delete {} and its Start menu \
             shortcut.",
            folder.display()
        ))
    };
    let kilobytes = u32::try_from(bytes_in(folder).div_ceil(1024)).unwrap_or(u32::MAX);
    let values = installed_app_values(folder, shortcut, version, kilobytes);

    let subkey = wide(STATS_UNINSTALL_KEY);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: the name is NUL-terminated and outlives the call; the key is written only when the call
    // succeeds, and closed on every way out below.
    let created = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, subkey.as_ptr(), &mut key) };
    if created != ERROR_SUCCESS {
        return Err(failed(created));
    }
    for (name, value) in values {
        let name = wide(name);
        let (kind, bytes): (u32, Vec<u8>) = match value {
            Value::Text(text) => (
                REG_SZ,
                wide(&text)
                    .iter()
                    .flat_map(|unit| unit.to_le_bytes())
                    .collect(),
            ),
            Value::Number(number) => (REG_DWORD, number.to_le_bytes().to_vec()),
        };
        // SAFETY: the name is NUL-terminated, and the data is the number of bytes passed with it.
        let written = unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                kind,
                bytes.as_ptr(),
                bytes.len() as u32,
            )
        };
        if written != ERROR_SUCCESS {
            // SAFETY: opened above and not used again.
            unsafe { RegCloseKey(key) };
            return Err(failed(written));
        }
    }
    // SAFETY: opened above and not used again.
    unsafe { RegCloseKey(key) };
    Ok(())
}

#[cfg(not(windows))]
fn list_in_installed_apps(_: &Path, _: &Path, _: &str) -> Result<(), Problem> {
    Err(Problem::Local(
        "The list of installed apps is Windows', and this is not Windows.".into(),
    ))
}

/// Puts Noctorium Stats in the applications menu: the menu entry it came with, made to point at where it
/// was unpacked, and its icon in the theme at [icons], in the folder for its size.
///
/// Rewritten rather than copied, because the entry it ships with names its program and icon the way an
/// installed package would -- `noctorium-stats`, found on PATH -- and a desktop started before
/// ~/.local/bin existed has a PATH without it. Absolute paths are found whatever the session's PATH is. An
/// archive with no entry, or no icon, gets one written here, with Noctorium's mark.
fn menu_entry(folder: &Path, entry: &Path, icons: &Path) -> Result<(), Problem> {
    let program = stats_program_in(folder, false).ok_or_else(|| {
        Problem::Local(format!(
            "There is no {STATS_COMMAND} in {} to add to the menu.",
            folder.display()
        ))
    })?;
    let shipped_icon = fs::read(folder.join(format!("{STATS_COMMAND}.png"))).ok();
    let icon_bytes = shipped_icon.as_deref().unwrap_or(ICON);
    let side = png_side(icon_bytes).unwrap_or(128);
    let icon = icons
        .join(format!("{side}x{side}"))
        .join("apps")
        .join(format!("{STATS_COMMAND}.png"));
    let shipped_entry = fs::read(folder.join(format!("{STATS_COMMAND}.desktop")))
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());

    for parent in [icon.parent(), entry.parent()].into_iter().flatten() {
        fs::create_dir_all(parent).map_err(|e| couldnt("make", parent, e))?;
    }
    fs::write(&icon, icon_bytes).map_err(|e| couldnt("write", &icon, e))?;
    let written = stats_desktop_entry(shipped_entry.as_deref(), &program, &icon);
    fs::write(entry, written).map_err(|e| couldnt("write", entry, e))?;

    // As for the AppImage's: for the desktops that keep a cache of the menu.
    if let Some(parent) = entry.parent() {
        if crate::system::on_path("update-desktop-database") {
            let _ = Command::new("update-desktop-database")
                .arg(parent)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    Ok(())
}

/// The width of a square PNG, read from its header, which is all that decides the folder an icon theme
/// keeps it in; nothing for anything that is not a square PNG.
pub fn png_side(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (width == height && width > 0).then_some(width)
}

/// Noctorium Stats' menu entry, from the one it shipped with when there is one: every `Exec` that runs
/// `noctorium-stats` made to run [program] instead, with its arguments kept, every `TryExec` made
/// [program], and every `Icon` made [icon] -- with everything else, its name, its words in other
/// languages, its categories, as it was.
///
/// An entry with no `Exec` for it to point somewhere, or none at all, is written here in full.
pub fn stats_desktop_entry(shipped: Option<&str>, program: &Path, icon: &Path) -> String {
    // An Exec line is a command line, quoted and escaped as one; TryExec and Icon are plain values, in
    // which only the backslash is special.
    let exec = exec_argument(&program.to_string_lossy());
    let try_exec = program.to_string_lossy().replace('\\', "\\\\");
    let icon = icon.to_string_lossy().replace('\\', "\\\\");
    if let Some(shipped) = shipped {
        let mut pointed = false;
        let lines: Vec<String> = shipped
            .lines()
            .map(|line| {
                let line = line.trim_end_matches('\r');
                let Some((key, value)) = line.split_once('=') else {
                    return line.to_string();
                };
                match key.trim() {
                    "Exec" => match value.trim().split_once(char::is_whitespace) {
                        Some((first, rest)) if runs_stats(first) => {
                            pointed = true;
                            format!("Exec={exec} {}", rest.trim_start())
                        }
                        None if runs_stats(value.trim()) => {
                            pointed = true;
                            format!("Exec={exec}")
                        }
                        _ => line.to_string(),
                    },
                    "TryExec" if runs_stats(value.trim()) => format!("TryExec={try_exec}"),
                    "Icon" => format!("Icon={icon}"),
                    _ => line.to_string(),
                }
            })
            .collect();
        if pointed {
            return lines.join("\n") + "\n";
        }
    }
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={STATS_NAME}\n\
         GenericName=Listening statistics\n\
         Comment=Your listening, in figures, from your Noctorium account\n\
         Exec={exec}\n\
         Icon={icon}\n\
         Terminal=false\n\
         Categories=AudioVideo;Audio;\n\
         StartupWMClass={STATS_COMMAND}\n"
    )
}

/// Whether the program an `Exec` line starts with is Noctorium Stats', by any path or none.
fn runs_stats(program: &str) -> bool {
    let program = program.trim_matches('"');
    program == STATS_COMMAND || program.ends_with(&format!("/{STATS_COMMAND}"))
}

/// Making a Start menu shortcut, which is a COM object and nothing else: the `.lnk` format is Windows'
/// own, and IShellLink is the one thing that writes it the way Explorer reads it.
mod shortcut {
    use std::path::Path;

    #[cfg(windows)]
    pub fn create(
        program: &Path,
        folder: &Path,
        description: &str,
        at: &Path,
    ) -> Result<(), String> {
        use std::ffi::c_void;
        use windows_sys::core::{GUID, HRESULT};
        use windows_sys::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        };

        // windows-sys has the functions but not the interfaces, so the two used are laid out here, in
        // the order ShObjIdl_core.h and ObjIdl.h declare them. Only the methods called are typed; the
        // rest are there to keep the ones after them at the right place.
        type Unused = usize;
        #[repr(C)]
        struct ShellLinkVtbl {
            query_interface:
                unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
            add_ref: Unused,
            release: unsafe extern "system" fn(*mut c_void) -> u32,
            get_path: Unused,
            get_id_list: Unused,
            set_id_list: Unused,
            get_description: Unused,
            set_description: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
            get_working_directory: Unused,
            set_working_directory: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
            get_arguments: Unused,
            set_arguments: Unused,
            get_hotkey: Unused,
            set_hotkey: Unused,
            get_show_cmd: Unused,
            set_show_cmd: Unused,
            get_icon_location: Unused,
            set_icon_location: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> HRESULT,
            set_relative_path: Unused,
            resolve: Unused,
            set_path: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
        }
        #[repr(C)]
        struct PersistFileVtbl {
            query_interface: Unused,
            add_ref: Unused,
            release: unsafe extern "system" fn(*mut c_void) -> u32,
            get_class_id: Unused,
            is_dirty: Unused,
            load: Unused,
            save: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> HRESULT,
        }
        const CLSID_SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
        const IID_ISHELL_LINK_W: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
        const IID_IPERSIST_FILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);

        let wide = |path: &std::ffi::OsStr| -> Vec<u16> {
            use std::os::windows::ffi::OsStrExt;
            path.encode_wide().chain(Some(0)).collect()
        };
        let checked = |what: &str, result: HRESULT| {
            if result < 0 {
                Err(format!("{what} failed (0x{:08X})", result as u32))
            } else {
                Ok(())
            }
        };
        let program = wide(program.as_os_str());
        let folder = wide(folder.as_os_str());
        let description = wide(std::ffi::OsStr::new(description));
        let at = wide(at.as_os_str());

        // SAFETY: COM is started on this thread and ended on it, if it was this that started it; every
        // interface pointer is checked before it is used and released exactly once; every string passed
        // is NUL-terminated and outlives the call it is passed to.
        unsafe {
            // A thread that already has COM in another mode answers RPC_E_CHANGED_MODE, a failure, and
            // the shell link works there all the same; only a success is ended again below.
            let started = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) >= 0;
            let mut link: *mut c_void = std::ptr::null_mut();
            let outcome = checked(
                "Creating a shell link",
                CoCreateInstance(
                    &CLSID_SHELL_LINK,
                    std::ptr::null_mut(),
                    CLSCTX_INPROC_SERVER,
                    &IID_ISHELL_LINK_W,
                    &mut link,
                ),
            )
            .and_then(|()| {
                let vtbl = &**(link as *mut *const ShellLinkVtbl);
                let mut file: *mut c_void = std::ptr::null_mut();
                let made = checked(
                    "Setting its program",
                    (vtbl.set_path)(link, program.as_ptr()),
                )
                .and_then(|()| {
                    checked(
                        "Setting its folder",
                        (vtbl.set_working_directory)(link, folder.as_ptr()),
                    )
                })
                .and_then(|()| {
                    checked(
                        "Setting its description",
                        (vtbl.set_description)(link, description.as_ptr()),
                    )
                })
                .and_then(|()| {
                    checked(
                        "Setting its icon",
                        (vtbl.set_icon_location)(link, program.as_ptr(), 0),
                    )
                })
                .and_then(|()| {
                    checked(
                        "Asking it how to be saved",
                        (vtbl.query_interface)(link, &IID_IPERSIST_FILE, &mut file),
                    )
                })
                .and_then(|()| {
                    let persist = &**(file as *mut *const PersistFileVtbl);
                    let saved = checked("Saving it", (persist.save)(file, at.as_ptr(), 1));
                    (persist.release)(file);
                    saved
                });
                (vtbl.release)(link);
                made
            });
            if started {
                CoUninitialize();
            }
            outcome
        }
    }

    #[cfg(not(windows))]
    pub fn create(_: &Path, _: &Path, _: &str, _: &Path) -> Result<(), String> {
        Err("a Start menu is Windows', and this is not Windows".into())
    }
}

/// The user's PATH with [directory] added at the end, or nothing when it is there already.
///
/// Windows compares PATH entries without regard to case or a trailing backslash, so this does too: a
/// second copy of the same folder in a different case is clutter that outlives the program. Entries can
/// be quoted, and an empty one -- two semicolons together -- is nothing.
pub fn path_with(existing: &str, directory: &str) -> Option<String> {
    let normal = |entry: &str| {
        entry
            .trim()
            .trim_matches('"')
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    let wanted = normal(directory);
    if existing.split(';').any(|entry| normal(entry) == wanted) {
        return None;
    }
    let kept = existing.trim_end_matches(';');
    Some(if kept.trim().is_empty() {
        directory.to_string()
    } else {
        format!("{kept};{directory}")
    })
}

/// Puts [directory] on this user's PATH, in the registry where Windows keeps it, and tells every open
/// program that it changed.
///
/// `HKCU\Environment` rather than `setx`, which truncates a PATH longer than 1024 characters -- silently,
/// and permanently. The broadcast is what makes a terminal opened from Explorer afterwards see the change;
/// one that is already open has its own copy of the environment and never will, which is why the end of
/// the install says to open a new one.
#[cfg(windows)]
fn add_to_user_path(directory: &Path) -> Result<(), Problem> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ, REG_SZ,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() };
    let failed = |what: &str, code: u32| {
        Problem::Local(format!(
            "Could not {what} your PATH (Windows error {code}). The Noctorium CLI is unpacked; add {} \
             to PATH yourself to run it from anywhere.",
            directory.display()
        ))
    };

    let subkey = wide("Environment");
    let name = wide("Path");
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: every pointer is to a live, correctly sized buffer owned by this function, and the key is
    // closed on every path out of it.
    unsafe {
        let opened = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            &mut key,
        );
        if opened != ERROR_SUCCESS {
            return Err(failed("open", opened));
        }

        let mut kind = 0u32;
        let mut size = 0u32;
        let asked = RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        );
        let existing = if asked == ERROR_FILE_NOT_FOUND {
            // No PATH of the user's own yet, which is unusual but allowed: the system one still applies.
            kind = REG_EXPAND_SZ;
            String::new()
        } else if asked != ERROR_SUCCESS {
            RegCloseKey(key);
            return Err(failed("read", asked));
        } else {
            let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
            let mut bytes = (buffer.len() * 2) as u32;
            let read = RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            );
            if read != ERROR_SUCCESS {
                RegCloseKey(key);
                return Err(failed("read", read));
            }
            let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
            String::from_utf16_lossy(&buffer[..length])
        };

        let Some(updated) = path_with(&existing, &directory.to_string_lossy()) else {
            RegCloseKey(key);
            return Ok(());
        };
        // Kept as whatever it was, so a PATH full of %USERPROFILE% stays expandable; a new one is made
        // expandable, which is what Windows itself writes.
        let kind = if kind == REG_SZ {
            REG_SZ
        } else {
            REG_EXPAND_SZ
        };
        let data = wide(&updated);
        let written = RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            kind,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        );
        RegCloseKey(key);
        if written != ERROR_SUCCESS {
            return Err(failed("change", written));
        }

        let environment = wide("Environment");
        let mut result = 0usize;
        // A program that does not answer within the timeout is skipped rather than waited on.
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            environment.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5_000,
            &mut result,
        );
    }
    Ok(())
}

#[cfg(not(windows))]
fn add_to_user_path(_: &Path) -> Result<(), Problem> {
    Ok(())
}

/// What to say at the end about how to start what was just installed.
pub fn how_to_start(method: Method, places: &Places, user_bin_on_path: bool) -> String {
    match method {
        Method::WindowsMsi | Method::WindowsSetup => {
            "Start it from the Start menu, or from the shortcut on the desktop.".into()
        }
        Method::MacDiskImage => format!(
            "Start it from Launchpad or Spotlight, or with open {}.",
            places
                .mac_applications()
                .map(|folder| places.show(&folder.join(MAC_APP)))
                .unwrap_or_else(|_| format!("/Applications/{MAC_APP}"))
        ),
        Method::Apt | Method::Dnf | Method::Zypper => {
            "Start it from your applications menu, or run /opt/noctorium/bin/Noctorium.".into()
        }
        Method::Pacman => "Start it from your applications menu, or run noctorium.".into(),
        Method::AppImage => format!(
            "Start it from your applications menu, or run {}.",
            places
                .appimage()
                .map(|p| places.show(&p))
                .unwrap_or_else(|_| "~/Applications/Noctorium.AppImage".into())
        ),
        Method::Flatpak => {
            format!("Start it from your applications menu, or run flatpak run {FLATPAK_ID}.")
        }
        // Quoted, since the folder has a space in it and this is for pasting into a terminal.
        Method::CliWindows if places.leave_path => format!(
            "Your PATH was left as it is, so run it as \"{}\".",
            places
                .cli_folder(true)
                .map(|folder| folder.join("noctorium.exe").display().to_string())
                .unwrap_or_else(|_| "noctorium.exe".into())
        ),
        Method::CliWindows => {
            "Open a new terminal -- one already open still has the old PATH -- and run noctorium."
                .into()
        }
        Method::CliLinux | Method::CliMac if user_bin_on_path => {
            "Run noctorium in a terminal.".into()
        }
        Method::CliLinux => {
            "~/.local/bin is not on your PATH yet, so run it as ~/.local/bin/noctorium, \
             or add export PATH=\"$HOME/.local/bin:$PATH\" to ~/.profile and open a new terminal."
                .into()
        }
        // This terminal read ~/.zprofile before the line was in it, and never will again.
        Method::CliMac => format!(
            "Open a new terminal, which reads the line just added to {}, and run noctorium -- or run it \
             here as ~/.local/bin/noctorium. A shell other than zsh needs {PROFILE_LINE} in its own \
             profile.",
            places
                .shell_profile()
                .map(|profile| places.show(&profile))
                .unwrap_or_else(|_| "~/.zprofile".into())
        ),
        Method::StatsWindows => {
            "Start it from the Start menu, where it is Noctorium Stats. It signs in with your \
             Noctorium account."
                .into()
        }
        Method::StatsLinux => format!(
            "Start it from your applications menu, or run {}. It signs in with your Noctorium account.",
            if user_bin_on_path {
                STATS_COMMAND.to_string()
            } else {
                format!("~/.local/bin/{STATS_COMMAND}")
            }
        ),
        // Quoted, since the name has a space in it and this is for pasting into a terminal.
        Method::StatsMac => format!(
            "Start it from Launchpad or Spotlight, or with open \"{}\". It signs in with your \
             Noctorium account.",
            places
                .mac_applications()
                .map(|folder| places.show(&folder.join(STATS_MAC_APP)))
                .unwrap_or_else(|_| format!("/Applications/{STATS_MAC_APP}"))
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places() -> Places {
        Places {
            home: Some(PathBuf::from("/home/sam")),
            data: Some(PathBuf::from("/home/sam/.local/share")),
            programs: Some(PathBuf::from(r"C:\Users\Sam\AppData\Local\Programs")),
            ..Places::default()
        }
    }

    /// An administrator's Mac, which can write to /Applications.
    fn mac_places() -> Places {
        Places {
            home: Some(PathBuf::from("/Users/sam")),
            data: Some(PathBuf::from("/Users/sam/.local/share")),
            applications: Some(PathBuf::from("/Applications")),
            ..Places::default()
        }
    }

    fn only_command(actions: Vec<Action>) -> (String, Vec<String>) {
        assert_eq!(actions.len(), 1, "{actions:?}");
        match actions.into_iter().next() {
            Some(Action::Run { program, args }) => (program, args),
            other => panic!("expected a command, got {other:?}"),
        }
    }

    fn command(method: Method, file: &str, sudo: Option<&str>) -> (String, Vec<String>) {
        only_command(actions_for(method, Path::new(file), sudo, &places(), false).unwrap())
    }

    #[test]
    fn a_release_without_an_msi_runs_its_setup_itself() {
        let (program, args) = command(Method::WindowsSetup, r"C:\Temp\Noctorium-setup.exe", None);
        assert_eq!(program, r"C:\Temp\Noctorium-setup.exe");
        assert!(args.is_empty(), "the installer asks its own questions");
    }

    const MSI: &str =
        r"C:\Users\Sam\AppData\Local\Temp\noctorium-installer\Noctorium-1.0.0-windows-x64.msi";

    /// The same line Noctorium's own updater hands msiexec, word for word.
    #[test]
    fn a_first_install_hands_msiexec_the_msi_and_nothing_to_click() {
        let actions =
            actions_for(Method::WindowsMsi, Path::new(MSI), None, &places(), false).unwrap();
        assert_eq!(
            actions,
            vec![Action::InstallMsi {
                msi: PathBuf::from(MSI),
                folder: None,
                wizard: false,
            }]
        );
        assert_eq!(
            msiexec_arguments(Path::new(MSI), None, false),
            format!(r#"/i "{MSI}" /passive /norestart MSIFASTINSTALL=7"#)
        );
        assert_eq!(
            actions[0].describe(&places()),
            format!(r#"msiexec /i "{MSI}" /passive /norestart MSIFASTINSTALL=7"#),
            "the plan shows exactly what will run"
        );
    }

    /// The .msi does not remember a folder chosen at the first install, and would move Noctorium back to
    /// Program Files without being told.
    #[test]
    fn an_upgrade_is_told_the_folder_noctorium_is_already_in() {
        let installed = Places {
            installed: Some(PathBuf::from(r"D:\Apps\Noctorium\")),
            ..places()
        };
        let actions =
            actions_for(Method::WindowsMsi, Path::new(MSI), None, &installed, false).unwrap();
        assert_eq!(
            actions,
            vec![Action::InstallMsi {
                msi: PathBuf::from(MSI),
                folder: Some(PathBuf::from(r"D:\Apps\Noctorium\")),
                wizard: false,
            }]
        );
        assert_eq!(
            msiexec_arguments(
                Path::new(MSI),
                Some(Path::new(r"D:\Apps\Noctorium\")),
                false
            ),
            format!(
                r#"/i "{MSI}" /passive /norestart MSIFASTINSTALL=7 INSTALLDIR="D:\Apps\Noctorium""#
            ),
            "and without the backslash InstallLocation ends with, before the closing quote"
        );
    }

    #[test]
    fn the_wizard_is_the_same_line_without_passive_and_starts_on_the_same_folder() {
        let installed = Places {
            installed: Some(PathBuf::from(r"C:\Program Files\Noctorium")),
            ..places()
        };
        let actions =
            actions_for(Method::WindowsMsi, Path::new(MSI), None, &installed, true).unwrap();
        assert_eq!(
            actions[0].describe(&installed),
            format!(
                r#"msiexec /i "{MSI}" /norestart MSIFASTINSTALL=7 INSTALLDIR="C:\Program Files\Noctorium""#
            )
        );
        // The wizard means nothing to anything but the .msi.
        assert_eq!(
            actions_for(Method::Apt, Path::new("/tmp/n.deb"), None, &places(), true).unwrap(),
            actions_for(Method::Apt, Path::new("/tmp/n.deb"), None, &places(), false).unwrap()
        );
    }

    /// Folders that people really have: spaces, an apostrophe, accents, a name in another script. A pair
    /// of double quotes is all any of them needs, since no Windows path can contain one.
    #[test]
    fn folders_of_every_shape_are_quoted_whole() {
        for (folder, written) in [
            (r"C:\Program Files\Noctorium", r"C:\Program Files\Noctorium"),
            (
                r"C:\Users\Seán O'Brien\Apps\Noctorium\",
                r"C:\Users\Seán O'Brien\Apps\Noctorium",
            ),
            (r"D:\Müzik Çalar\Noctorium", r"D:\Müzik Çalar\Noctorium"),
            (r"E:\音楽\Noctorium", r"E:\音楽\Noctorium"),
            (
                r"C:\Rock 'n' Roll (x86) & more\Noctorium",
                r"C:\Rock 'n' Roll (x86) & more\Noctorium",
            ),
            (r"\\server\share\Noctorium\", r"\\server\share\Noctorium"),
            // A drive's root keeps its backslash: `C:` alone is wherever C: was last.
            (r"F:\", r"F:\"),
        ] {
            let line = msiexec_arguments(
                Path::new(r"C:\Users\Seán O'Brien\AppData\Local\Temp\noctorium-installer\N.msi"),
                Some(Path::new(folder)),
                false,
            );
            assert!(
                line.starts_with(
                    r#"/i "C:\Users\Seán O'Brien\AppData\Local\Temp\noctorium-installer\N.msi" /passive"#
                ),
                "{line}"
            );
            assert!(
                line.ends_with(&format!(r#" INSTALLDIR="{written}""#)),
                "{folder}: {line}"
            );
            assert_eq!(line.matches('"').count(), 4, "{line}");
        }
    }

    /// What msiexec is handed is the line as it was written, with nothing quoted again around it. Asked of
    /// cmd's echo, which says back the rest of its own command line as it got it, since msiexec itself
    /// would install something.
    #[cfg(windows)]
    #[test]
    fn the_line_reaches_the_program_as_it_was_written() {
        let line = msiexec_arguments(
            Path::new(r"C:\Users\Sam O'Neil\AppData\Local\Temp\noctorium-installer\N 1.msi"),
            Some(Path::new(r"D:\Program Files (x86)\Noctorium\")),
            false,
        );
        let cmd = std::env::var_os("ComSpec")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32\cmd.exe"));
        let output = verbatim(&cmd, &format!("/d /c echo {line}"))
            .stdin(Stdio::null())
            .output()
            .expect("cmd runs");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim_end(), line);
        assert!(
            line.ends_with(r#"INSTALLDIR="D:\Program Files (x86)\Noctorium""#),
            "{line}"
        );
    }

    #[test]
    fn windows_installers_exit_codes_are_said_plainly() {
        assert!(matches!(msiexec_outcome(Some(0)), Ok(None)));
        assert!(
            matches!(msiexec_outcome(Some(3010)), Ok(Some(note)) if note.contains("restarting"))
        );
        for (code, said) in [
            (1602, "cancelled"),
            (1618, "installing something else"),
            (1603, "exit code 1603"),
        ] {
            match msiexec_outcome(Some(code)) {
                Err(problem) => assert!(problem.to_string().contains(said), "{code}: {problem}"),
                Ok(_) => panic!("{code} is not a success"),
            }
        }
        assert!(msiexec_outcome(None).is_err());
    }

    #[test]
    fn a_deb_goes_through_apt_so_dependencies_are_fetched_rather_than_reported() {
        let (program, args) = command(Method::Apt, "/tmp/noctorium.deb", None);
        assert_eq!(program, "apt-get");
        assert_eq!(args, vec!["install", "-y", "/tmp/noctorium.deb"]);
    }

    #[test]
    fn a_bare_file_name_is_made_into_a_path_so_apt_does_not_read_it_as_a_package() {
        let (_, args) = command(Method::Apt, "noctorium.deb", None);
        assert_eq!(args.last().unwrap(), "./noctorium.deb");
    }

    #[test]
    fn an_rpm_goes_through_dnf_on_fedora_and_zypper_on_opensuse() {
        let (program, args) = command(Method::Dnf, "/tmp/n.rpm", None);
        assert_eq!(program, "dnf");
        assert_eq!(args, vec!["install", "-y", "/tmp/n.rpm"]);

        let (program, args) = command(Method::Zypper, "/tmp/n.rpm", Some("sudo"));
        assert_eq!(program, "sudo");
        assert_eq!(
            args,
            vec![
                "zypper",
                "--non-interactive",
                "install",
                "--allow-unsigned-rpm",
                "/tmp/n.rpm"
            ]
        );
    }

    #[test]
    fn an_arch_package_goes_through_pacman() {
        let (program, args) = command(
            Method::Pacman,
            "/tmp/noctorium-1.0.0-1-x86_64.pkg.tar.zst",
            Some("pkexec"),
        );
        assert_eq!(program, "pkexec");
        assert_eq!(
            args,
            vec![
                "pacman",
                "-U",
                "--noconfirm",
                "/tmp/noctorium-1.0.0-1-x86_64.pkg.tar.zst"
            ]
        );
    }

    #[test]
    fn asking_for_rights_puts_the_tool_in_front_and_keeps_the_order() {
        let (program, args) = command(Method::Dnf, "/tmp/n.rpm", Some("pkexec"));
        assert_eq!(program, "pkexec");
        assert_eq!(args, vec!["dnf", "install", "-y", "/tmp/n.rpm"]);
    }

    /// A Flatpak is for this user, so nothing is escalated even when a tool to escalate with was found.
    #[test]
    fn a_flatpak_adds_flathub_for_its_runtime_clears_an_older_copy_and_installs_the_bundle() {
        let actions = actions_for(
            Method::Flatpak,
            Path::new("/tmp/Noctorium-1.0.0-x86_64.flatpak"),
            None,
            &places(),
            false,
        )
        .unwrap();
        assert_eq!(
            actions,
            vec![
                Action::Run {
                    program: "flatpak".into(),
                    args: [
                        "remote-add",
                        "--user",
                        "--if-not-exists",
                        "flathub",
                        "https://dl.flathub.org/repo/flathub.flatpakrepo"
                    ]
                    .map(String::from)
                    .to_vec(),
                },
                Action::ClearFlatpak {
                    id: "app.noctorium.Noctorium".into(),
                },
                Action::Run {
                    program: "flatpak".into(),
                    args: [
                        "install",
                        "--user",
                        "-y",
                        "--bundle",
                        "/tmp/Noctorium-1.0.0-x86_64.flatpak"
                    ]
                    .map(String::from)
                    .to_vec(),
                },
            ]
        );
    }

    #[test]
    fn an_appimage_goes_into_applications_with_a_menu_entry_beside_the_others() {
        let actions = actions_for(
            Method::AppImage,
            Path::new("/tmp/Noctorium-1.0.0-x86_64.AppImage"),
            None,
            &places(),
            false,
        )
        .unwrap();
        assert_eq!(
            actions,
            vec![Action::PlaceAppImage {
                from: PathBuf::from("/tmp/Noctorium-1.0.0-x86_64.AppImage"),
                to: PathBuf::from("/home/sam/Applications/Noctorium.AppImage"),
                entry: PathBuf::from("/home/sam/.local/share/applications/noctorium.desktop"),
                icon: PathBuf::from(
                    "/home/sam/.local/share/icons/hicolor/128x128/apps/noctorium.png"
                ),
            }]
        );
    }

    #[test]
    fn the_terminal_player_is_unpacked_into_the_users_own_folders() {
        let linux = actions_for(
            Method::CliLinux,
            Path::new("/tmp/noctorium-cli-1.0.0-linux-x64.tar.gz"),
            None,
            &places(),
            false,
        )
        .unwrap();
        assert_eq!(
            linux,
            vec![Action::UnpackCli {
                archive: PathBuf::from("/tmp/noctorium-cli-1.0.0-linux-x64.tar.gz"),
                into: PathBuf::from("/home/sam/.local/share/noctorium-cli"),
                link: Some(PathBuf::from("/home/sam/.local/bin/noctorium")),
                path: false,
            }]
        );

        let windows = actions_for(
            Method::CliWindows,
            Path::new(r"C:\Temp\noctorium-cli-1.0.0-windows-x64.zip"),
            None,
            &places(),
            false,
        )
        .unwrap();
        assert_eq!(
            windows,
            vec![Action::UnpackCli {
                archive: PathBuf::from(r"C:\Temp\noctorium-cli-1.0.0-windows-x64.zip"),
                into: PathBuf::from(r"C:\Users\Sam\AppData\Local\Programs").join("Noctorium CLI"),
                link: None,
                path: true,
            }]
        );
    }

    /// NOCTORIUM_NO_PATH: the CLI unpacked wherever LOCALAPPDATA says, and the account's PATH untouched,
    /// which the plan says and the end of the install allows for.
    #[test]
    fn the_path_can_be_left_alone_for_a_trial_install() {
        let trial = Places {
            programs: Some(PathBuf::from(r"C:\Temp\sandbox\Programs")),
            leave_path: true,
            ..places()
        };
        let actions = actions_for(
            Method::CliWindows,
            Path::new(r"C:\Temp\noctorium-cli-1.0.0-windows-x64.zip"),
            None,
            &trial,
            false,
        )
        .unwrap();
        assert_eq!(
            actions,
            vec![Action::UnpackCli {
                archive: PathBuf::from(r"C:\Temp\noctorium-cli-1.0.0-windows-x64.zip"),
                into: PathBuf::from(r"C:\Temp\sandbox\Programs").join("Noctorium CLI"),
                link: None,
                path: false,
            }]
        );
        let said = actions[0].describe(&trial);
        assert!(said.contains("leave your PATH as it is"), "{said}");
        let start = how_to_start(Method::CliWindows, &trial, false);
        assert!(
            start.contains("noctorium.exe") && start.contains("PATH was left"),
            "{start}"
        );
    }

    #[test]
    fn a_per_user_format_without_a_home_says_so() {
        let homeless = Places::default();
        assert!(matches!(
            actions_for(
                Method::AppImage,
                Path::new("/tmp/x"),
                None,
                &homeless,
                false
            ),
            Err(Problem::Local(_))
        ));
        assert!(actions_for(Method::Apt, Path::new("/tmp/x.deb"), None, &homeless, false).is_ok());
    }

    #[test]
    fn only_the_packages_need_root() {
        for method in [Method::Apt, Method::Dnf, Method::Zypper, Method::Pacman] {
            assert!(method.needs_root(), "{method:?}");
        }
        for method in [
            Method::WindowsMsi,
            Method::WindowsSetup,
            Method::MacDiskImage,
            Method::AppImage,
            Method::Flatpak,
            Method::CliLinux,
            Method::CliWindows,
            Method::CliMac,
        ] {
            assert!(!method.needs_root(), "{method:?}");
        }
    }

    #[test]
    fn a_mac_copies_the_application_out_of_its_disk_image_into_applications() {
        let actions = actions_for(
            Method::MacDiskImage,
            Path::new("/tmp/Noctorium-1.0.0-macos-arm64.dmg"),
            None,
            &mac_places(),
            false,
        )
        .unwrap();
        assert_eq!(
            actions,
            vec![Action::PlaceApp {
                image: PathBuf::from("/tmp/Noctorium-1.0.0-macos-arm64.dmg"),
                into: PathBuf::from("/Applications"),
            }]
        );
        let said = actions[0].describe(&mac_places());
        assert!(said.contains("Noctorium.app"), "{said}");
        assert!(said.contains("into /Applications"), "{said}");

        // An account that cannot write to /Applications has its own.
        let standard = Places {
            applications: Some(PathBuf::from("/Users/sam/Applications")),
            ..mac_places()
        };
        let actions = actions_for(
            Method::MacDiskImage,
            Path::new("/tmp/x.dmg"),
            None,
            &standard,
            false,
        )
        .unwrap();
        assert_eq!(
            actions,
            vec![Action::PlaceApp {
                image: PathBuf::from("/tmp/x.dmg"),
                into: PathBuf::from("/Users/sam/Applications"),
            }]
        );
        if !cfg!(windows) {
            assert!(actions[0]
                .describe(&standard)
                .contains("into ~/Applications"));
        }
    }

    #[test]
    fn a_mac_installs_the_terminal_player_where_linux_does() {
        let actions = actions_for(
            Method::CliMac,
            Path::new("/tmp/noctorium-cli-1.0.0-macos-arm64.tar.gz"),
            None,
            &mac_places(),
            false,
        )
        .unwrap();
        assert_eq!(
            actions,
            vec![Action::UnpackCli {
                archive: PathBuf::from("/tmp/noctorium-cli-1.0.0-macos-arm64.tar.gz"),
                into: PathBuf::from("/Users/sam/.local/share/noctorium-cli"),
                link: Some(PathBuf::from("/Users/sam/.local/bin/noctorium")),
                path: false,
            }]
        );
        assert_eq!(
            add_user_bin_to_path(&mac_places()).unwrap(),
            Action::AddToProfile {
                profile: PathBuf::from("/Users/sam/.zprofile"),
            }
        );
        assert!(matches!(
            add_user_bin_to_path(&Places::default()),
            Err(Problem::Local(_))
        ));
    }

    #[test]
    fn each_system_has_its_own_way_of_installing_the_terminal_player() {
        assert_eq!(Method::cli_for(Os::Windows), Method::CliWindows);
        assert_eq!(Method::cli_for(Os::Linux), Method::CliLinux);
        assert_eq!(Method::cli_for(Os::MacOs), Method::CliMac);
    }

    #[test]
    fn the_profile_line_is_added_once_and_apart_from_what_is_there() {
        let block = format!("{PROFILE_MARK}\n{PROFILE_LINE}\n");
        assert_eq!(profile_addition("").as_deref(), Some(block.as_str()));
        assert_eq!(
            profile_addition("eval \"$(/opt/homebrew/bin/brew shellenv)\"\n").as_deref(),
            Some(format!("\n{block}").as_str()),
            "a blank line between somebody's own lines and this one"
        );
        assert_eq!(
            profile_addition("export EDITOR=vim").as_deref(),
            Some(format!("\n\n{block}").as_str()),
            "a last line that was never ended is ended first"
        );

        // Already there, put there by this or by hand.
        assert_eq!(profile_addition(&format!("export A=1\n\n{block}")), None);
        assert_eq!(
            profile_addition("  export PATH=\"$HOME/.local/bin:$PATH\"  \n"),
            None
        );
        assert_eq!(profile_addition(&format!("{PROFILE_MARK}\n")), None);
    }

    #[test]
    fn running_the_install_twice_leaves_the_profile_with_one_line() {
        let here = crate::archive::tests::scratch("profile");
        let profile = here.join(".zprofile");
        add_to_profile(&profile).expect("made from nothing");
        add_to_profile(&profile).expect("and left alone the second time");
        assert_eq!(
            fs::read_to_string(&profile).unwrap(),
            format!("{PROFILE_MARK}\n{PROFILE_LINE}\n")
        );

        let other = here.join("existing");
        fs::write(&other, "export EDITOR=vim\n").unwrap();
        add_to_profile(&other).unwrap();
        add_to_profile(&other).unwrap();
        assert_eq!(
            fs::read_to_string(&other).unwrap(),
            format!("export EDITOR=vim\n\n{PROFILE_MARK}\n{PROFILE_LINE}\n"),
            "what was there stays, first"
        );
    }

    /// An application bundle with one file in it, as far as these tests need one.
    fn bundle(at: &Path, files: &[(&str, &str)]) {
        for (relative, contents) in files {
            let file = at.join(relative);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, contents).unwrap();
        }
    }

    /// What ditto does, as far as these tests can tell.
    fn copy_tree(from: &Path, to: &Path) -> Result<(), Problem> {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target)?;
            } else {
                fs::copy(entry.path(), &target).unwrap();
            }
        }
        Ok(())
    }

    fn version_in(app: &Path) -> String {
        fs::read_to_string(app.join("Contents/version")).unwrap()
    }

    #[test]
    fn the_application_is_found_at_the_top_of_its_disk_image() {
        let image = crate::archive::tests::scratch("image");
        bundle(
            &image.join("Noctorium.app"),
            &[("Contents/Info.plist", "<plist/>")],
        );
        // The link to /Applications a disk image carries, which here is a plain folder: what matters is
        // that it is not an application.
        fs::create_dir_all(image.join("Applications")).unwrap();
        assert_eq!(app_in(&image, MAC_APP), Some(image.join("Noctorium.app")));

        let renamed = crate::archive::tests::scratch("renamed");
        bundle(
            &renamed.join("Noctorium 1.0.app"),
            &[("Contents/Info.plist", "<plist/>")],
        );
        fs::create_dir_all(renamed.join("Applications")).unwrap();
        assert_eq!(
            app_in(&renamed, MAC_APP),
            Some(renamed.join("Noctorium 1.0.app"))
        );

        let two = crate::archive::tests::scratch("two");
        bundle(&two.join("One.app"), &[("Contents/Info.plist", "")]);
        bundle(&two.join("Two.app"), &[("Contents/Info.plist", "")]);
        assert_eq!(app_in(&two, MAC_APP), None, "two applications are not one");

        let empty = crate::archive::tests::scratch("empty");
        assert_eq!(app_in(&empty, MAC_APP), None);
    }

    #[test]
    fn a_first_install_makes_the_folder_it_goes_in() {
        let here = crate::archive::tests::scratch("first");
        let image = here.join("image");
        bundle(&image.join("Noctorium.app"), &[("Contents/version", "new")]);
        let folder = here.join("Users/sam/Applications");

        let installed = replace_app(&image.join("Noctorium.app"), &folder, MAC_APP, &copy_tree)
            .expect("installs");
        assert_eq!(installed, folder.join("Noctorium.app"));
        assert_eq!(version_in(&installed), "new");
    }

    /// The case the swap exists for: an upgrade leaves nothing of the old version, and nothing of itself.
    #[test]
    fn replacing_an_older_application_leaves_nothing_of_it() {
        let here = crate::archive::tests::scratch("upgrade");
        let folder = here.join("Applications");
        bundle(
            &folder.join("Noctorium.app"),
            &[
                ("Contents/version", "old"),
                ("Contents/app/old-only.jar", "stale"),
            ],
        );
        let image = here.join("image");
        bundle(&image.join("Noctorium.app"), &[("Contents/version", "new")]);

        let installed = replace_app(&image.join("Noctorium.app"), &folder, MAC_APP, &copy_tree)
            .expect("upgrades");
        assert_eq!(version_in(&installed), "new");
        assert!(
            !installed.join("Contents/app/old-only.jar").exists(),
            "a jar the new version does not have must not survive the upgrade"
        );
        assert!(!folder.join("Noctorium.app.old").exists());
        assert!(!folder.join("Noctorium.app.partial").exists());
        assert!(
            image.join("Noctorium.app/Contents/version").is_file(),
            "the disk image is copied from, never moved out of"
        );
    }

    /// A copy that fails half way -- a full disk, an image that will not read -- must never leave the Mac
    /// without the Noctorium it had.
    #[test]
    fn a_copy_that_fails_leaves_the_installed_application_as_it_was() {
        let here = crate::archive::tests::scratch("failed");
        let folder = here.join("Applications");
        bundle(
            &folder.join("Noctorium.app"),
            &[("Contents/version", "old")],
        );
        let image = here.join("image");
        bundle(&image.join("Noctorium.app"), &[("Contents/version", "new")]);

        let half_then_fail = |from: &Path, to: &Path| -> Result<(), Problem> {
            copy_tree(from, to)?;
            Err(Problem::Local(
                "ditto could not copy: No space left on device".into(),
            ))
        };
        let outcome = replace_app(
            &image.join("Noctorium.app"),
            &folder,
            MAC_APP,
            &half_then_fail,
        );
        assert!(matches!(outcome, Err(Problem::Local(why)) if why.contains("No space")));
        assert_eq!(version_in(&folder.join("Noctorium.app")), "old");
        assert!(!folder.join("Noctorium.app.partial").exists());
        assert!(!folder.join("Noctorium.app.old").exists());
    }

    /// Whatever an earlier run left half done is cleared away first, rather than copied into or over.
    #[test]
    fn leftovers_of_an_earlier_run_are_cleared_first() {
        let here = crate::archive::tests::scratch("leftovers");
        let folder = here.join("Applications");
        bundle(
            &folder.join("Noctorium.app.partial"),
            &[("Contents/half", "x")],
        );
        bundle(
            &folder.join("Noctorium.app.old"),
            &[("Contents/version", "older")],
        );
        let image = here.join("image");
        bundle(&image.join("Noctorium.app"), &[("Contents/version", "new")]);

        let installed =
            replace_app(&image.join("Noctorium.app"), &folder, MAC_APP, &copy_tree).unwrap();
        assert_eq!(version_in(&installed), "new");
        assert!(!installed.join("Contents/half").exists());
        assert!(!folder.join("Noctorium.app.partial").exists());
        assert!(!folder.join("Noctorium.app.old").exists());
    }

    #[test]
    fn a_command_is_shown_quoted_only_where_it_has_to_be() {
        let args = ["install", "-y", "/tmp/a b.deb"].map(String::from);
        let shown = command_line("sudo", &args);
        if cfg!(windows) {
            assert_eq!(shown, "sudo install -y \"/tmp/a b.deb\"");
        } else {
            assert_eq!(shown, "sudo install -y '/tmp/a b.deb'");
            assert_eq!(
                command_line("echo", &["it's".to_string()]),
                r"echo 'it'\''s'"
            );
        }
    }

    #[test]
    fn a_path_is_added_once_at_the_end() {
        let ours = r"C:\Users\Sam\AppData\Local\Programs\Noctorium CLI";
        assert_eq!(
            path_with(r"C:\Windows;C:\Tools", ours).as_deref(),
            Some(r"C:\Windows;C:\Tools;C:\Users\Sam\AppData\Local\Programs\Noctorium CLI")
        );
        assert_eq!(path_with("", ours).as_deref(), Some(ours));
        assert_eq!(
            path_with(r"C:\Tools;", ours).as_deref(),
            Some(r"C:\Tools;C:\Users\Sam\AppData\Local\Programs\Noctorium CLI"),
            "no empty entry from a trailing semicolon"
        );
    }

    #[test]
    fn a_path_already_there_in_any_spelling_is_left_alone() {
        let ours = r"C:\Users\Sam\AppData\Local\Programs\Noctorium CLI";
        assert_eq!(path_with(&format!(r"C:\Tools;{ours}"), ours), None);
        assert_eq!(
            path_with(
                r"c:\users\sam\appdata\local\programs\noctorium cli\;C:\Tools",
                ours
            ),
            None,
            "case and a trailing backslash do not make it a different folder"
        );
        assert_eq!(
            path_with(
                r#""C:\Users\Sam\AppData\Local\Programs\Noctorium CLI";C:\Tools"#,
                ours
            ),
            None,
            "nor do quotes"
        );
    }

    #[test]
    fn the_menu_entry_points_at_the_appimage_however_odd_its_path() {
        let entry = desktop_entry(
            Path::new("/home/sam/Applications/Noctorium.AppImage"),
            Path::new("/home/sam/.local/share/icons/hicolor/128x128/apps/noctorium.png"),
        );
        assert!(entry.starts_with("[Desktop Entry]\nType=Application\nName=Noctorium\n"));
        assert!(entry.contains("\nExec=\"/home/sam/Applications/Noctorium.AppImage\"\n"));
        assert!(entry
            .contains("\nIcon=/home/sam/.local/share/icons/hicolor/128x128/apps/noctorium.png\n"));
        assert!(entry.contains("\nCategories=AudioVideo;Audio;Player;\n"));

        // A space is fine inside the quotes; a dollar and a percent are not, and a backslash is escaped
        // twice, once for the Exec line and once for the file.
        let odd = desktop_entry(
            Path::new("/home/Sam Smith/$5 100%/a\\b/Noctorium.AppImage"),
            Path::new("/i.png"),
        );
        assert!(
            odd.contains("\nExec=\"/home/Sam Smith/\\\\$5 100%%/a\\\\\\\\b/Noctorium.AppImage\"\n"),
            "{odd}"
        );
    }

    #[test]
    fn each_install_says_how_to_start_it() {
        let places = places();
        assert!(how_to_start(Method::WindowsMsi, &places, true).contains("Start menu"));
        assert!(how_to_start(Method::Apt, &places, true).contains("/opt/noctorium/bin/Noctorium"));
        assert!(how_to_start(Method::Pacman, &places, true).contains("run noctorium"));
        assert!(how_to_start(Method::Flatpak, &places, true)
            .contains("flatpak run app.noctorium.Noctorium"));
        assert!(how_to_start(Method::CliWindows, &places, true).contains("new terminal"));
        assert!(how_to_start(Method::CliLinux, &places, false).contains("~/.local/bin/noctorium"));
        assert_eq!(
            how_to_start(Method::CliLinux, &places, true),
            "Run noctorium in a terminal."
        );

        let mac = mac_places();
        let start = how_to_start(Method::MacDiskImage, &mac, true);
        assert!(start.contains("Launchpad or Spotlight"), "{start}");
        // Joined with a backslash where the tests run on Windows.
        if !cfg!(windows) {
            assert!(
                start.contains("open /Applications/Noctorium.app"),
                "{start}"
            );
        }
        assert_eq!(
            how_to_start(Method::CliMac, &mac, true),
            "Run noctorium in a terminal."
        );
        let new_terminal = how_to_start(Method::CliMac, &mac, false);
        assert!(
            new_terminal.contains("Open a new terminal"),
            "{new_terminal}"
        );
        assert!(new_terminal.contains(".zprofile"), "{new_terminal}");
    }

    // ------------------------------------------------------------ Noctorium Stats

    use crate::archive::tests::{scratch, tar_gz, zip_file};

    #[test]
    fn stats_is_installed_for_this_user_on_every_system() {
        let windows = Places {
            start_menu: Some(PathBuf::from(
                r"C:\Users\Sam\AppData\Roaming\Start Menu\Programs",
            )),
            ..places()
        };
        let actions = actions_for(
            Method::StatsWindows,
            Path::new(r"C:\Temp\noctorium-stats-1.0.0-windows-x64.zip"),
            None,
            &windows,
            false,
        )
        .unwrap();
        let folder = PathBuf::from(r"C:\Users\Sam\AppData\Local\Programs").join("Noctorium Stats");
        assert_eq!(
            actions,
            vec![
                Action::UnpackStats {
                    archive: PathBuf::from(r"C:\Temp\noctorium-stats-1.0.0-windows-x64.zip"),
                    into: folder.clone(),
                    link: None,
                },
                Action::StartMenuShortcut {
                    folder: folder.clone(),
                    shortcut: PathBuf::from(r"C:\Users\Sam\AppData\Roaming\Start Menu\Programs")
                        .join("Noctorium Stats.lnk"),
                },
            ]
        );
        assert!(actions[1]
            .describe(&windows)
            .starts_with("add it to the Start menu"));
        assert_eq!(
            list_installed(&windows, "1.0.0").unwrap(),
            Some(Action::ListInstalled {
                folder,
                shortcut: PathBuf::from(r"C:\Users\Sam\AppData\Roaming\Start Menu\Programs")
                    .join("Noctorium Stats.lnk"),
                version: "1.0.0".into(),
            })
        );
        let trial = Places {
            leave_path: true,
            ..windows.clone()
        };
        assert_eq!(list_installed(&trial, "1.0.0").unwrap(), None);
        assert!(
            matches!(
                actions_for(
                    Method::StatsWindows,
                    Path::new("x.zip"),
                    None,
                    &places(),
                    false
                ),
                Err(Problem::Local(why)) if why.contains("APPDATA")
            ),
            "no Start menu, no install"
        );

        let linux = actions_for(
            Method::StatsLinux,
            Path::new("/tmp/noctorium-stats-1.0.0-linux-x64.tar.gz"),
            None,
            &places(),
            false,
        )
        .unwrap();
        assert_eq!(
            linux,
            vec![
                Action::UnpackStats {
                    archive: PathBuf::from("/tmp/noctorium-stats-1.0.0-linux-x64.tar.gz"),
                    into: PathBuf::from("/home/sam/.local/share/noctorium-stats"),
                    link: Some(PathBuf::from("/home/sam/.local/bin/noctorium-stats")),
                },
                Action::MenuEntry {
                    folder: PathBuf::from("/home/sam/.local/share/noctorium-stats"),
                    entry: PathBuf::from(
                        "/home/sam/.local/share/applications/noctorium-stats.desktop"
                    ),
                    icons: PathBuf::from("/home/sam/.local/share/icons/hicolor"),
                },
            ]
        );

        let mac = actions_for(
            Method::StatsMac,
            Path::new("/tmp/noctorium-stats-1.0.0-macos-arm64.zip"),
            None,
            &mac_places(),
            false,
        )
        .unwrap();
        assert_eq!(
            mac,
            vec![Action::PlaceZippedApp {
                archive: PathBuf::from("/tmp/noctorium-stats-1.0.0-macos-arm64.zip"),
                into: PathBuf::from("/Applications"),
                app: "Noctorium Stats.app".into(),
            }]
        );
        assert!(mac[0]
            .describe(&mac_places())
            .contains("Noctorium Stats.app"));

        for method in [Method::StatsWindows, Method::StatsLinux, Method::StatsMac] {
            assert!(!method.needs_root(), "{method:?}");
        }
        assert_eq!(Method::stats_for(Os::Windows), Method::StatsWindows);
        assert_eq!(Method::stats_for(Os::Linux), Method::StatsLinux);
        assert_eq!(Method::stats_for(Os::MacOs), Method::StatsMac);
    }

    #[test]
    fn stats_says_how_to_start_it() {
        assert!(how_to_start(Method::StatsWindows, &places(), true).contains("Start menu"));
        assert!(how_to_start(Method::StatsLinux, &places(), true).contains("run noctorium-stats."));
        assert!(how_to_start(Method::StatsLinux, &places(), false)
            .contains("run ~/.local/bin/noctorium-stats."));
        let mac = how_to_start(Method::StatsMac, &mac_places(), true);
        assert!(mac.contains("Launchpad or Spotlight"), "{mac}");
        if !cfg!(windows) {
            assert!(
                mac.contains("open \"/Applications/Noctorium Stats.app\""),
                "{mac}"
            );
        }
    }

    /// The Windows archive as published: `Noctorium Stats.exe` alone, which becomes the folder's only
    /// file -- and installed again over itself, an older version leaves nothing behind.
    #[test]
    fn the_windows_archive_is_unpacked_into_a_folder_of_its_own() {
        let here = scratch("stats-windows");
        let into = here.join("Programs").join("Noctorium Stats");
        let first = here.join("noctorium-stats-1.0.0-windows-x64.zip");
        zip_file(
            &first,
            &[("Noctorium Stats.exe", "MZ one"), ("old-only.dll", "stale")],
        );
        unpack_stats(&first, &into, None, true).expect("first install");
        let second = here.join("noctorium-stats-1.1.0-windows-x64.zip");
        zip_file(&second, &[("Noctorium Stats.exe", "MZ two")]);
        unpack_stats(&second, &into, None, true).expect("upgrade");
        assert_eq!(
            fs::read_to_string(into.join("Noctorium Stats.exe")).unwrap(),
            "MZ two"
        );
        assert!(!into.join("old-only.dll").exists());
        assert_eq!(
            stats_program_in(&into, true),
            Some(into.join("Noctorium Stats.exe"))
        );

        // One in a folder of its own at the top installs the same.
        let foldered = here.join("foldered.zip");
        zip_file(
            &foldered,
            &[("noctorium-stats/Noctorium Stats.exe", "MZ three")],
        );
        unpack_stats(&foldered, &into, None, true).expect("installs");
        assert_eq!(
            fs::read_to_string(into.join("Noctorium Stats.exe")).unwrap(),
            "MZ three"
        );
    }

    #[test]
    fn an_archive_without_the_program_is_said_to_be_the_wrong_shape() {
        let here = scratch("stats-shape");
        let archive = here.join("noctorium-stats-1.0.0-windows-x64.zip");
        zip_file(&archive, &[("README.txt", "nothing to run")]);
        let into = here.join("Noctorium Stats");
        let said = unpack_stats(&archive, &into, None, true)
            .unwrap_err()
            .to_string();
        assert!(said.contains("its program is not in it"), "{said}");
    }

    /// The Linux archive as published -- the program, its menu entry and its icon -- unpacked, linked,
    /// and put in the menu pointing at where it is, with its icon in the folder for its size.
    #[test]
    fn the_linux_archive_is_unpacked_linked_and_put_in_the_menu() {
        let here = scratch("stats-linux");
        let archive = here.join("noctorium-stats-1.0.0-linux-x64.tar.gz");
        tar_gz(
            &archive,
            &[
                ("noctorium-stats", "#!/bin/sh\necho stats\n", 0o755),
                (
                    "noctorium-stats.desktop",
                    "[Desktop Entry]\nType=Application\nName=Noctorium Stats\nName[de]=Noctorium-Statistik\n\
                     TryExec=noctorium-stats\nExec=noctorium-stats %U\nIcon=noctorium-stats\n\
                     Categories=AudioVideo;\n",
                    0o644,
                ),
                ("noctorium-stats.png", "not really a PNG", 0o644),
            ],
        );
        let folder = here.join("share").join("noctorium-stats");
        let link = here.join("bin").join("noctorium-stats");
        unpack_stats(&archive, &folder, Some(&link), false).expect("unpacks");
        let program = folder.join("noctorium-stats");
        assert_eq!(stats_program_in(&folder, false), Some(program.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::read_link(&link).unwrap(), program);
            let mode = fs::metadata(&program).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "the program must stay executable");
        }

        let entry = here.join("share/applications/noctorium-stats.desktop");
        let icons = here.join("share/icons/hicolor");
        menu_entry(&folder, &entry, &icons).expect("in the menu");
        let written = fs::read_to_string(&entry).unwrap();
        // Not a PNG, so its size cannot be read, and it goes where a 128-pixel icon would.
        let icon = icons
            .join("128x128")
            .join("apps")
            .join("noctorium-stats.png");
        assert_eq!(fs::read_to_string(&icon).unwrap(), "not really a PNG");
        assert!(
            written.contains("\nName[de]=Noctorium-Statistik\n"),
            "{written}"
        );
        assert!(
            written.contains(&format!(
                "\nExec={} %U\n",
                exec_argument(&program.to_string_lossy())
            )),
            "{written}"
        );
        assert!(
            written.contains(&format!(
                "\nIcon={}\n",
                icon.to_string_lossy().replace('\\', "\\\\")
            )),
            "{written}"
        );
    }

    /// An archive with neither entry nor icon still gets both: an entry written here, and Noctorium's
    /// mark, which is a 128-pixel PNG.
    #[test]
    fn a_program_with_no_entry_or_icon_is_given_both() {
        let here = scratch("stats-bare");
        let folder = here.join("noctorium-stats");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("noctorium-stats"), "#!/bin/sh\n").unwrap();
        let entry = here.join("applications/noctorium-stats.desktop");
        let icons = here.join("icons/hicolor");
        menu_entry(&folder, &entry, &icons).expect("in the menu");
        let icon = icons.join("128x128/apps/noctorium-stats.png");
        assert_eq!(fs::read(&icon).unwrap(), ICON);
        let written = fs::read_to_string(&entry).unwrap();
        assert!(written.starts_with("[Desktop Entry]\nType=Application\nName=Noctorium Stats\n"));
        assert!(written.contains("\nTerminal=false\n"));

        let missing = here.join("empty");
        fs::create_dir_all(&missing).unwrap();
        assert!(menu_entry(&missing, &entry, &icons).is_err());
    }

    #[test]
    fn a_menu_entry_is_pointed_at_the_program_wherever_it_names_it() {
        let program = Path::new("/home/sam/.local/share/noctorium-stats/noctorium-stats");
        let icon =
            Path::new("/home/sam/.local/share/icons/hicolor/256x256/apps/noctorium-stats.png");
        let shipped =
            "[Desktop Entry]\r\nName=Noctorium Stats\r\nExec=/usr/bin/noctorium-stats\r\n\
                       TryExec=noctorium-stats\r\nIcon=noctorium-stats\r\n\r\n\
                       [Desktop Action refresh]\nName=Refresh\nExec=noctorium-stats --refresh\n\
                       [Desktop Action other]\nExec=xdg-open https://example.test\n";
        let written = stats_desktop_entry(Some(shipped), program, icon);
        assert_eq!(
            written,
            "[Desktop Entry]\nName=Noctorium Stats\n\
             Exec=\"/home/sam/.local/share/noctorium-stats/noctorium-stats\"\n\
             TryExec=/home/sam/.local/share/noctorium-stats/noctorium-stats\n\
             Icon=/home/sam/.local/share/icons/hicolor/256x256/apps/noctorium-stats.png\n\n\
             [Desktop Action refresh]\nName=Refresh\n\
             Exec=\"/home/sam/.local/share/noctorium-stats/noctorium-stats\" --refresh\n\
             [Desktop Action other]\nExec=xdg-open https://example.test\n"
        );

        // A home with a space and a dollar in it, escaped as an Exec line wants.
        let odd = stats_desktop_entry(
            Some("[Desktop Entry]\nExec=noctorium-stats\n"),
            Path::new("/home/Sam Smith/$5/noctorium-stats"),
            icon,
        );
        assert!(
            odd.contains("\nExec=\"/home/Sam Smith/\\\\$5/noctorium-stats\"\n"),
            "{odd}"
        );

        // Nothing that runs it, so nothing to point: written in full instead.
        let other = stats_desktop_entry(
            Some("[Desktop Entry]\nExec=something-else\n"),
            program,
            icon,
        );
        assert!(other.contains("\nName=Noctorium Stats\n"), "{other}");
        assert!(!other.contains("something-else"), "{other}");
        assert_eq!(stats_desktop_entry(None, program, icon), other);
    }

    #[test]
    fn an_icons_size_is_read_from_its_header() {
        assert_eq!(png_side(ICON), Some(128));
        let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        header.extend(256u32.to_be_bytes());
        header.extend(256u32.to_be_bytes());
        assert_eq!(png_side(&header), Some(256));
        let mut wide = header[..16].to_vec();
        wide.extend(512u32.to_be_bytes());
        wide.extend(256u32.to_be_bytes());
        assert_eq!(png_side(&wide), None, "not square");
        assert_eq!(png_side(b"GIF89a"), None);
        assert_eq!(png_side(b""), None);
    }

    /// The Mac's zip, as `ditto -c -k --keepParent` makes it, unpacked here by the program's own unpacker
    /// standing in for `ditto -x -k`, and copied by plain Rust standing in for `ditto`.
    #[test]
    fn a_macs_stats_comes_out_of_its_zip_into_applications() {
        let here = scratch("stats-mac");
        let applications = here.join("Applications");
        bundle(
            &applications.join("Noctorium Stats.app"),
            &[
                ("Contents/version", "old"),
                ("Contents/Resources/old-only", "stale"),
            ],
        );
        let archive = here.join("noctorium-stats-1.1.0-macos-arm64.zip");
        zip_file(
            &archive,
            &[
                ("Noctorium Stats.app/Contents/version", "new"),
                (
                    "Noctorium Stats.app/Contents/MacOS/noctorium-stats",
                    "\u{7f}ELF",
                ),
                ("__MACOSX/Noctorium Stats.app/._Contents", "resource fork"),
            ],
        );
        let installed = unzip_and_replace_app(
            &archive,
            &applications,
            STATS_MAC_APP,
            &crate::archive::unpack_into,
            &copy_tree,
        )
        .expect("installs");
        assert_eq!(installed, applications.join("Noctorium Stats.app"));
        assert_eq!(version_in(&installed), "new");
        assert!(installed.join("Contents/MacOS/noctorium-stats").is_file());
        assert!(!installed.join("Contents/Resources/old-only").exists());
        assert!(!applications.join("Noctorium Stats.app.old").exists());
        assert!(!applications.join("__MACOSX").exists());

        let wrong = here.join("wrong.zip");
        zip_file(&wrong, &[("README", "no application")]);
        let said = unzip_and_replace_app(
            &wrong,
            &applications,
            STATS_MAC_APP,
            &crate::archive::unpack_into,
            &copy_tree,
        )
        .unwrap_err()
        .to_string();
        assert!(said.contains("has no Noctorium Stats.app in it"), "{said}");
        assert_eq!(
            version_in(&applications.join("Noctorium Stats.app")),
            "new",
            "and what was installed is left as it was"
        );
    }

    /// Apps & features is told the name, the version, where it is and how to remove it, and the line
    /// that removes it quotes every path the one way PowerShell reads nothing inside.
    #[test]
    fn stats_is_entered_in_installed_apps_with_a_way_to_remove_it() {
        let folder = Path::new(r"C:\Users\Seán O'Brien\AppData\Local\Programs\Noctorium Stats");
        let shortcut = Path::new(r"C:\Users\Seán O'Brien\Start Menu\Programs\Noctorium Stats.lnk");
        let values = installed_app_values(folder, shortcut, "1.2.3", 7_000);
        let value = |name: &str| {
            values
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("no {name}"))
        };
        assert_eq!(value("DisplayName"), Value::Text("Noctorium Stats".into()));
        assert_eq!(value("DisplayVersion"), Value::Text("1.2.3".into()));
        assert_eq!(value("Publisher"), Value::Text("Noctorium".into()));
        assert_eq!(value("NoModify"), Value::Number(1));
        assert_eq!(value("EstimatedSize"), Value::Number(7_000));
        let Value::Text(uninstall) = value("UninstallString") else {
            panic!("not text")
        };
        assert!(
            uninstall.contains(r#"WindowsPowerShell\v1.0\powershell.exe" -NoProfile"#),
            "{uninstall}"
        );
        assert!(
            uninstall.contains(
                r"-LiteralPath 'C:\Users\Seán O''Brien\AppData\Local\Programs\Noctorium Stats'"
            ),
            "{uninstall}"
        );
        assert!(
            uninstall.contains(r"-LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\NoctoriumStats'"),
            "{uninstall}"
        );
        assert!(
            uninstall.contains("Stop-Process -Name 'Noctorium Stats'"),
            "{uninstall}"
        );
        // Two double quotes round the program, and one pair round the whole of the command.
        assert_eq!(uninstall.matches('"').count(), 4, "{uninstall}");
    }

    /// What Apps & features runs, run for real -- against a folder and a shortcut of the test's own, a
    /// process nobody has, and a registry key nobody has -- and both taken away.
    #[cfg(windows)]
    #[test]
    fn the_uninstall_line_takes_away_the_folder_and_the_shortcut() {
        let here = scratch("stats-uninstall");
        let folder = here.join("Zoë's $HOME`s Stats");
        fs::create_dir_all(folder.join("sub")).unwrap();
        fs::write(folder.join("Noctorium Stats.exe"), "MZ").unwrap();
        fs::write(folder.join("sub").join("data"), "x").unwrap();
        let shortcut = here.join("Noctorium Stats.lnk");
        fs::write(&shortcut, "link").unwrap();
        let line = uninstall_arguments(
            "noctorium-installer-test-nothing-runs-as-this",
            &folder,
            &shortcut,
            &format!(
                r"Software\NoctoriumInstallerTest\NoSuchKey-{}",
                std::process::id()
            ),
        );
        let status = verbatim(&powershell(), &line)
            .stdin(Stdio::null())
            .status()
            .expect("PowerShell runs");
        assert!(status.success(), "{line}");
        assert!(!folder.exists(), "{line}");
        assert!(!shortcut.exists(), "{line}");
        assert!(here.exists(), "and nothing above them");

        // A program that will not let go of its folder: the uninstall says it did not work, and the
        // entry -- were this the real one -- would be left to try again with.
        use std::os::windows::fs::OpenOptionsExt;
        let held = here.join("Held");
        fs::create_dir_all(&held).unwrap();
        fs::write(held.join("Noctorium Stats.exe"), "MZ").unwrap();
        let open = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(held.join("Noctorium Stats.exe"))
            .unwrap();
        let line = uninstall_arguments(
            "noctorium-installer-test-nothing-runs-as-this",
            &held,
            &here.join("none.lnk"),
            &format!(
                r"Software\NoctoriumInstallerTest\NoSuchKey-{}",
                std::process::id()
            ),
        );
        let status = verbatim(&powershell(), &line)
            .stdin(Stdio::null())
            .status()
            .expect("PowerShell runs");
        drop(open);
        assert_eq!(status.code(), Some(1), "{line}");
        assert!(held.join("Noctorium Stats.exe").exists());
    }

    /// A shortcut made in a folder of the test's own, read back by Windows' own scripting host, which is
    /// what Explorer reads it with.
    #[cfg(windows)]
    #[test]
    fn a_start_menu_shortcut_points_at_the_program() {
        let here = scratch("stats-shortcut");
        let folder = here.join("Noctorium Stats");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("Noctorium Stats.exe"), "MZ").unwrap();
        let shortcut = here.join("Start Menu").join("Noctorium Stats.lnk");
        start_menu_shortcut(&folder, &shortcut).expect("made");
        assert!(shortcut.is_file());
        let read = Command::new(powershell())
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!(
                "$l = (New-Object -ComObject WScript.Shell).CreateShortcut('{}'); \
                 $l.TargetPath; $l.WorkingDirectory",
                shortcut.display()
            ))
            .stdin(Stdio::null())
            .output()
            .expect("PowerShell runs");
        let said = String::from_utf8_lossy(&read.stdout);
        let lines: Vec<&str> = said.lines().map(str::trim).collect();
        // Compared by their ends and by what is there, since the temporary folder may be named in its
        // short form here and the shell link keeps the long one.
        assert_eq!(lines.len(), 2, "{said}");
        assert!(
            lines[0].ends_with(r"\Noctorium Stats\Noctorium Stats.exe"),
            "{said}"
        );
        assert!(Path::new(lines[0]).is_file(), "{said}");
        assert!(lines[1].ends_with(r"\Noctorium Stats"), "{said}");
        assert!(Path::new(lines[1]).is_dir(), "{said}");

        let empty = here.join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert!(start_menu_shortcut(&empty, &here.join("x.lnk")).is_err());
    }
}
