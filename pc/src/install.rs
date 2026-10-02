//! Handing the downloaded file to whatever installs software on this machine, or putting it in place.
//!
//! Packages go to the package manager, always: apt, dnf, zypper and pacman resolve dependencies, record
//! what they installed, and can remove it again, and an installer that copies files into /opt behind
//! their back leaves a machine whose package database is a lie. The per-user formats -- an AppImage, the
//! Noctorium CLI -- have no package manager, so for those this is the installer, and does the little an
//! installer does: put the file somewhere permanent, make it runnable, and tell the desktop or the shell
//! where it is.

use crate::archive;
use crate::github::{Problem, Wanted};
use crate::system::PackageManager;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Flathub, where the runtime every Flatpak bundle is built against comes from.
pub const FLATHUB: &str = "https://dl.flathub.org/repo/flathub.flatpakrepo";
/// The Flatpak's application ID, which is also how it is started.
pub const FLATPAK_ID: &str = "app.noctorium.Noctorium";

/// The mark, as a PNG, for the menu entry an AppImage gets. The window's copy is decoded pixels; this is
/// the file itself, because a desktop reads PNGs and not raw RGBA.
const ICON: &[u8] = include_bytes!("../assets/noctorium-mark-128.png");

/// How one file gets installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Windows: run the installer and let it do its own asking.
    WindowsSetup,
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

    /// The release file this takes.
    pub fn wanted(self) -> Wanted {
        match self {
            Method::WindowsSetup => Wanted::WindowsSetup,
            Method::Apt => Wanted::DebianPackage,
            Method::Dnf | Method::Zypper => Wanted::RpmPackage,
            Method::Pacman => Wanted::ArchPackage,
            Method::AppImage => Wanted::AppImage,
            Method::Flatpak => Wanted::Flatpak,
            Method::CliWindows => Wanted::CliWindows,
            Method::CliLinux => Wanted::CliLinux,
        }
    }

    /// Whether it writes where only root can. Everything else is installed for this user alone.
    pub fn needs_root(self) -> bool {
        matches!(
            self,
            Method::Apt | Method::Dnf | Method::Zypper | Method::Pacman
        )
    }

    /// The format, as a menu shows it.
    pub fn describe(self) -> &'static str {
        match self {
            Method::WindowsSetup => "Windows installer (.exe)",
            Method::Apt => "Debian package (.deb)",
            Method::Dnf | Method::Zypper => "RPM package (.rpm)",
            Method::Pacman => "Arch Linux package (.pkg.tar.zst)",
            Method::AppImage => "AppImage",
            Method::Flatpak => "Flatpak",
            Method::CliWindows => "folder of its own, on your PATH",
            Method::CliLinux => "folder of its own, linked into ~/.local/bin",
        }
    }

    /// What it means in practice, in a few words, for the line beside it in a menu.
    pub fn explain(self) -> &'static str {
        match self {
            Method::WindowsSetup => "the usual installer, which asks its own questions",
            Method::Apt => "through apt, which fetches what it needs",
            Method::Dnf => "through dnf, which fetches what it needs",
            Method::Zypper => "through zypper, which fetches what it needs",
            Method::Pacman => "through pacman, with mpv as a dependency",
            Method::AppImage => "one file in ~/Applications, no password, uses this machine's mpv",
            Method::Flatpak => "sandboxed, for you alone, with its own mpv",
            Method::CliWindows | Method::CliLinux => "for you alone, no administrator needed",
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
        Places {
            home,
            data,
            programs,
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

    /// Where the Noctorium CLI is unpacked to.
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
    /// PATH for every user.
    pub fn user_bin(&self) -> Result<PathBuf, Problem> {
        Ok(self.need(&self.home, "HOME")?.join(".local").join("bin"))
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
    /// An AppImage copied into place, made executable and given a menu entry.
    PlaceAppImage {
        from: PathBuf,
        to: PathBuf,
        entry: PathBuf,
        icon: PathBuf,
    },
    /// The Noctorium CLI unpacked, and then linked from [link] on Linux or put on the PATH on Windows.
    UnpackCli {
        archive: PathBuf,
        into: PathBuf,
        link: Option<PathBuf>,
    },
    /// The Flatpak taken out if it is installed already, keeping its data, so a bundle can go in.
    ClearFlatpak { id: String },
}

/// What installing [file] with [method] takes, as steps that can be shown before any of them is run.
///
/// Built rather than run, so the caller can print it before anything happens -- an installer that asks
/// for a password should have said what it is about to do first -- and so it can be tested without
/// installing anything.
pub fn actions_for(
    method: Method,
    file: &Path,
    escalate_with: Option<&str>,
    places: &Places,
) -> Result<Vec<Action>, Problem> {
    let path = file.to_string_lossy().to_string();
    let actions = match method {
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
        }],
        Method::CliLinux => vec![Action::UnpackCli {
            archive: file.to_path_buf(),
            into: places.cli_folder(false)?,
            link: Some(places.user_bin()?.join("noctorium")),
        }],
    };
    Ok(actions)
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
                into, link: None, ..
            } => format!(
                "unpack it into {} and add that folder to your PATH",
                places.show(into)
            ),
            Action::ClearFlatpak { id } => format!(
                "flatpak uninstall --user -y {id}, if an older one is installed (its settings stay)"
            ),
        }
    }

    /// Does it.
    pub fn perform(&self) -> Result<(), Problem> {
        match self {
            Action::Run { program, args } => run(program, args),
            Action::PlaceAppImage {
                from,
                to,
                entry,
                icon,
            } => place_appimage(from, to, entry, icon),
            Action::UnpackCli {
                archive,
                into,
                link,
            } => install_cli(archive, into, link.as_deref()),
            Action::ClearFlatpak { id } => clear_flatpak(id),
        }
    }
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

/// Unpacks the Noctorium CLI and makes `noctorium` something a new terminal can run.
fn install_cli(archive: &Path, into: &Path, link: Option<&Path>) -> Result<(), Problem> {
    archive::install_folder(archive, into)?;
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
        Some(link) => link_launcher(&launcher, link),
        None => {
            // The folder the launcher is in, which is the install folder itself for the published
            // archive and its bin folder for one laid out the other way.
            let folder = launcher.parent().unwrap_or(into);
            add_to_user_path(folder)
        }
    }
}

#[cfg(unix)]
fn link_launcher(launcher: &Path, link: &Path) -> Result<(), Problem> {
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
                "{} is already there and is not a link to the Noctorium CLI, so it has been left \
                 alone. The CLI is unpacked in {}; move that file aside and run this again, or run the \
                 CLI from where it is.",
                link.display(),
                launcher.display()
            )));
        }
        std::fs::remove_file(link).map_err(|e| local("replace", link, e))?;
    }
    std::os::unix::fs::symlink(launcher, link).map_err(|e| local("make the link", link, e))
}

#[cfg(not(unix))]
fn link_launcher(_: &Path, _: &Path) -> Result<(), Problem> {
    Ok(())
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
        Method::WindowsSetup => {
            "Start it from the Start menu, or from the shortcut on the desktop.".into()
        }
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
        Method::CliWindows => {
            "Open a new terminal -- one already open still has the old PATH -- and run noctorium."
                .into()
        }
        Method::CliLinux if user_bin_on_path => "Run noctorium in a terminal.".into(),
        Method::CliLinux => {
            "~/.local/bin is not on your PATH yet, so run it as ~/.local/bin/noctorium, \
             or add export PATH=\"$HOME/.local/bin:$PATH\" to ~/.profile and open a new terminal."
                .into()
        }
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
        only_command(actions_for(method, Path::new(file), sudo, &places()).unwrap())
    }

    #[test]
    fn windows_runs_the_installer_itself() {
        let (program, args) = command(Method::WindowsSetup, r"C:\Temp\Noctorium-setup.exe", None);
        assert_eq!(program, r"C:\Temp\Noctorium-setup.exe");
        assert!(args.is_empty(), "the installer asks its own questions");
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
        )
        .unwrap();
        assert_eq!(
            linux,
            vec![Action::UnpackCli {
                archive: PathBuf::from("/tmp/noctorium-cli-1.0.0-linux-x64.tar.gz"),
                into: PathBuf::from("/home/sam/.local/share/noctorium-cli"),
                link: Some(PathBuf::from("/home/sam/.local/bin/noctorium")),
            }]
        );

        let windows = actions_for(
            Method::CliWindows,
            Path::new(r"C:\Temp\noctorium-cli-1.0.0-windows-x64.zip"),
            None,
            &places(),
        )
        .unwrap();
        assert_eq!(
            windows,
            vec![Action::UnpackCli {
                archive: PathBuf::from(r"C:\Temp\noctorium-cli-1.0.0-windows-x64.zip"),
                into: PathBuf::from(r"C:\Users\Sam\AppData\Local\Programs").join("Noctorium CLI"),
                link: None,
            }]
        );
    }

    #[test]
    fn a_per_user_format_without_a_home_says_so() {
        let homeless = Places::default();
        assert!(matches!(
            actions_for(Method::AppImage, Path::new("/tmp/x"), None, &homeless),
            Err(Problem::Local(_))
        ));
        assert!(actions_for(Method::Apt, Path::new("/tmp/x.deb"), None, &homeless).is_ok());
    }

    #[test]
    fn only_the_packages_need_root() {
        for method in [Method::Apt, Method::Dnf, Method::Zypper, Method::Pacman] {
            assert!(method.needs_root(), "{method:?}");
        }
        for method in [
            Method::WindowsSetup,
            Method::AppImage,
            Method::Flatpak,
            Method::CliLinux,
            Method::CliWindows,
        ] {
            assert!(!method.needs_root(), "{method:?}");
        }
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
    }
}
