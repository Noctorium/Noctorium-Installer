//! The install itself, in three parts, with nothing in it that knows whether there is a window.
//!
//! Split where a person would want it split. [`look`] asks GitHub what the release is and looks at the
//! machine, and changes nothing. [`plan`] decides from those what would be installed and how -- which
//! product, which file, which command -- and is pure, so a window can remake it every time somebody
//! picks a different format without asking GitHub again. [`carry_out`] is everything that touches the
//! machine.
//!
//! The terminal shows the plan and asks; the window puts a button between them. Both get the same
//! sequence and the same failures, which is the point of it living here rather than in either.

use crate::fetch;
use crate::github::{self, Arch, Asset, Problem, Release};
use crate::install::{self, Action, Method, Places};
use crate::system::{self, Asking, Os, System};
use std::path::{Path, PathBuf};

/// What can be installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Product {
    /// The desktop application, with a window.
    Desktop,
    /// The terminal player.
    Cli,
}

impl Product {
    pub fn name(self) -> &'static str {
        match self {
            Product::Desktop => "Noctorium",
            Product::Cli => "Noctorium CLI",
        }
    }
}

/// Which of them were asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Products {
    #[default]
    Desktop,
    Cli,
    Both,
}

impl Products {
    pub fn each(self) -> Vec<Product> {
        match self {
            Products::Desktop => vec![Product::Desktop],
            Products::Cli => vec![Product::Cli],
            Products::Both => vec![Product::Desktop, Product::Cli],
        }
    }

    pub fn parse(word: &str) -> Option<Products> {
        match word.to_ascii_lowercase().as_str() {
            "desktop" => Some(Products::Desktop),
            "cli" => Some(Products::Cli),
            "both" => Some(Products::Both),
            _ => None,
        }
    }
}

/// How the desktop application is to be installed on Linux. Windows and a Mac have one way each, which
/// is `auto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// The distribution's own package when it is one this knows, and otherwise the AppImage.
    #[default]
    Auto,
    Deb,
    Rpm,
    Arch,
    AppImage,
    Flatpak,
}

impl Format {
    pub fn parse(word: &str) -> Option<Format> {
        match word.to_ascii_lowercase().as_str() {
            "auto" => Some(Format::Auto),
            "deb" => Some(Format::Deb),
            "rpm" => Some(Format::Rpm),
            "arch" => Some(Format::Arch),
            "appimage" => Some(Format::AppImage),
            "flatpak" => Some(Format::Flatpak),
            _ => None,
        }
    }

    /// The format a method installs, as the flag spells it.
    pub fn of(method: Method) -> Format {
        match method {
            Method::Apt => Format::Deb,
            Method::Dnf | Method::Zypper => Format::Rpm,
            Method::Pacman => Format::Arch,
            Method::AppImage => Format::AppImage,
            Method::Flatpak => Format::Flatpak,
            Method::WindowsSetup
            | Method::MacDiskImage
            | Method::CliWindows
            | Method::CliLinux
            | Method::CliMac => Format::Auto,
        }
    }
}

/// What was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub products: Products,
    pub format: Format,
    /// Who will be asked for a password, which decides between pkexec and sudo.
    pub asking: Asking,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            products: Products::Desktop,
            format: Format::Auto,
            asking: Asking::Window,
        }
    }
}

/// Everything asked of GitHub and of the machine, held so a plan can be made and remade from it.
#[derive(Debug, Clone)]
pub struct Found {
    pub release: Release,
    /// The release's `SHA256SUMS.txt`, or nothing if it has none -- which [`plan`] refuses, but a listing
    /// of the release's files does not need.
    pub checksums: Option<String>,
    /// Whether this is the latest release rather than one asked for by version.
    pub latest: bool,
    pub system: System,
    pub places: Places,
}

impl Found {
    /// The version as somebody reads it, without the tag's leading v.
    pub fn version(&self) -> &str {
        self.release.version()
    }
}

/// Asks GitHub for a release -- the latest, or the one tagged `v<version>` -- and looks at this machine.
pub fn look(version: Option<&str>) -> Result<Found, Problem> {
    let tag = version.map(|v| format!("v{}", v.trim_start_matches('v')));
    let release = Release::from_json(&fetch::release(tag.as_deref())?)?;
    // Fetched here rather than after the download, so a release whose checksums cannot be read is found
    // out about before three hundred megabytes have been pulled down for nothing.
    let checksums = match release.checksums() {
        Some(asset) => Some(fetch::text(&asset.url)?),
        None => None,
    };
    Ok(Found {
        release,
        checksums,
        latest: tag.is_none(),
        system: System::detect(),
        places: Places::here(),
    })
}

/// One way the desktop application could be installed here.
#[derive(Debug, Clone)]
pub struct Offer {
    pub method: Method,
    /// The file the release has for it, if it has one.
    pub asset: Option<Asset>,
    /// Why it cannot be used, when it cannot: not in this release, or not possible on this machine.
    pub unavailable: Option<String>,
    /// Whether this is what `auto` would pick.
    pub recommended: bool,
}

/// The ways the desktop application can be installed on this machine, best first.
///
/// On Windows there is one, and on a Mac there is one, the disk image for its processor. On Linux there
/// is the distribution's own package when its package manager is here, then the AppImage, which runs
/// anywhere, then the Flatpak, which needs Flatpak. Each says whether this release carries it, so a menu
/// can show what there is rather than offering something that will fail after the download.
pub fn offers(found: &Found) -> Vec<Offer> {
    let system = &found.system;
    let Some(arch) = system.arch else {
        return vec![];
    };
    let mut methods: Vec<(Method, Option<String>)> = Vec::new();
    match system.os {
        Os::Windows => methods.push((Method::WindowsSetup, None)),
        Os::MacOs => methods.push((Method::MacDiskImage, None)),
        Os::Linux => {
            if let Some(manager) = system.package_manager {
                methods.push((Method::for_package_manager(manager), None));
            }
            methods.push((Method::AppImage, None));
            let flatpak = (!system.flatpak).then(|| {
                "Flatpak is not installed here. Install it with your package manager first."
                    .to_string()
            });
            methods.push((Method::Flatpak, flatpak));
        }
        Os::Other(_) => {}
    }

    let mut offers: Vec<Offer> = methods
        .into_iter()
        .map(|(method, not_here)| {
            let asset = found.release.asset_for(method.wanted(), arch).cloned();
            let unavailable = not_here.or_else(|| {
                asset
                    .is_none()
                    .then(|| format!("{} has no {}.", found.release.tag, method.describe()))
            });
            Offer {
                method,
                asset,
                unavailable,
                recommended: false,
            }
        })
        .collect();
    if let Some(best) = offers.iter_mut().find(|offer| offer.unavailable.is_none()) {
        best.recommended = true;
    }
    offers
}

/// What is about to happen, settled before anything is downloaded.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The release's tag, as published: `v0.4.1`.
    pub tag: String,
    pub latest: bool,
    pub items: Vec<Item>,
    pub places: Places,
    /// Things worth saying before anything happens: a fallback that was taken, a warning about root.
    pub notes: Vec<String>,
}

/// One product, the file it comes in, and how it goes in.
#[derive(Debug, Clone)]
pub struct Item {
    pub product: Product,
    pub asset: Asset,
    /// What that file must hash to, taken from the release's own `SHA256SUMS.txt`.
    pub published: String,
    pub method: Method,
    /// How this machine will be asked for administrator rights, if it has to be.
    pub escalation: Option<&'static str>,
    /// Where it is downloaded to.
    pub file: PathBuf,
    /// What installing it takes, which is what gets shown before it is done.
    pub actions: Vec<Action>,
    /// What to say once it is in.
    pub start: String,
}

impl Plan {
    /// The version as somebody reads it, without the tag's leading v.
    pub fn version(&self) -> &str {
        self.tag.strip_prefix('v').unwrap_or(&self.tag)
    }

    pub fn megabytes(&self) -> f64 {
        self.items.iter().map(|i| i.asset.megabytes()).sum()
    }

    /// Whether going ahead will put a password prompt in front of somebody.
    pub fn needs_password(&self) -> bool {
        self.items.iter().any(|i| i.escalation.is_some())
    }
}

/// Where downloads go: a folder of this program's own, so a half-finished download is never left in the
/// middle of somebody's Downloads folder.
pub fn download_folder() -> PathBuf {
    std::env::temp_dir().join("noctorium-installer")
}

/// Works out what would be installed, without installing it or writing anything to disk.
pub fn plan(found: &Found, options: Options) -> Result<Plan, Problem> {
    let system = &found.system;
    if let Os::Other(name) = system.os {
        return Err(Problem::Local(format!(
            "This installer is for Windows, macOS and Linux, and this is {name}. The releases page has \
             everything Noctorium is published for."
        )));
    }
    let arch = system.arch.ok_or_else(|| {
        Problem::Local(format!(
            "Noctorium is published for x86-64, and for ARM64 on a Mac, and this machine is {}. There \
             is nothing in the release it could run.",
            system.arch_name
        ))
    })?;
    let listing = found.checksums.as_deref().ok_or(Problem::NoChecksums)?;

    let mut notes = Vec::new();
    let mut items = Vec::new();
    for product in options.products.each() {
        let (method, asset) = match product {
            Product::Desktop => desktop_method(found, options.format, &mut notes)?,
            Product::Cli => cli_method(found, arch)?,
        };
        let published = github::published_checksum(listing, &asset.name).ok_or_else(|| {
            Problem::Missing(format!(
                "{} is in the release but not in its SHA256SUMS.txt, so it cannot be checked. Refusing \
                 to install something unverified.",
                asset.name
            ))
        })?;

        let escalation = if method.needs_root() && !system.root {
            Some(system::escalation(options.asking).ok_or_else(|| {
                Problem::Local(format!(
                    "Installing a {} needs administrator rights, and this machine has neither sudo nor \
                     pkexec to ask for them. Run this as root, or choose the AppImage, which needs no \
                     password.",
                    method.describe()
                ))
            })?)
        } else {
            None
        };

        // Not said of the Windows installer, which asks its own questions, nor of a Mac application, which
        // root puts in the /Applications everybody shares rather than in a home folder of its own.
        if !method.needs_root() && !matches!(method, Method::WindowsSetup | Method::MacDiskImage) {
            if let Some(user) = &system.sudo_user {
                notes.push(format!(
                    "This is running as root through sudo, so {} will be installed for root and not \
                     for {user}. Run it without sudo to install it for yourself.",
                    product.name()
                ));
            }
        }
        if product == Product::Desktop
            && system.os == Os::Linux
            && !system.mpv
            && matches!(
                method,
                Method::Apt | Method::Dnf | Method::Zypper | Method::AppImage
            )
        {
            // The Flatpak carries its own mpv and the Arch package depends on it; everything else plays
            // through the distribution's, which may not be here yet.
            let line = system
                .package_manager
                .map(|m| m.install_line("mpv"))
                .unwrap_or_else(|| "your package manager".into());
            notes.push(format!(
                "mpv is not installed, and Noctorium plays through it. Install it with: {line}"
            ));
        }

        if method == Method::MacDiskImage {
            if let Some(folder) = &found.places.applications {
                if folder != Path::new("/Applications") {
                    notes.push(format!(
                        "This account cannot write to /Applications -- on a Mac that takes an \
                         administrator -- so Noctorium goes into {}, which is yours alone. Spotlight \
                         finds it there just the same.",
                        found.places.show(folder)
                    ));
                }
            }
        }

        let file = download_folder().join(&asset.name);
        let mut actions = install::actions_for(method, &file, escalation, &found.places)?;
        let user_bin_on_path = found
            .places
            .user_bin()
            .map(|bin| system::on_search_path(&std::env::var("PATH").unwrap_or_default(), &bin))
            .unwrap_or(false);
        if method == Method::CliLinux && !user_bin_on_path {
            notes.push(
                "~/.local/bin is not on your PATH, so `noctorium` will not be found by name until it \
                 is. The end of the install says how."
                    .into(),
            );
        }
        // Most Linux distributions put ~/.local/bin on PATH once it exists, and the end of the install says
        // what to do on one that does not. macOS never does, so on a Mac it is done here, and shown in
        // the plan like everything else that changes a file.
        if method == Method::CliMac && !user_bin_on_path {
            actions.push(install::add_user_bin_to_path(&found.places)?);
        }
        let start = install::how_to_start(method, &found.places, user_bin_on_path);
        items.push(Item {
            product,
            asset,
            published,
            method,
            escalation,
            file,
            actions,
            start,
        });
    }

    Ok(Plan {
        tag: found.release.tag.clone(),
        latest: found.latest,
        items,
        places: found.places.clone(),
        notes,
    })
}

/// The method and file for the desktop application, from what was asked and what this machine and
/// release allow.
fn desktop_method(
    found: &Found,
    format: Format,
    notes: &mut Vec<String>,
) -> Result<(Method, Asset), Problem> {
    let offers = offers(found);
    let system = &found.system;
    if format == Format::Auto {
        let Some(best) = offers.iter().find(|o| o.unavailable.is_none()) else {
            let wanted: Vec<String> = offers
                .iter()
                .filter(|o| o.method != Method::Flatpak || system.flatpak)
                .map(|o| o.method.describe().to_string())
                .collect();
            return Err(if wanted.is_empty() {
                Problem::NothingForThisMachine
            } else if system.os == Os::MacOs {
                // An older release is no help here: the Mac is the newest thing Noctorium is built for.
                let arch = system.arch.unwrap_or(Arch::X86_64);
                Problem::Missing(format!(
                    "{} has nothing for a Mac: it carries no {} ({}). Noctorium for macOS is new, and \
                     releases from before it have none; --list shows what this one carries.",
                    found.release.tag,
                    Method::MacDiskImage.describe(),
                    Method::MacDiskImage.wanted().pattern(arch)
                ))
            } else {
                Problem::Missing(format!(
                    "{} has nothing this machine can install: it carries no {}. An older release may \
                     have one -- try --version -- or take a file from the releases page.",
                    found.release.tag,
                    join_or(&wanted)
                ))
            });
        };
        // Said when the obvious choice was passed over, so nobody wonders why their Arch machine was
        // handed an AppImage.
        if let Some(first) = offers.first() {
            if first.method != best.method && first.method.needs_root() {
                notes.push(format!(
                    "{} has no {}, so this is the {} instead.",
                    found.release.tag,
                    first.method.describe(),
                    best.method.describe()
                ));
            }
        }
        if system.os == Os::Linux
            && system.package_manager.is_none()
            && best.method == Method::AppImage
        {
            notes.push(format!(
                "{} is not a distribution this installer has a package for, so this is the AppImage, \
                 which runs anywhere.",
                system.describe()
            ));
        }
        return Ok((
            best.method,
            best.asset.clone().expect("an available offer has a file"),
        ));
    }

    if let Some(name) = only_one_way(system.os) {
        return Err(Problem::Local(format!(
            "There is only one way to install Noctorium on {name}, so --format is for Linux."
        )));
    }
    let method = match format {
        Format::Auto => unreachable!("handled above"),
        Format::AppImage => Method::AppImage,
        Format::Flatpak => Method::Flatpak,
        Format::Deb | Format::Rpm | Format::Arch => {
            native_method(format, system).ok_or_else(|| {
                let (file, tool) = match format {
                    Format::Deb => (".deb", "apt"),
                    Format::Rpm => (".rpm", "dnf or zypper"),
                    _ => (".pkg.tar.zst", "pacman"),
                };
                Problem::Local(format!(
                    "A {file} needs {tool}, and this machine has none. --format appimage runs \
                     anywhere."
                ))
            })?
        }
    };
    let offer = offers
        .iter()
        .find(|o| o.method == method)
        .cloned()
        .unwrap_or(Offer {
            method,
            asset: found
                .release
                .asset_for(method.wanted(), system.arch.unwrap_or(Arch::X86_64))
                .cloned(),
            unavailable: None,
            recommended: false,
        });
    if method == Method::Flatpak && !system.flatpak {
        return Err(Problem::Local(
            "Flatpak is not installed on this machine. Install it with your package manager first, or \
             choose another format."
                .into(),
        ));
    }
    match offer.asset {
        Some(asset) => Ok((method, asset)),
        None => Err(Problem::Missing(format!(
            "{} has no {} ({}). It is new, and older releases were not built with one; --list shows \
             what this one carries.",
            found.release.tag,
            method.describe(),
            method
                .wanted()
                .pattern(system.arch.unwrap_or(Arch::X86_64))
        ))),
    }
}

/// The name of [os] when it has exactly one way of installing Noctorium, and so no use for --format.
pub fn only_one_way(os: Os) -> Option<&'static str> {
    match os {
        Os::Windows => Some("Windows"),
        Os::MacOs => Some("macOS"),
        Os::Linux | Os::Other(_) => None,
    }
}

/// The package manager that installs a native format here, if there is one.
fn native_method(format: Format, system: &System) -> Option<Method> {
    let installed = |manager: system::PackageManager| system::on_path(manager.tool());
    use system::PackageManager::*;
    match format {
        Format::Deb => installed(Apt).then_some(Method::Apt),
        // The family's own tool first: openSUSE can have dnf installed, and zypper is still its owner.
        Format::Rpm => match system.package_manager {
            Some(Zypper) => Some(Method::Zypper),
            Some(Dnf) => Some(Method::Dnf),
            _ if installed(Dnf) => Some(Method::Dnf),
            _ if installed(Zypper) => Some(Method::Zypper),
            _ => None,
        },
        Format::Arch => installed(Pacman).then_some(Method::Pacman),
        _ => None,
    }
}

/// The method and file for the terminal player.
fn cli_method(found: &Found, arch: Arch) -> Result<(Method, Asset), Problem> {
    let method = Method::cli_for(found.system.os);
    let system_name = match found.system.os {
        Os::Windows => "Windows",
        Os::MacOs => "macOS",
        _ => "Linux",
    };
    match found.release.asset_for(method.wanted(), arch) {
        Some(asset) => Ok((method, asset.clone())),
        None => Err(Problem::Missing(format!(
            "{} has no Noctorium CLI for {system_name} yet. It ships as {} once a release carries it; \
             until then, install Noctorium itself.",
            found.release.tag,
            method.wanted().pattern(arch)
        ))),
    }
}

fn join_or(words: &[String]) -> String {
    match words {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// How far [`carry_out`] has got. `item` is the index into [`Plan::items`].
#[derive(Debug, Clone)]
pub enum Step {
    Downloading {
        item: usize,
        done: u64,
        total: u64,
    },
    /// The download is complete and hashes to what the release said it would.
    Verified {
        item: usize,
        hash: String,
    },
    /// About to do one part of the install. The command is given so it can be shown before it runs.
    Installing {
        item: usize,
        command: String,
    },
    /// That product is in.
    Installed {
        item: usize,
    },
}

/// Downloads what [`plan`] found, checks it, installs it, and clears up after itself.
pub fn carry_out(plan: &Plan, report: &mut dyn FnMut(Step)) -> Result<(), Problem> {
    // Everything that would stop an install, asked before anything is downloaded for it.
    for item in &plan.items {
        install::check_before_download(item.method)?;
    }

    let directory = download_folder();
    std::fs::create_dir_all(&directory)
        .map_err(|e| Problem::Local(format!("Could not make a folder to download into: {e}")))?;

    for (index, item) in plan.items.iter().enumerate() {
        let (file, actual) = fetch::download(
            &item.asset.url,
            &directory,
            &item.asset.name,
            item.asset.size,
            |done, total| {
                report(Step::Downloading {
                    item: index,
                    done,
                    total,
                })
            },
        )?;

        if actual != item.published {
            // Removed rather than left about: a file that failed its checksum is the one file nobody
            // should be able to run by accident afterwards.
            let _ = std::fs::remove_file(&file);
            return Err(Problem::WrongChecksum {
                name: item.asset.name.clone(),
            });
        }
        report(Step::Verified {
            item: index,
            hash: actual,
        });

        for action in &item.actions {
            report(Step::Installing {
                item: index,
                command: action.describe(&plan.places),
            });
            if let Err(problem) = action.perform() {
                let _ = std::fs::remove_file(&file);
                return Err(problem);
            }
        }
        report(Step::Installed { item: index });

        // The download is several hundred megabytes and has done its job.
        let _ = std::fs::remove_file(&file);
    }
    Ok(())
}

/// The whole of the old single-step discovery: the latest release, the desktop application, the format
/// this machine would pick. What a front end with nothing to ask uses.
pub fn discover() -> Result<Plan, Problem> {
    let found = look(None)?;
    plan(&found, Options::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::{Family, OsRelease, PackageManager};

    const SUMS: &str = "\
1111111111111111111111111111111111111111111111111111111111111111  Noctorium-0.7.0-windows-x64-setup.exe
2222222222222222222222222222222222222222222222222222222222222222  noctorium_0.7.0_amd64.deb
3333333333333333333333333333333333333333333333333333333333333333  Noctorium-0.7.0-x86_64.AppImage
4444444444444444444444444444444444444444444444444444444444444444  noctorium-cli-0.7.0-linux-x64.tar.gz
5555555555555555555555555555555555555555555555555555555555555555  Noctorium-0.7.0-macos-arm64.dmg
6666666666666666666666666666666666666666666666666666666666666666  Noctorium-0.7.0-macos-x64.dmg
7777777777777777777777777777777777777777777777777777777777777777  noctorium-cli-0.7.0-macos-arm64.tar.gz
8888888888888888888888888888888888888888888888888888888888888888  noctorium-cli-0.7.0-macos-x64.tar.gz
9999999999999999999999999999999999999999999999999999999999999999  noctorium-installer-cli-macos
";

    fn release(names: &[&str]) -> Release {
        Release {
            tag: "v0.7.0".into(),
            assets: names
                .iter()
                .map(|name| Asset {
                    name: name.to_string(),
                    url: format!("https://example.test/{name}"),
                    size: 100 * 1_048_576,
                })
                .collect(),
        }
    }

    fn linux(family: Option<Family>, manager: Option<PackageManager>) -> System {
        System {
            os: Os::Linux,
            arch: Some(Arch::X86_64),
            arch_name: "x86_64",
            release: Some(OsRelease {
                id: "ubuntu".into(),
                ..Default::default()
            }),
            mac_version: None,
            family,
            package_manager: manager,
            flatpak: false,
            mpv: true,
            root: true,
            sudo_user: None,
        }
    }

    fn found(system: System, names: &[&str]) -> Found {
        Found {
            release: release(names),
            checksums: Some(SUMS.into()),
            latest: true,
            system,
            places: Places {
                home: Some("/home/sam".into()),
                data: Some("/home/sam/.local/share".into()),
                programs: Some(r"C:\Users\Sam\AppData\Local\Programs".into()),
                applications: None,
            },
        }
    }

    /// A Mac, of the kind given, run by an administrator who has not put ~/.local/bin on PATH.
    fn mac(arch: Arch) -> System {
        System {
            os: Os::MacOs,
            arch: Some(arch),
            arch_name: if arch == Arch::Aarch64 {
                "aarch64"
            } else {
                "x86_64"
            },
            release: None,
            mac_version: Some("14.5".into()),
            family: None,
            package_manager: None,
            flatpak: false,
            mpv: false,
            root: false,
            sudo_user: None,
        }
    }

    fn found_on_a_mac(arch: Arch, names: &[&str]) -> Found {
        Found {
            places: Places {
                home: Some("/Users/sam".into()),
                data: Some("/Users/sam/.local/share".into()),
                programs: None,
                applications: Some("/Applications".into()),
            },
            ..found(mac(arch), names)
        }
    }

    const ALL: &[&str] = &[
        "noctorium_0.7.0_amd64.deb",
        "Noctorium-0.7.0-x86_64.AppImage",
        "noctorium-cli-0.7.0-linux-x64.tar.gz",
        "SHA256SUMS.txt",
    ];

    /// What a release with Macs in it carries, around the Mac's own files: the others' files, and the
    /// installers, none of which a Mac may take.
    const WITH_MACS: &[&str] = &[
        "Noctorium-0.7.0-windows-x64-setup.exe",
        "noctorium_0.7.0_amd64.deb",
        "Noctorium-0.7.0-x86_64.AppImage",
        "noctorium-cli-0.7.0-linux-x64.tar.gz",
        "noctorium-installer-cli-macos",
        "Noctorium-0.7.0-macos-arm64.dmg",
        "Noctorium-0.7.0-macos-x64.dmg",
        "noctorium-cli-0.7.0-macos-arm64.tar.gz",
        "noctorium-cli-0.7.0-macos-x64.tar.gz",
        "noctorium-installer-cli-linux-x64",
        "SHA256SUMS.txt",
    ];

    #[test]
    fn each_kind_of_mac_is_planned_the_disk_image_for_its_processor() {
        for (arch, image, published) in [
            (Arch::Aarch64, "Noctorium-0.7.0-macos-arm64.dmg", "5"),
            (Arch::X86_64, "Noctorium-0.7.0-macos-x64.dmg", "6"),
        ] {
            let plan =
                plan(&found_on_a_mac(arch, WITH_MACS), Options::default()).expect("should plan");
            assert_eq!(plan.items.len(), 1);
            let item = &plan.items[0];
            assert_eq!(item.method, Method::MacDiskImage);
            assert_eq!(item.asset.name, image, "{arch:?}");
            assert_eq!(item.published, published.repeat(64));
            assert_eq!(
                item.escalation, None,
                "nothing on a Mac asks for a password"
            );
            assert_eq!(
                item.actions,
                vec![Action::PlaceApp {
                    image: download_folder().join(image),
                    into: PathBuf::from("/Applications"),
                }]
            );
            assert!(item.actions[0]
                .describe(&plan.places)
                .contains("into /Applications"));
            assert!(plan.notes.is_empty(), "{:?}", plan.notes);
            assert!(!plan.needs_password());
        }
    }

    #[test]
    fn both_products_on_a_mac_are_the_app_and_the_cli_with_path_put_right() {
        let options = Options {
            products: Products::Both,
            ..Options::default()
        };
        let plan = plan(&found_on_a_mac(Arch::Aarch64, WITH_MACS), options).expect("should plan");
        let methods: Vec<Method> = plan.items.iter().map(|i| i.method).collect();
        assert_eq!(methods, vec![Method::MacDiskImage, Method::CliMac]);
        let cli = &plan.items[1];
        assert_eq!(cli.asset.name, "noctorium-cli-0.7.0-macos-arm64.tar.gz");
        assert_eq!(cli.published, "7".repeat(64));
        // The PATH the tests run with never has /Users/sam/.local/bin on it, so the profile is changed.
        assert_eq!(
            cli.actions,
            vec![
                Action::UnpackCli {
                    archive: download_folder().join("noctorium-cli-0.7.0-macos-arm64.tar.gz"),
                    into: PathBuf::from("/Users/sam/.local/share/noctorium-cli"),
                    link: Some(PathBuf::from("/Users/sam/.local/bin/noctorium")),
                },
                Action::AddToProfile {
                    profile: PathBuf::from("/Users/sam/.zprofile"),
                },
            ]
        );
        assert!(cli.start.contains("Open a new terminal"), "{}", cli.start);
    }

    #[test]
    fn a_mac_has_one_way_to_install_and_no_use_for_a_format() {
        let found = found_on_a_mac(Arch::Aarch64, WITH_MACS);
        let offers = offers(&found);
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].method, Method::MacDiskImage);
        assert!(offers[0].recommended);
        assert_eq!(
            offers[0].asset.as_ref().map(|a| a.name.as_str()),
            Some("Noctorium-0.7.0-macos-arm64.dmg")
        );

        for format in [Format::Deb, Format::AppImage, Format::Flatpak] {
            let options = Options {
                format,
                ..Options::default()
            };
            let said = plan(&found, options).unwrap_err().to_string();
            assert!(
                said.contains("on macOS, so --format is for Linux"),
                "{said}"
            );
        }
        assert_eq!(only_one_way(Os::MacOs), Some("macOS"));
        assert_eq!(only_one_way(Os::Windows), Some("Windows"));
        assert_eq!(only_one_way(Os::Linux), None);
    }

    /// 0.7 and everything before it: Windows, Linux and installers, and nothing a Mac can take.
    #[test]
    fn a_release_from_before_the_mac_says_so_rather_than_installing_something_else() {
        let found = found_on_a_mac(Arch::Aarch64, ALL);
        let said = plan(&found, Options::default()).unwrap_err().to_string();
        assert!(said.contains("nothing for a Mac"), "{said}");
        assert!(
            said.contains("Noctorium-<version>-macos-arm64.dmg"),
            "{said}"
        );

        let options = Options {
            products: Products::Cli,
            ..Options::default()
        };
        let said = plan(&found, options).unwrap_err().to_string();
        assert!(said.contains("no Noctorium CLI for macOS yet"), "{said}");
        assert!(
            said.contains("noctorium-cli-<version>-macos-arm64.tar.gz"),
            "{said}"
        );
    }

    #[test]
    fn an_account_that_cannot_write_to_applications_is_told_where_it_went_instead() {
        let mut found = found_on_a_mac(Arch::X86_64, WITH_MACS);
        found.places.applications = Some("/Users/sam/Applications".into());
        let plan = plan(&found, Options::default()).expect("should plan");
        assert_eq!(
            plan.items[0].actions,
            vec![Action::PlaceApp {
                image: download_folder().join("Noctorium-0.7.0-macos-x64.dmg"),
                into: PathBuf::from("/Users/sam/Applications"),
            }]
        );
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("cannot write to /Applications")),
            "{:?}",
            plan.notes
        );
    }

    #[test]
    fn root_through_sudo_on_a_mac_is_warned_only_about_the_cli() {
        let mut system = mac(Arch::Aarch64);
        system.root = true;
        system.sudo_user = Some("sam".into());
        let found = Found {
            system,
            ..found_on_a_mac(Arch::Aarch64, WITH_MACS)
        };
        let options = Options {
            products: Products::Both,
            ..Options::default()
        };
        let plan = plan(&found, options).expect("should plan");
        let warnings: Vec<&String> = plan
            .notes
            .iter()
            .filter(|n| n.contains("not for sam"))
            .collect();
        assert_eq!(warnings.len(), 1, "{:?}", plan.notes);
        assert!(warnings[0].contains("Noctorium CLI"), "{}", warnings[0]);
    }

    #[test]
    fn auto_takes_the_distributions_own_package() {
        let found = found(linux(Some(Family::Debian), Some(PackageManager::Apt)), ALL);
        let plan = plan(&found, Options::default()).expect("should plan");
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].method, Method::Apt);
        assert_eq!(plan.items[0].asset.name, "noctorium_0.7.0_amd64.deb");
        assert_eq!(plan.items[0].published, "2".repeat(64));
        assert!(plan.notes.is_empty(), "{:?}", plan.notes);
    }

    #[test]
    fn auto_takes_the_appimage_where_there_is_no_package_for_the_distribution() {
        let found = found(linux(None, None), ALL);
        let plan = plan(&found, Options::default()).expect("should plan");
        assert_eq!(plan.items[0].method, Method::AppImage);
        assert!(plan.notes.iter().any(|n| n.contains("runs anywhere")));
    }

    /// An Arch machine and a release from before there was an Arch package.
    #[test]
    fn auto_falls_back_to_the_appimage_and_says_why() {
        let found = found(linux(Some(Family::Arch), Some(PackageManager::Pacman)), ALL);
        let plan = plan(&found, Options::default()).expect("should plan");
        assert_eq!(plan.items[0].method, Method::AppImage);
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("has no Arch Linux package")),
            "{:?}",
            plan.notes
        );
    }

    #[test]
    fn a_release_with_nothing_for_this_machine_says_what_it_lacks() {
        let found = found(
            linux(Some(Family::Arch), Some(PackageManager::Pacman)),
            &["noctorium_0.7.0_amd64.deb", "SHA256SUMS.txt"],
        );
        let problem = plan(&found, Options::default()).unwrap_err();
        let said = problem.to_string();
        assert!(said.contains("Arch Linux package"), "{said}");
        assert!(said.contains("AppImage"), "{said}");
    }

    #[test]
    fn the_terminal_player_missing_from_a_release_is_said_plainly() {
        let found = found(
            linux(Some(Family::Debian), Some(PackageManager::Apt)),
            &["noctorium_0.7.0_amd64.deb", "SHA256SUMS.txt"],
        );
        let options = Options {
            products: Products::Cli,
            ..Options::default()
        };
        let said = plan(&found, options).unwrap_err().to_string();
        assert!(said.contains("no Noctorium CLI for Linux yet"), "{said}");
        assert!(
            said.contains("noctorium-cli-<version>-linux-x64.tar.gz"),
            "{said}"
        );
    }

    #[test]
    fn both_products_are_planned_one_after_the_other() {
        let found = found(linux(Some(Family::Debian), Some(PackageManager::Apt)), ALL);
        let options = Options {
            products: Products::Both,
            ..Options::default()
        };
        let plan = plan(&found, options).expect("should plan");
        let methods: Vec<Method> = plan.items.iter().map(|i| i.method).collect();
        assert_eq!(methods, vec![Method::Apt, Method::CliLinux]);
        assert!((plan.megabytes() - 200.0).abs() < 0.01);
    }

    #[test]
    fn a_file_missing_from_the_checksums_is_refused() {
        let mut found = found(linux(None, None), ALL);
        found.checksums = Some(SUMS.replace("Noctorium-0.7.0-x86_64.AppImage", "something-else"));
        assert!(matches!(
            plan(&found, Options::default()),
            Err(Problem::Missing(_))
        ));
        found.checksums = None;
        assert!(matches!(
            plan(&found, Options::default()),
            Err(Problem::NoChecksums)
        ));
    }

    #[test]
    fn a_format_that_needs_flatpak_is_refused_without_it() {
        let found = found(linux(None, None), ALL);
        let options = Options {
            format: Format::Flatpak,
            ..Options::default()
        };
        let said = plan(&found, options).unwrap_err().to_string();
        assert!(said.contains("Flatpak is not installed"), "{said}");
    }

    #[test]
    fn a_format_the_release_lacks_names_the_file_it_looked_for() {
        let mut system = linux(None, None);
        system.flatpak = true;
        let found = found(system, ALL);
        let options = Options {
            format: Format::Flatpak,
            ..Options::default()
        };
        let said = plan(&found, options).unwrap_err().to_string();
        assert!(
            said.contains("Noctorium-<version>-x86_64.flatpak"),
            "{said}"
        );
    }

    #[test]
    fn the_offers_say_which_formats_this_release_carries() {
        let mut system = linux(Some(Family::Debian), Some(PackageManager::Apt));
        system.flatpak = true;
        let offers = offers(&found(system, ALL));
        let methods: Vec<Method> = offers.iter().map(|o| o.method).collect();
        assert_eq!(
            methods,
            vec![Method::Apt, Method::AppImage, Method::Flatpak]
        );
        assert!(offers[0].recommended);
        assert!(offers[1].unavailable.is_none());
        assert!(offers[2]
            .unavailable
            .as_deref()
            .unwrap()
            .contains("has no Flatpak"));
    }

    #[test]
    fn an_architecture_nothing_is_built_for_is_told_so() {
        let mut system = linux(None, None);
        system.arch = None;
        system.arch_name = "riscv64";
        let said = plan(&found(system, ALL), Options::default())
            .unwrap_err()
            .to_string();
        assert!(said.contains("riscv64"), "{said}");
    }

    #[test]
    fn root_through_sudo_is_warned_that_per_user_installs_are_roots() {
        let mut system = linux(None, None);
        system.sudo_user = Some("sam".into());
        let plan = plan(&found(system, ALL), Options::default()).expect("should plan");
        assert!(plan.notes.iter().any(|n| n.contains("not for sam")));
    }

    #[test]
    fn a_missing_mpv_is_mentioned_for_the_formats_that_need_the_distributions() {
        let mut system = linux(Some(Family::Debian), Some(PackageManager::Apt));
        system.mpv = false;
        let plan = plan(&found(system, ALL), Options::default()).expect("should plan");
        assert!(plan
            .notes
            .iter()
            .any(|n| n.contains("sudo apt install mpv")));
    }

    #[test]
    fn products_and_formats_are_read_from_their_flags() {
        assert_eq!(Products::parse("both"), Some(Products::Both));
        assert_eq!(Products::parse("CLI"), Some(Products::Cli));
        assert_eq!(Products::parse("everything"), None);
        assert_eq!(Format::parse("AppImage"), Some(Format::AppImage));
        assert_eq!(Format::parse("arch"), Some(Format::Arch));
        assert_eq!(Format::parse("snap"), None);
    }
}
