//! What this machine is, found by looking at it and changing nothing.
//!
//! On Linux the answer that matters is which package manager owns the system, and the distribution says
//! so itself in `/etc/os-release`: `ID` names it and `ID_LIKE` names what it is built on, so Linux Mint
//! reads as Ubuntu and Debian, and EndeavourOS as Arch, without this having to know every derivative
//! there is. The tool still has to be on PATH -- Fedora Silverblue is Fedora with no dnf -- and a machine
//! that says nothing recognisable is asked tool by tool, which is how this program has always worked.
//!
//! On a Mac the answer that matters is the processor, which is not always what this program is running
//! as: see [mac_hardware].

use crate::github::Arch;
use std::path::Path;

/// The operating system, as far as an installer cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    MacOs,
    /// Anything else. Named so the message can say what it is.
    Other(&'static str),
}

/// The few fields of `/etc/os-release` an installer uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OsRelease {
    /// `ubuntu`, `fedora`, `opensuse-tumbleweed`, `arch`.
    pub id: String,
    /// What it is built on, nearest first: `ubuntu debian` for Mint.
    pub id_like: Vec<String>,
    /// `Ubuntu 24.04.1 LTS`, for showing.
    pub pretty_name: Option<String>,
    pub name: Option<String>,
}

impl OsRelease {
    /// The family that installs packages the way this distribution does, from its own ID first and then
    /// from what it says it is like.
    pub fn family(&self) -> Option<Family> {
        std::iter::once(&self.id)
            .chain(self.id_like.iter())
            .find_map(|word| Family::from_id(word))
    }

    /// The distribution's own name for itself.
    pub fn describe(&self) -> String {
        self.pretty_name
            .clone()
            .or_else(|| self.name.clone())
            .unwrap_or_else(|| {
                if self.id.is_empty() {
                    "Linux".to_string()
                } else {
                    self.id.clone()
                }
            })
    }
}

/// Reads `/etc/os-release`, which is a shell-style list of `KEY=value` lines.
///
/// Values may be bare, in double quotes with backslash escapes, or in single quotes; comments and blank
/// lines are skipped, and so is anything that is not a line of that shape -- one odd line should not cost
/// the whole answer.
pub fn parse_os_release(text: &str) -> OsRelease {
    let mut release = OsRelease::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = unquote(value.trim());
        match key.trim() {
            "ID" => release.id = value.to_ascii_lowercase(),
            "ID_LIKE" => {
                release.id_like = value
                    .split_whitespace()
                    .map(|word| word.to_ascii_lowercase())
                    .collect()
            }
            "PRETTY_NAME" if !value.is_empty() => release.pretty_name = Some(value),
            "NAME" if !value.is_empty() => release.name = Some(value),
            _ => {}
        }
    }
    release
}

fn unquote(value: &str) -> String {
    if let Some(inner) = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        // The escapes the format allows inside double quotes: \" \\ \$ \`.
        let mut out = String::with_capacity(inner.len());
        let mut characters = inner.chars();
        while let Some(c) = characters.next() {
            if c == '\\' {
                if let Some(next) = characters.next() {
                    out.push(next);
                }
            } else {
                out.push(c);
            }
        }
        return out;
    }
    if let Some(inner) = value
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return inner.to_string();
    }
    value.to_string()
}

/// A family of distributions that share a package format and a package manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Debian, Ubuntu, Mint, Pop!_OS, elementary, Zorin, Raspberry Pi OS: `.deb` through apt.
    Debian,
    /// Fedora, RHEL, CentOS Stream, Rocky, Alma, Nobara: `.rpm` through dnf.
    Fedora,
    /// openSUSE Leap and Tumbleweed, SLES: `.rpm` through zypper.
    Suse,
    /// Arch, Manjaro, EndeavourOS, Garuda, CachyOS: `.pkg.tar.zst` through pacman.
    Arch,
}

impl Family {
    /// Which family a single `ID` or `ID_LIKE` word belongs to.
    pub fn from_id(word: &str) -> Option<Family> {
        let word = word.to_ascii_lowercase();
        let family = match word.as_str() {
            "debian" | "ubuntu" | "linuxmint" | "raspbian" | "pop" | "elementary" | "zorin"
            | "neon" | "kali" | "devuan" => Family::Debian,
            "fedora" | "rhel" | "centos" | "rocky" | "almalinux" | "ol" | "nobara" | "amzn" => {
                Family::Fedora
            }
            "suse" | "opensuse" | "sles" | "sled" => Family::Suse,
            "arch" | "manjaro" | "endeavouros" | "garuda" | "artix" | "cachyos" => Family::Arch,
            // openSUSE's IDs carry the edition: opensuse-tumbleweed, opensuse-leap, opensuse-microos.
            other if other.starts_with("opensuse") => Family::Suse,
            _ => return None,
        };
        Some(family)
    }

    pub fn package_manager(self) -> PackageManager {
        match self {
            Family::Debian => PackageManager::Apt,
            Family::Fedora => PackageManager::Dnf,
            Family::Suse => PackageManager::Zypper,
            Family::Arch => PackageManager::Pacman,
        }
    }
}

/// The tool that owns the system's software.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Apt,
    Dnf,
    Zypper,
    Pacman,
}

impl PackageManager {
    /// The program itself. `apt-get` rather than `apt`, whose own manual says not to script it.
    pub fn tool(self) -> &'static str {
        match self {
            PackageManager::Apt => "apt-get",
            PackageManager::Dnf => "dnf",
            PackageManager::Zypper => "zypper",
            PackageManager::Pacman => "pacman",
        }
    }

    /// As a person says it, which for apt is apt.
    pub fn describe(self) -> &'static str {
        match self {
            PackageManager::Apt => "apt",
            other => other.tool(),
        }
    }

    /// The line that installs something by name, for telling somebody how to get mpv.
    pub fn install_line(self, package: &str) -> String {
        match self {
            PackageManager::Apt => format!("sudo apt install {package}"),
            PackageManager::Dnf => format!("sudo dnf install {package}"),
            PackageManager::Zypper => format!("sudo zypper install {package}"),
            PackageManager::Pacman => format!("sudo pacman -S {package}"),
        }
    }
}

/// Everything about this machine the plan depends on.
#[derive(Debug, Clone)]
pub struct System {
    pub os: Os,
    /// What this machine runs, or nothing for a processor nothing is built for. On a Mac, the processor
    /// itself, even when this program is running translated by Rosetta.
    pub arch: Option<Arch>,
    /// The architecture as Rust names it, for saying what this machine is when it is none of the above.
    pub arch_name: &'static str,
    /// What `/etc/os-release` says, on Linux.
    pub release: Option<OsRelease>,
    /// The version of macOS, `14.5`, on a Mac.
    pub mac_version: Option<String>,
    pub family: Option<Family>,
    /// The family's package manager, if its tool is actually here.
    pub package_manager: Option<PackageManager>,
    pub flatpak: bool,
    pub mpv: bool,
    pub root: bool,
    /// Who ran sudo, when this is running under it: per-user installs would then land in root's home.
    pub sudo_user: Option<String>,
}

impl System {
    /// Looks.
    pub fn detect() -> System {
        let os = if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Other(std::env::consts::OS)
        };
        let arch = if os == Os::MacOs {
            mac_hardware(Arch::this_machine(), sysctl_number)
        } else {
            Arch::this_machine()
        };
        let release = (os == Os::Linux)
            .then(|| std::fs::read_to_string("/etc/os-release").ok())
            .flatten()
            // The file's documented fallback, for the rare system that has only that.
            .or_else(|| {
                (os == Os::Linux)
                    .then(|| std::fs::read_to_string("/usr/lib/os-release").ok())
                    .flatten()
            })
            .map(|text| parse_os_release(&text));
        let family = release.as_ref().and_then(OsRelease::family);
        let package_manager = if os == Os::Linux {
            choose_package_manager(family, on_path)
        } else {
            None
        };
        let root = is_root();
        System {
            os,
            arch,
            arch_name: std::env::consts::ARCH,
            release,
            // Since 10.13.4. Asked of the kernel rather than of sw_vers, which is a program to start and
            // read for one short line.
            mac_version: (os == Os::MacOs)
                .then(|| sysctl_text("kern.osproductversion"))
                .flatten(),
            family,
            package_manager,
            flatpak: os == Os::Linux && on_path("flatpak"),
            mpv: on_path(if cfg!(windows) { "mpv.exe" } else { "mpv" }),
            root,
            sudo_user: std::env::var("SUDO_USER")
                .ok()
                .filter(|user| root && !user.is_empty() && user != "root"),
        }
    }

    /// One line saying what this is: `Ubuntu 24.04.1 LTS`, `macOS 14.5`, `Windows`.
    pub fn describe(&self) -> String {
        match self.os {
            Os::Windows => "Windows".to_string(),
            Os::Linux => self
                .release
                .as_ref()
                .map(OsRelease::describe)
                .unwrap_or_else(|| "Linux".to_string()),
            Os::MacOs => match &self.mac_version {
                Some(version) => format!("macOS {version}"),
                None => "macOS".to_string(),
            },
            Os::Other(name) => name.to_string(),
        }
    }
}

/// The processor a Mac really has, given what this program is running as and a way of asking the kernel
/// for a number by name.
///
/// The terminal installer is one universal binary, and macOS chooses which half of it runs: the ARM half
/// on Apple silicon -- unless whatever started it is itself running under Rosetta, a Terminal set to open
/// using Rosetta or an x86-64 shell, and then the Intel half runs, translated, and everything this
/// program can ask about itself says x86-64. Taking that at its word would install the Intel Noctorium on
/// a machine that runs the ARM one natively, translated for as long as it is installed.
///
/// So the kernel is asked. `hw.optional.arm64` is 1 on Apple silicon however the asking process runs, and
/// `sysctl.proc_translated` is 1 for a process under Rosetta, which only Apple silicon has; either says
/// ARM. On an Intel Mac neither is 1 -- the first is 0 or not there at all, depending on the version of
/// macOS -- and the answer is the one this program was built as.
pub fn mac_hardware(running_as: Option<Arch>, ask: impl Fn(&str) -> Option<i64>) -> Option<Arch> {
    let says_arm = |name: &str| ask(name) == Some(1);
    if says_arm("hw.optional.arm64") || says_arm("sysctl.proc_translated") {
        return Some(Arch::Aarch64);
    }
    running_as
}

/// The raw value the kernel keeps under [name], or nothing if it keeps none by that name.
#[cfg(target_os = "macos")]
fn sysctl(name: &str) -> Option<Vec<u8>> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut size: libc::size_t = 0;
    // SAFETY: with no buffer, sysctlbyname only writes the value's length into `size`, which is live.
    let asked = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if asked != 0 || size == 0 {
        return None;
    }
    let mut value = vec![0u8; size];
    // SAFETY: the buffer is `size` bytes long, which is what the call is told it may write.
    let read = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            value.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    value.truncate(size);
    Some(value)
}

/// A number the kernel keeps, such as `hw.optional.arm64`. The ones asked for here are 32-bit integers,
/// and a 64-bit one is read too rather than misread.
#[cfg(target_os = "macos")]
fn sysctl_number(name: &str) -> Option<i64> {
    let value = sysctl(name)?;
    match value.len() {
        4 => Some(i32::from_ne_bytes(<[u8; 4]>::try_from(&value[..]).ok()?).into()),
        8 => Some(i64::from_ne_bytes(<[u8; 8]>::try_from(&value[..]).ok()?)),
        _ => None,
    }
}

/// A string the kernel keeps, such as `kern.osproductversion`, without the NUL it ends with.
#[cfg(target_os = "macos")]
fn sysctl_text(name: &str) -> Option<String> {
    let value = sysctl(name)?;
    let text = String::from_utf8_lossy(&value);
    let text = text.trim_end_matches('\0').trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(not(target_os = "macos"))]
fn sysctl_number(_: &str) -> Option<i64> {
    None
}

#[cfg(not(target_os = "macos"))]
fn sysctl_text(_: &str) -> Option<String> {
    None
}

/// Whether this user can create things in [folder], asked of the kernel, which is what will decide.
///
/// Used on a Mac for /Applications, which every administrator can write to and a standard account
/// cannot. Working it out from the permission bits instead would mean knowing every group this user is
/// in, and would still miss an access control list.
pub fn writable(folder: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(path) = std::ffi::CString::new(folder.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: access only reads the path, which is NUL-terminated and outlives the call.
        unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = folder;
        false
    }
}

/// The package manager to use, given the family the distribution claims and a way of asking whether a
/// tool is installed.
///
/// A known family gets its own tool or nothing: an openSUSE machine that happens to have apt installed
/// for some reason is still an openSUSE machine. A family nobody recognises is asked tool by tool, apt
/// first because a Debian derivative too obscure to say `ID_LIKE=debian` is likelier than the rest.
pub fn choose_package_manager(
    family: Option<Family>,
    installed: impl Fn(&str) -> bool,
) -> Option<PackageManager> {
    match family {
        Some(family) => {
            let manager = family.package_manager();
            installed(manager.tool()).then_some(manager)
        }
        None => [
            PackageManager::Apt,
            PackageManager::Dnf,
            PackageManager::Zypper,
            PackageManager::Pacman,
        ]
        .into_iter()
        .find(|manager| installed(manager.tool())),
    }
}

/// Whether a program is on PATH.
pub fn on_path(tool: &str) -> bool {
    // `which` is not everywhere, but PATH is: this asks the same question without depending on a program
    // to answer it.
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join(tool).is_file()))
        .unwrap_or(false)
}

/// Whether `directory` is one of the entries of a Unix PATH, read the way the shell reads it.
///
/// Split on the colon by hand rather than with `split_paths`, which splits on whatever this machine
/// uses -- and the question is only ever asked of a Linux or a Mac PATH, but the tests run everywhere.
pub fn on_search_path(path: &str, directory: &Path) -> bool {
    let wanted = directory.to_string_lossy();
    let wanted = wanted.trim_end_matches('/');
    path.split(':')
        .any(|entry| !entry.is_empty() && entry.trim_end_matches('/') == wanted)
}

/// Whether this process can already do anything, asked of the kernel rather than of the environment.
pub fn is_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Who is going to be asked for a password, which decides how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asking {
    /// Somebody looking at a window: pkexec puts up the desktop's own password dialog.
    Window,
    /// Somebody at a terminal. Whether they really are is the caller's to say.
    Terminal { interactive: bool },
}

/// How this machine asks for administrator rights, or nothing if it is already running with them or has
/// no way to.
pub fn escalation(asking: Asking) -> Option<&'static str> {
    choose_escalation(is_root(), asking, on_path)
}

/// The decision itself, with what it depends on passed in so it can be tested.
///
/// A window wants pkexec first: it is the desktop's own password dialog, which is what somebody double
/// clicking an installer expects. A terminal wants sudo first: it asks right there in the terminal, and
/// pkexec only works when a polkit agent is running to ask on its behalf -- which over ssh, or in a bare
/// console, it is not. A terminal nobody is typing into gets pkexec back, since sudo would wait for a
/// password that is never coming.
pub fn choose_escalation(
    root: bool,
    asking: Asking,
    installed: impl Fn(&str) -> bool,
) -> Option<&'static str> {
    if root || cfg!(windows) {
        return None;
    }
    let order: [&'static str; 2] = match asking {
        Asking::Window | Asking::Terminal { interactive: false } => ["pkexec", "sudo"],
        Asking::Terminal { interactive: true } => ["sudo", "pkexec"],
    };
    order.into_iter().find(|tool| installed(tool))
}

#[cfg(test)]
mod tests {
    use super::*;

    const UBUNTU: &str = r#"PRETTY_NAME="Ubuntu 24.04.1 LTS"
NAME="Ubuntu"
VERSION_ID="24.04"
VERSION="24.04.1 LTS (Noble Numbat)"
ID=ubuntu
ID_LIKE=debian
HOME_URL="https://www.ubuntu.com/"
"#;

    #[test]
    fn reads_the_fields_of_os_release_in_every_quoting() {
        let release = parse_os_release(UBUNTU);
        assert_eq!(release.id, "ubuntu");
        assert_eq!(release.id_like, vec!["debian"]);
        assert_eq!(release.pretty_name.as_deref(), Some("Ubuntu 24.04.1 LTS"));
        assert_eq!(release.describe(), "Ubuntu 24.04.1 LTS");

        let odd = "# a comment\n\nID='arch'\nPRETTY_NAME=\"Say \\\"hello\\\" \\$HOME\"\nnot a line\nNAME=Arch\n";
        let release = parse_os_release(odd);
        assert_eq!(release.id, "arch");
        assert_eq!(release.pretty_name.as_deref(), Some("Say \"hello\" $HOME"));
        assert_eq!(release.name.as_deref(), Some("Arch"));
    }

    #[test]
    fn a_distribution_with_nothing_to_say_is_still_linux() {
        let release = parse_os_release("");
        assert_eq!(release.family(), None);
        assert_eq!(release.describe(), "Linux");
        assert_eq!(parse_os_release("ID=nixos").describe(), "nixos");
    }

    fn family_of(text: &str) -> Option<Family> {
        parse_os_release(text).family()
    }

    #[test]
    fn every_distribution_finds_its_family() {
        assert_eq!(family_of(UBUNTU), Some(Family::Debian));
        assert_eq!(family_of("ID=debian"), Some(Family::Debian));
        assert_eq!(
            family_of("ID=linuxmint\nID_LIKE=\"ubuntu debian\""),
            Some(Family::Debian)
        );
        assert_eq!(
            family_of("ID=pop\nID_LIKE=\"ubuntu debian\""),
            Some(Family::Debian)
        );
        assert_eq!(family_of("ID=fedora"), Some(Family::Fedora));
        assert_eq!(
            family_of("ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\""),
            Some(Family::Fedora)
        );
        assert_eq!(
            family_of("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\""),
            Some(Family::Suse)
        );
        assert_eq!(
            family_of("ID=\"opensuse-leap\"\nID_LIKE=\"suse opensuse\""),
            Some(Family::Suse)
        );
        assert_eq!(family_of("ID=arch"), Some(Family::Arch));
        assert_eq!(family_of("ID=manjaro\nID_LIKE=arch"), Some(Family::Arch));
        assert_eq!(
            family_of("ID=endeavouros\nID_LIKE=arch"),
            Some(Family::Arch)
        );
        assert_eq!(family_of("ID=nixos"), None);
        assert_eq!(family_of("ID=gentoo"), None);
    }

    /// A derivative is read as what it says it is built on, nearest first.
    #[test]
    fn the_distribution_itself_outranks_what_it_is_like() {
        // Nobara is Fedora; if it ever listed something else after, Fedora must still win.
        assert_eq!(
            family_of("ID=nobara\nID_LIKE=\"fedora debian\""),
            Some(Family::Fedora)
        );
        // A distribution nobody here knows by name is read through ID_LIKE.
        assert_eq!(
            family_of("ID=somethingnew\nID_LIKE=\"ubuntu debian\""),
            Some(Family::Debian)
        );
    }

    #[test]
    fn each_family_installs_with_its_own_tool() {
        assert_eq!(Family::Debian.package_manager().tool(), "apt-get");
        assert_eq!(Family::Fedora.package_manager().tool(), "dnf");
        assert_eq!(Family::Suse.package_manager().tool(), "zypper");
        assert_eq!(Family::Arch.package_manager().tool(), "pacman");
    }

    #[test]
    fn a_known_family_without_its_tool_has_no_package_manager() {
        // Fedora Silverblue: Fedora, but rpm-ostree rather than dnf. Not apt either, even if it is there.
        let only = |present: &'static [&'static str]| move |tool: &str| present.contains(&tool);
        assert_eq!(
            choose_package_manager(Some(Family::Fedora), only(&["apt-get"])),
            None
        );
        assert_eq!(
            choose_package_manager(Some(Family::Fedora), only(&["dnf"])),
            Some(PackageManager::Dnf)
        );
        // An unknown family is asked tool by tool, apt first.
        assert_eq!(
            choose_package_manager(None, only(&["pacman", "apt-get"])),
            Some(PackageManager::Apt)
        );
        assert_eq!(
            choose_package_manager(None, only(&["zypper"])),
            Some(PackageManager::Zypper)
        );
        assert_eq!(choose_package_manager(None, only(&[])), None);
    }

    #[test]
    fn a_window_prefers_pkexec_and_a_terminal_prefers_sudo() {
        if cfg!(windows) {
            // Windows installers ask for their own rights, so there is never anything to put in front.
            assert_eq!(choose_escalation(false, Asking::Window, |_| true), None);
            return;
        }
        let both = |_: &str| true;
        assert_eq!(
            choose_escalation(false, Asking::Window, both),
            Some("pkexec")
        );
        assert_eq!(
            choose_escalation(false, Asking::Terminal { interactive: true }, both),
            Some("sudo")
        );
        assert_eq!(
            choose_escalation(false, Asking::Terminal { interactive: false }, both),
            Some("pkexec"),
            "nobody is there to type a password into sudo"
        );
        let sudo_only = |tool: &str| tool == "sudo";
        assert_eq!(
            choose_escalation(false, Asking::Window, sudo_only),
            Some("sudo")
        );
        assert_eq!(
            choose_escalation(true, Asking::Terminal { interactive: true }, both),
            None,
            "root needs nothing"
        );
        assert_eq!(choose_escalation(false, Asking::Window, |_| false), None);
    }

    #[test]
    fn a_directory_is_found_on_a_path_however_it_is_written() {
        let home_bin = Path::new("/home/sam/.local/bin");
        assert!(on_search_path("/usr/bin:/home/sam/.local/bin", home_bin));
        assert!(on_search_path("/home/sam/.local/bin/:/usr/bin", home_bin));
        assert!(!on_search_path("/usr/bin:/bin", home_bin));
        assert!(!on_search_path("", home_bin));

        // A Mac's PATH as a new Terminal has it, which ~/.local/bin is not on until somebody puts it there.
        let mac_bin = Path::new("/Users/sam/.local/bin");
        let mac = "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/System/Cryptexes/App/usr/bin:/usr/bin:/bin:/usr/sbin:/sbin";
        assert!(!on_search_path(mac, mac_bin));
        assert!(on_search_path(
            &format!("/Users/sam/.local/bin:{mac}"),
            mac_bin
        ));
    }

    /// The kernel's answers on each kind of Mac, by name, with anything not listed absent.
    fn kernel(answers: &'static [(&'static str, i64)]) -> impl Fn(&str) -> Option<i64> {
        move |name| {
            answers
                .iter()
                .find(|(asked, _)| *asked == name)
                .map(|(_, value)| *value)
        }
    }

    #[test]
    fn apple_silicon_is_arm_even_when_this_program_runs_under_rosetta() {
        let native = kernel(&[("hw.optional.arm64", 1), ("sysctl.proc_translated", 0)]);
        assert_eq!(
            mac_hardware(Some(Arch::Aarch64), native),
            Some(Arch::Aarch64)
        );

        // The Intel half of the universal binary, started from a Terminal set to open using Rosetta.
        let translated = kernel(&[("hw.optional.arm64", 1), ("sysctl.proc_translated", 1)]);
        assert_eq!(
            mac_hardware(Some(Arch::X86_64), translated),
            Some(Arch::Aarch64),
            "Rosetta must not earn an Apple silicon Mac the Intel build"
        );

        // Either answer is enough on its own.
        let only_translated = kernel(&[("sysctl.proc_translated", 1)]);
        assert_eq!(
            mac_hardware(Some(Arch::X86_64), only_translated),
            Some(Arch::Aarch64)
        );
    }

    #[test]
    fn an_intel_mac_is_what_this_program_was_built_as() {
        // Newer macOS answers 0 on Intel; older has no such name at all.
        let newer = kernel(&[("hw.optional.arm64", 0), ("sysctl.proc_translated", 0)]);
        assert_eq!(mac_hardware(Some(Arch::X86_64), newer), Some(Arch::X86_64));
        let older = kernel(&[]);
        assert_eq!(mac_hardware(Some(Arch::X86_64), older), Some(Arch::X86_64));
    }

    #[test]
    fn a_mac_says_which_version_of_macos_it_is() {
        let mut system = System {
            os: Os::MacOs,
            arch: Some(Arch::Aarch64),
            arch_name: "aarch64",
            release: None,
            mac_version: Some("14.5".into()),
            family: None,
            package_manager: None,
            flatpak: false,
            mpv: false,
            root: false,
            sudo_user: None,
        };
        assert_eq!(system.describe(), "macOS 14.5");
        system.mac_version = None;
        assert_eq!(system.describe(), "macOS");
    }
}
