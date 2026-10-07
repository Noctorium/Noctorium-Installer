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

use crate::fetch::{self, Fetched};
use crate::github::{self, Arch, Asset, Problem, Release, Wanted};
use crate::install::{self, Action, Method, Places};
use crate::system::{self, Asking, Os, System};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// What can be installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Product {
    /// The desktop application, with a window.
    Desktop,
    /// The terminal player.
    Cli,
    /// Noctorium Stats, which signs in to the listener's Noctorium account and shows what they have
    /// listened to: a program of its own, beside the player rather than part of it.
    Stats,
}

impl Product {
    pub fn name(self) -> &'static str {
        match self {
            Product::Desktop => "Noctorium",
            Product::Cli => "Noctorium CLI",
            Product::Stats => "Noctorium Stats",
        }
    }

    /// All of them, in the order they are offered, planned and listed.
    pub const ALL: [Product; 3] = [Product::Desktop, Product::Cli, Product::Stats];
}

/// Which of them were asked for: any of the three, in any combination but none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Products {
    pub desktop: bool,
    pub cli: bool,
    pub stats: bool,
}

impl Default for Products {
    fn default() -> Self {
        Products::DESKTOP
    }
}

impl Products {
    pub const DESKTOP: Products = Products {
        desktop: true,
        cli: false,
        stats: false,
    };
    pub const CLI: Products = Products {
        desktop: false,
        cli: true,
        stats: false,
    };
    pub const STATS: Products = Products {
        desktop: false,
        cli: false,
        stats: true,
    };
    /// Noctorium and the Noctorium CLI, which is what `both` meant before there were three, and still
    /// means: a script written then installs now what it installed then.
    pub const BOTH: Products = Products {
        desktop: true,
        cli: true,
        stats: false,
    };
    pub const ALL: Products = Products {
        desktop: true,
        cli: true,
        stats: true,
    };

    pub fn each(self) -> Vec<Product> {
        Product::ALL
            .into_iter()
            .filter(|product| self.includes(*product))
            .collect()
    }

    /// What `--product` takes: `desktop`, `cli`, `stats`, `both` (Noctorium and the CLI) or `all`, or
    /// several of them joined by commas or plus signs -- `desktop,stats`. Nothing for a word it does not
    /// know, or for nothing at all.
    pub fn parse(words: &str) -> Option<Products> {
        let mut products = Products::none();
        for word in words.split([',', '+']).map(str::trim) {
            let one = match word.to_ascii_lowercase().as_str() {
                "desktop" => Products::DESKTOP,
                "cli" => Products::CLI,
                "stats" => Products::STATS,
                "both" => Products::BOTH,
                "all" => Products::ALL,
                _ => return None,
            };
            products = products.with(one);
        }
        (!products.is_empty()).then_some(products)
    }

    /// From a yes or no for each, which is how the window asks; nothing when all are no.
    pub fn from_choice(desktop: bool, cli: bool, stats: bool) -> Option<Products> {
        let products = Products {
            desktop,
            cli,
            stats,
        };
        (!products.is_empty()).then_some(products)
    }

    pub fn includes(self, product: Product) -> bool {
        match product {
            Product::Desktop => self.desktop,
            Product::Cli => self.cli,
            Product::Stats => self.stats,
        }
    }

    /// These and [other] together.
    pub fn with(self, other: Products) -> Products {
        Products {
            desktop: self.desktop || other.desktop,
            cli: self.cli || other.cli,
            stats: self.stats || other.stats,
        }
    }

    pub fn is_empty(self) -> bool {
        !self.desktop && !self.cli && !self.stats
    }

    pub fn count(self) -> usize {
        self.each().len()
    }

    fn none() -> Products {
        Products {
            desktop: false,
            cli: false,
            stats: false,
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
            Method::WindowsMsi
            | Method::WindowsSetup
            | Method::MacDiskImage
            | Method::CliWindows
            | Method::CliLinux
            | Method::CliMac
            | Method::StatsWindows
            | Method::StatsLinux
            | Method::StatsMac => Format::Auto,
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
    /// On Windows, the .msi's own wizard rather than a progress bar, for somebody who wants to choose the
    /// folder it goes in. Nothing anywhere else.
    pub wizard: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            products: Products::DESKTOP,
            format: Format::Auto,
            asking: Asking::Window,
            wizard: false,
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
/// On Windows there is one, and on a Mac there is one, the disk image for its processor. Windows' one is
/// the .msi, or for a release from before the .msi was published, the setup .exe -- which is the .msi
/// again, wrapped, and is not offered beside it as a choice, because it only ever does the same thing
/// slower. On Linux there is the distribution's own package when its package manager is here, then the
/// AppImage, which runs anywhere, then the Flatpak, which needs Flatpak. Each says whether this release
/// carries it, so a menu can show what there is rather than offering something that will fail after the
/// download.
pub fn offers(found: &Found) -> Vec<Offer> {
    let system = &found.system;
    let Some(arch) = system.arch else {
        return vec![];
    };
    let mut methods: Vec<(Method, Option<String>)> = Vec::new();
    match system.os {
        Os::Windows => {
            let has = |wanted: Wanted| found.release.asset_for(wanted, arch).is_some();
            let setup_only = !has(Wanted::WindowsMsi) && has(Wanted::WindowsSetup);
            let method = if setup_only {
                Method::WindowsSetup
            } else {
                Method::WindowsMsi
            };
            methods.push((method, None));
        }
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

/// The Noctorium CLI's file in this release for this machine, if it carries one.
pub fn cli_asset(found: &Found) -> Option<&Asset> {
    let arch = found.system.arch?;
    found
        .release
        .asset_for(Method::cli_for(found.system.os).wanted(), arch)
}

/// Noctorium Stats' file in this release for this machine, if it carries one.
pub fn stats_asset(found: &Found) -> Option<&Asset> {
    let arch = found.system.arch?;
    found
        .release
        .asset_for(Method::stats_for(found.system.os).wanted(), arch)
}

/// What is about to happen, settled before anything is downloaded.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The release's tag, as published: `v0.4.1`.
    pub tag: String,
    pub latest: bool,
    pub items: Vec<Item>,
    pub places: Places,
    /// Where the downloads go: [download_folder], except in a test.
    pub downloads: PathBuf,
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
    /// How much of it an earlier run left in the download folder.
    pub on_disk: OnDisk,
    /// What installing it takes, which is what gets shown before it is done.
    pub actions: Vec<Action>,
    /// What to say once it is in.
    pub start: String,
}

/// How much of a download is in the download folder already, from an earlier run: as far as its size
/// says, which is all a plan looks at. Whether it is really the file is the checksum's to decide, when it
/// is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnDisk {
    Nothing,
    /// A `.partial` of this many bytes, which the download carries on from.
    Part(u64),
    /// A file of the whole size, which is checked and used instead of being downloaded again.
    Whole,
}

impl OnDisk {
    /// What there is at [file] for a download of [size] bytes.
    pub fn at(file: &Path, size: u64) -> OnDisk {
        match std::fs::metadata(file) {
            Ok(found) if found.is_file() && (size == 0 || found.len() == size) => {
                return OnDisk::Whole
            }
            _ => {}
        }
        match fetch::already_have(file, size) {
            0 => OnDisk::Nothing,
            have => OnDisk::Part(have),
        }
    }
}

impl Item {
    /// The bytes still to come down, as far as the plan can tell.
    pub fn to_download(&self) -> u64 {
        match self.on_disk {
            OnDisk::Nothing => self.asset.size,
            OnDisk::Part(have) => self.asset.size.saturating_sub(have),
            OnDisk::Whole => 0,
        }
    }
}

impl Plan {
    /// The version as somebody reads it, without the tag's leading v.
    pub fn version(&self) -> &str {
        self.tag.strip_prefix('v').unwrap_or(&self.tag)
    }

    pub fn megabytes(&self) -> f64 {
        self.items.iter().map(|i| i.asset.megabytes()).sum()
    }

    /// What is still to come down, which is less than [megabytes] when an earlier run left some of it.
    pub fn megabytes_to_download(&self) -> f64 {
        self.items.iter().map(|i| i.to_download()).sum::<u64>() as f64 / 1_048_576.0
    }

    /// Whether going ahead will put a password prompt in front of somebody.
    pub fn needs_password(&self) -> bool {
        self.items.iter().any(|i| i.escalation.is_some())
    }
}

/// Where downloads go: a folder of this program's own, so a half-finished download is never left in the
/// middle of somebody's Downloads folder -- and the same one every time, so the next run finds what this
/// one left.
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
    let downloads = download_folder();

    let mut notes = Vec::new();
    let mut items = Vec::new();
    for product in options.products.each() {
        let (method, asset) = match product {
            Product::Desktop => desktop_method(found, options.format, &mut notes)?,
            Product::Cli => cli_method(found, arch)?,
            // Left out, with a word, rather than refused: Stats is new, and every release before it --
            // the one an older installer, a script or `--version` is pointed at -- has none. Whatever else
            // was asked for is installed all the same.
            Product::Stats => match stats_method(found, arch) {
                Some(chosen) => chosen,
                None => {
                    notes.push(stats_left_out(found));
                    continue;
                }
            },
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
        if !method.needs_root()
            && !matches!(
                method,
                Method::WindowsMsi | Method::WindowsSetup | Method::MacDiskImage | Method::StatsMac
            )
        {
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

        if matches!(method, Method::MacDiskImage | Method::StatsMac) {
            if let Some(folder) = &found.places.applications {
                if folder != Path::new("/Applications") {
                    notes.push(format!(
                        "This account cannot write to /Applications -- on a Mac that takes an \
                         administrator -- so {} goes into {}, which is yours alone. Spotlight finds it \
                         there just the same.",
                        product.name(),
                        found.places.show(folder)
                    ));
                }
            }
        }

        // Said because nobody would otherwise know: the folder a Windows install is in is not something
        // anybody looks at, until it moves.
        if method == Method::WindowsMsi {
            let installed = found
                .places
                .installed
                .as_ref()
                .map(|folder| folder.display().to_string());
            let installed = installed
                .as_deref()
                .map(|f| f.trim_end_matches(['\\', '/']));
            notes.push(match (installed, options.wizard) {
                (Some(folder), false) => format!(
                    "Noctorium is installed in {folder} already, and the new version goes into the \
                     same folder. Windows asks for permission first; after that there is nothing to \
                     click."
                ),
                (Some(folder), true) => format!(
                    "Noctorium is installed in {folder} already. The Noctorium setup opens on that \
                     folder, and another can be chosen there."
                ),
                (None, true) => {
                    "The Noctorium setup opens, where the folder it goes in can be chosen.".into()
                }
                (None, false) => "Windows asks for permission first; after that there is nothing \
                                  to click, and it goes into Program Files."
                    .into(),
            });
        }

        let file = downloads.join(&asset.name);
        let mut actions =
            install::actions_for(method, &file, escalation, &found.places, options.wizard)?;
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
        // Said only when it is all there is to say: with the Noctorium CLI beside it, the note above has
        // said it already, and Noctorium Stats is in the applications menu whatever PATH says.
        if method == Method::StatsLinux && !user_bin_on_path && !options.products.cli {
            notes.push(
                "~/.local/bin is not on your PATH, so `noctorium-stats` will not be found by name in a \
                 terminal until it is. The applications menu has it either way."
                    .into(),
            );
        }
        // Most Linux distributions put ~/.local/bin on PATH once it exists, and the end of the install says
        // what to do on one that does not. macOS never does, so on a Mac it is done here, and shown in
        // the plan like everything else that changes a file -- unless NOCTORIUM_NO_PATH asks for the
        // profile to be left alone.
        if method == Method::CliMac && !user_bin_on_path && !found.places.leave_path {
            actions.push(install::add_user_bin_to_path(&found.places)?);
        }
        if method == Method::StatsWindows {
            actions.extend(install::list_installed(&found.places, found.version())?);
        }
        let start = install::how_to_start(method, &found.places, user_bin_on_path);
        items.push(Item {
            product,
            on_disk: OnDisk::at(&file, asset.size),
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
        downloads,
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
        if best.method == Method::WindowsSetup {
            notes.push(format!(
                "{} has no .msi, so this is its setup .exe, which unpacks one again before it starts \
                 and asks its own questions.",
                found.release.tag
            ));
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

/// The method and file for Noctorium Stats, or nothing when this release carries none for this machine.
fn stats_method(found: &Found, arch: Arch) -> Option<(Method, Asset)> {
    let method = Method::stats_for(found.system.os);
    found
        .release
        .asset_for(method.wanted(), arch)
        .map(|asset| (method, asset.clone()))
}

/// What is said when Noctorium Stats was asked for and this release has none for this machine.
pub fn stats_left_out(found: &Found) -> String {
    let system_name = match found.system.os {
        Os::Windows => "Windows",
        Os::MacOs => "macOS",
        _ => "Linux",
    };
    let pattern = Method::stats_for(found.system.os)
        .wanted()
        .pattern(found.system.arch.unwrap_or(Arch::X86_64));
    format!(
        "Noctorium Stats is not in {} yet, so it is left out and nothing is downloaded for it. It is \
         new, and comes for {system_name} as {pattern} from the release that first carries it.",
        found.release.tag
    )
}

fn join_or(words: &[String]) -> String {
    match words {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// How far [`carry_out`] has got. `item` is the index into [`Plan::items`], and every step says which it
/// is about, because with two products their downloads run at once and their steps arrive interleaved.
///
/// Each item ends in exactly one of [Step::Installed] -- or [Step::Verified], when only downloading --
/// and [Step::Failed].
#[derive(Debug, Clone)]
pub enum Step {
    Downloading {
        item: usize,
        done: u64,
        total: u64,
    },
    /// The file is whole and hashes to what the release said it would: downloaded, carried on from an
    /// earlier run, or found from one.
    Verified {
        item: usize,
        how: Fetched,
    },
    /// About to do one part of the install. The command is given so it can be shown before it runs.
    Installing {
        item: usize,
        command: String,
    },
    /// That product is in, with anything the install had to say about it.
    Installed {
        item: usize,
        note: Option<String>,
    },
    /// That product is not, and why. The others carry on.
    Failed {
        item: usize,
        why: String,
    },
}

/// Downloads what [`plan`] found, checks it, installs it, and clears up after itself.
///
/// Everything is downloaded at once, each on a connection of its own, and each product is installed the
/// moment its own download has been checked, rather than after the slowest of them. The installs
/// themselves go one at a time, in the order the downloads finish: two installers at once is two
/// password prompts at once, and two package managers at once is one of them refusing to start. In
/// practice that is the Noctorium CLI, which is a fifth of the size, unpacked while the rest of Noctorium
/// is still coming down, and then Noctorium.
///
/// One product failing does not stop the other: somebody who asked for both and has a network that
/// dropped half of one still gets the other. The first failure is what this returns, once everything has
/// finished; each is reported as it happens. A download that was checked is removed once it is
/// installed, and kept when the install failed or was cancelled, so trying again does not download it
/// again.
pub fn carry_out(plan: &Plan, report: &mut dyn FnMut(Step)) -> Result<(), Problem> {
    // Everything that would stop an install, asked before anything is downloaded for it.
    for item in &plan.items {
        install::check_before_download(item.method)?;
    }
    run(plan, report, true)
}

/// Downloads what [`plan`] found and checks it, and installs nothing: the files are left in the
/// download folder, where a later run finds them and installs them without downloading them again.
pub fn download_only(plan: &Plan, report: &mut dyn FnMut(Step)) -> Result<(), Problem> {
    run(plan, report, false)
}

/// What the threads doing the work tell the one reporting it.
enum Event {
    Progress(usize, u64, u64),
    Fetched(usize, Result<(PathBuf, Fetched), Problem>),
    Installing(usize, String),
    Installed(usize, Result<Option<String>, Problem>),
}

fn run(plan: &Plan, report: &mut dyn FnMut(Step), install: bool) -> Result<(), Problem> {
    if plan.items.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(&plan.downloads)
        .map_err(|e| Problem::Local(format!("Could not make a folder to download into: {e}")))?;

    let mut first_failure: Option<Problem> = None;
    let mut fail = |item: usize, problem: Problem, report: &mut dyn FnMut(Step)| {
        report(Step::Failed {
            item,
            why: problem.to_string(),
        });
        first_failure.get_or_insert(problem);
    };

    std::thread::scope(|scope| {
        let (events, heard) = mpsc::channel::<Event>();
        for (index, item) in plan.items.iter().enumerate() {
            let events = events.clone();
            scope.spawn(move || {
                let progress = events.clone();
                let fetched = fetch::download(
                    &item.asset.url,
                    &plan.downloads,
                    &item.asset.name,
                    item.asset.size,
                    &item.published,
                    |done, total| {
                        let _ = progress.send(Event::Progress(index, done, total));
                    },
                );
                let _ = events.send(Event::Fetched(index, fetched));
            });
        }

        // The one thread that installs, which is given each item as its download is checked and works
        // through them in that order. It is not this thread, so that the progress of a download still
        // coming down goes on being reported while another product is being installed.
        let (work, queue) = mpsc::channel::<usize>();
        let mut work = Some(work);
        if install {
            let events = events.clone();
            scope.spawn(move || {
                for index in queue {
                    let mut outcome = Ok(None);
                    for action in &plan.items[index].actions {
                        let _ =
                            events.send(Event::Installing(index, action.describe(&plan.places)));
                        match action.perform() {
                            Ok(note) => {
                                if note.is_some() {
                                    outcome = Ok(note);
                                }
                            }
                            Err(problem) => {
                                outcome = Err(problem);
                                break;
                            }
                        }
                    }
                    let _ = events.send(Event::Installed(index, outcome));
                }
            });
        }
        // Only the threads hold a way to say anything now, so the loop below ends when the last of them
        // has finished.
        drop(events);

        let mut finished = 0;
        for event in heard {
            match event {
                Event::Progress(item, done, total) => {
                    report(Step::Downloading { item, done, total })
                }
                Event::Fetched(item, Ok((_, how))) => {
                    report(Step::Verified { item, how });
                    match &work {
                        Some(work) if install => {
                            let _ = work.send(item);
                        }
                        _ => finished += 1,
                    }
                }
                Event::Fetched(item, Err(problem)) => {
                    fail(item, problem, report);
                    finished += 1;
                }
                Event::Installing(item, command) => report(Step::Installing { item, command }),
                Event::Installed(item, Ok(note)) => {
                    // The download is several hundred megabytes and has done its job.
                    let _ = std::fs::remove_file(&plan.items[item].file);
                    report(Step::Installed { item, note });
                    finished += 1;
                }
                Event::Installed(item, Err(problem)) => {
                    fail(item, problem, report);
                    finished += 1;
                }
            }
            // Nothing more to install, so the installing thread is let go.
            if finished == plan.items.len() {
                work = None;
            }
        }
    });

    match first_failure {
        Some(problem) => Err(problem),
        None => Ok(()),
    }
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
0000000000000000000000000000000000000000000000000000000000000000  Noctorium-0.7.0-windows-x64.msi
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  noctorium-cli-0.7.0-windows-x64.zip
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
                ..Places::default()
            },
        }
    }

    fn windows() -> System {
        System {
            os: Os::Windows,
            arch: Some(Arch::X86_64),
            arch_name: "x86_64",
            release: None,
            mac_version: None,
            family: None,
            package_manager: None,
            flatpak: false,
            mpv: false,
            root: false,
            sudo_user: None,
        }
    }

    /// What a Windows release carries: the .msi, the setup that wraps it, and the CLI's archive.
    const ON_WINDOWS: &[&str] = &[
        "Noctorium-0.7.0-windows-x64-setup.exe",
        "Noctorium-0.7.0-windows-x64.msi",
        "noctorium-cli-0.7.0-windows-x64.zip",
        "Noctorium-Installer-windows-x64.exe",
        "SHA256SUMS.txt",
    ];

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
                applications: Some("/Applications".into()),
                ..Places::default()
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
            products: Products::BOTH,
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
                    path: false,
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
            products: Products::CLI,
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
            products: Products::BOTH,
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
            products: Products::CLI,
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
            products: Products::BOTH,
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
        assert_eq!(Products::parse("both"), Some(Products::BOTH));
        assert_eq!(Products::parse("CLI"), Some(Products::CLI));
        assert_eq!(Products::parse("everything"), None);
        assert_eq!(Format::parse("AppImage"), Some(Format::AppImage));
        assert_eq!(Format::parse("arch"), Some(Format::Arch));
        assert_eq!(Format::parse("snap"), None);
    }

    // ------------------------------------------------------------ Windows

    #[test]
    fn windows_takes_the_msi_and_hands_it_to_msiexec() {
        let plan = plan(&found(windows(), ON_WINDOWS), Options::default()).expect("should plan");
        assert_eq!(plan.items.len(), 1);
        let item = &plan.items[0];
        assert_eq!(item.method, Method::WindowsMsi);
        assert_eq!(item.asset.name, "Noctorium-0.7.0-windows-x64.msi");
        assert_eq!(item.published, "0".repeat(64));
        assert_eq!(
            item.actions,
            vec![Action::InstallMsi {
                msi: download_folder().join("Noctorium-0.7.0-windows-x64.msi"),
                folder: None,
                wizard: false,
            }]
        );
        let shown = item.actions[0].describe(&plan.places);
        assert!(
            shown.starts_with("msiexec /i ")
                && shown.ends_with(" /passive /norestart MSIFASTINSTALL=7"),
            "{shown}"
        );
        assert!(
            plan.notes.iter().any(|n| n.contains("nothing to click")),
            "{:?}",
            plan.notes
        );
        assert!(item.start.contains("Start menu"));
        assert!(
            !plan.needs_password(),
            "Windows asks for its own permission"
        );
    }

    #[test]
    fn a_release_without_an_msi_falls_back_to_its_setup_and_says_so() {
        let names = &[
            "Noctorium-0.7.0-windows-x64-setup.exe",
            "noctorium-cli-0.7.0-windows-x64.zip",
            "SHA256SUMS.txt",
        ];
        let found = found(windows(), names);
        let offers = offers(&found);
        assert_eq!(
            offers.len(),
            1,
            "the setup is never offered beside the .msi"
        );
        assert_eq!(offers[0].method, Method::WindowsSetup);

        let plan = plan(&found, Options::default()).expect("should plan");
        let item = &plan.items[0];
        assert_eq!(item.method, Method::WindowsSetup);
        assert_eq!(item.asset.name, "Noctorium-0.7.0-windows-x64-setup.exe");
        assert_eq!(
            item.actions,
            vec![Action::Run {
                program: download_folder()
                    .join("Noctorium-0.7.0-windows-x64-setup.exe")
                    .to_string_lossy()
                    .into(),
                args: vec![],
            }]
        );
        assert!(
            plan.notes.iter().any(|n| n.contains("has no .msi")),
            "{:?}",
            plan.notes
        );
    }

    #[test]
    fn a_release_with_neither_says_what_it_looked_for() {
        let found = found(windows(), &["noctorium_0.7.0_amd64.deb", "SHA256SUMS.txt"]);
        let said = plan(&found, Options::default()).unwrap_err().to_string();
        assert!(said.contains("Windows Installer package (.msi)"), "{said}");
    }

    #[test]
    fn an_upgrade_on_windows_goes_into_the_folder_noctorium_is_in() {
        let mut found = found(windows(), ON_WINDOWS);
        found.places.installed = Some(PathBuf::from(r"D:\Programs\Noctorium\"));
        let plan = plan(&found, Options::default()).expect("should plan");
        assert_eq!(
            plan.items[0].actions,
            vec![Action::InstallMsi {
                msi: download_folder().join("Noctorium-0.7.0-windows-x64.msi"),
                folder: Some(PathBuf::from(r"D:\Programs\Noctorium\")),
                wizard: false,
            }]
        );
        assert!(plan.items[0].actions[0]
            .describe(&plan.places)
            .ends_with(r#" INSTALLDIR="D:\Programs\Noctorium""#));
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains(r"D:\Programs\Noctorium")),
            "{:?}",
            plan.notes
        );
    }

    #[test]
    fn the_wizard_is_there_for_choosing_the_folder() {
        let options = Options {
            wizard: true,
            ..Options::default()
        };
        let plan = plan(&found(windows(), ON_WINDOWS), options).expect("should plan");
        assert_eq!(
            plan.items[0].actions,
            vec![Action::InstallMsi {
                msi: download_folder().join("Noctorium-0.7.0-windows-x64.msi"),
                folder: None,
                wizard: true,
            }]
        );
        let shown = plan.items[0].actions[0].describe(&plan.places);
        assert!(!shown.contains("/passive"), "{shown}");
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("folder it goes in can be chosen")),
            "{:?}",
            plan.notes
        );

        // Over an install that is there already, the wizard starts on its folder, and says so rather
        // than promising nothing to click.
        let mut installed = found(windows(), ON_WINDOWS);
        installed.places.installed = Some(PathBuf::from(r"D:\Noctorium"));
        let plan = super::plan(&installed, options).expect("should plan");
        assert!(
            plan.items[0].actions[0]
                .describe(&plan.places)
                .ends_with(r#"MSIFASTINSTALL=7 INSTALLDIR="D:\Noctorium""#),
            "{:?}",
            plan.items[0].actions
        );
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("opens on that folder") && !n.contains("nothing to click")),
            "{:?}",
            plan.notes
        );
    }

    #[test]
    fn both_products_on_windows_are_the_msi_and_the_cli_on_the_path() {
        let options = Options {
            products: Products::BOTH,
            ..Options::default()
        };
        let plan = plan(&found(windows(), ON_WINDOWS), options).expect("should plan");
        let methods: Vec<Method> = plan.items.iter().map(|i| i.method).collect();
        assert_eq!(methods, vec![Method::WindowsMsi, Method::CliWindows]);
        assert_eq!(
            plan.items[1].actions,
            vec![Action::UnpackCli {
                archive: download_folder().join("noctorium-cli-0.7.0-windows-x64.zip"),
                into: PathBuf::from(r"C:\Users\Sam\AppData\Local\Programs").join("Noctorium CLI"),
                link: None,
                path: true,
            }]
        );
        assert!((plan.megabytes() - 200.0).abs() < 0.01);
    }

    #[test]
    fn a_mac_leaves_its_profile_alone_when_asked_to() {
        let mut found = found_on_a_mac(Arch::Aarch64, WITH_MACS);
        found.places.leave_path = true;
        let options = Options {
            products: Products::CLI,
            ..Options::default()
        };
        let plan = plan(&found, options).expect("should plan");
        assert!(
            !plan.items[0]
                .actions
                .iter()
                .any(|a| matches!(a, Action::AddToProfile { .. })),
            "{:?}",
            plan.items[0].actions
        );
    }

    #[test]
    fn the_windows_choice_is_any_of_the_three_but_none() {
        assert_eq!(
            Products::from_choice(true, false, false),
            Some(Products::DESKTOP)
        );
        assert_eq!(
            Products::from_choice(false, true, false),
            Some(Products::CLI)
        );
        assert_eq!(
            Products::from_choice(true, true, false),
            Some(Products::BOTH)
        );
        assert_eq!(
            Products::from_choice(false, false, true),
            Some(Products::STATS)
        );
        assert_eq!(Products::from_choice(true, true, true), Some(Products::ALL));
        assert_eq!(Products::from_choice(false, false, false), None);
        assert!(Products::BOTH.includes(Product::Cli));
        assert!(!Products::BOTH.includes(Product::Stats));
        assert!(!Products::DESKTOP.includes(Product::Cli));
        assert_eq!(
            Products::ALL.each(),
            vec![Product::Desktop, Product::Cli, Product::Stats],
            "always in the same order, whatever order they were asked for in"
        );
        assert_eq!(Products::default(), Products::DESKTOP);
    }

    /// `both` is still Noctorium and the CLI, as it was in every script written before Stats.
    #[test]
    fn the_products_are_read_one_at_a_time_or_several_together() {
        assert_eq!(Products::parse("stats"), Some(Products::STATS));
        assert_eq!(Products::parse("both"), Some(Products::BOTH));
        assert_eq!(Products::parse("all"), Some(Products::ALL));
        assert_eq!(
            Products::parse("desktop,stats"),
            Some(Products {
                desktop: true,
                cli: false,
                stats: true
            })
        );
        assert_eq!(
            Products::parse("Stats+CLI"),
            Some(Products {
                desktop: false,
                cli: true,
                stats: true
            })
        );
        assert_eq!(Products::parse("both,stats"), Some(Products::ALL));
        assert_eq!(
            Products::parse("stats, desktop"),
            Products::parse("desktop,stats")
        );
        assert_eq!(Products::parse(""), None);
        assert_eq!(
            Products::parse("desktop,"),
            None,
            "an empty word is not a product"
        );
        assert_eq!(Products::parse("desktop,player"), None);
    }

    // ------------------------------------------------------------ Noctorium Stats

    const STATS_SUMS: &str = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  noctorium-stats-0.7.0-windows-x64.zip
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  noctorium-stats-0.7.0-linux-x64.tar.gz
cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc  noctorium-stats-0.7.0-macos-arm64.zip
dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd  noctorium-stats-0.7.0-macos-x64.zip
eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee  Noctorium-Stats-0.7.0.apk
";

    /// Everything a release with Stats carries, around the files these tests look for.
    const WITH_STATS: &[&str] = &[
        "Noctorium-0.7.0-windows-x64.msi",
        "noctorium-cli-0.7.0-windows-x64.zip",
        "noctorium_0.7.0_amd64.deb",
        "Noctorium-0.7.0-x86_64.AppImage",
        "noctorium-cli-0.7.0-linux-x64.tar.gz",
        "Noctorium-0.7.0-macos-arm64.dmg",
        "Noctorium-0.7.0-macos-x64.dmg",
        "noctorium-cli-0.7.0-macos-arm64.tar.gz",
        "noctorium-stats-0.7.0-windows-x64.zip",
        "noctorium-stats-0.7.0-linux-x64.tar.gz",
        "noctorium-stats-0.7.0-macos-arm64.zip",
        "noctorium-stats-0.7.0-macos-x64.zip",
        "Noctorium-Stats-0.7.0.apk",
        "SHA256SUMS.txt",
    ];

    fn with_stats(found: Found) -> Found {
        Found {
            release: release(WITH_STATS),
            checksums: Some(format!("{SUMS}{STATS_SUMS}")),
            ..found
        }
    }

    fn only(products: Products) -> Options {
        Options {
            products,
            ..Options::default()
        }
    }

    #[test]
    fn windows_unpacks_stats_beside_the_cli_and_gives_it_a_shortcut_and_an_uninstaller() {
        let mut found = with_stats(found(windows(), ON_WINDOWS));
        found.places.start_menu =
            Some(r"C:\Users\Sam\AppData\Roaming\Microsoft\Windows\Start Menu\Programs".into());
        let plan = plan(&found, only(Products::STATS)).expect("should plan");
        assert_eq!(plan.items.len(), 1);
        let item = &plan.items[0];
        assert_eq!(item.product, Product::Stats);
        assert_eq!(item.method, Method::StatsWindows);
        assert_eq!(item.asset.name, "noctorium-stats-0.7.0-windows-x64.zip");
        assert_eq!(item.published, "a".repeat(64));
        assert_eq!(item.escalation, None);
        let folder = PathBuf::from(r"C:\Users\Sam\AppData\Local\Programs").join("Noctorium Stats");
        let shortcut =
            PathBuf::from(r"C:\Users\Sam\AppData\Roaming\Microsoft\Windows\Start Menu\Programs")
                .join("Noctorium Stats.lnk");
        assert_eq!(
            item.actions,
            vec![
                Action::UnpackStats {
                    archive: download_folder().join("noctorium-stats-0.7.0-windows-x64.zip"),
                    into: folder.clone(),
                    link: None,
                },
                Action::StartMenuShortcut {
                    folder: folder.clone(),
                    shortcut: shortcut.clone(),
                },
                Action::ListInstalled {
                    folder,
                    shortcut,
                    version: "0.7.0".into(),
                },
            ]
        );
        assert!(item.start.contains("Start menu"), "{}", item.start);
        assert!(
            plan.notes.is_empty(),
            "nothing about the .msi for Stats alone: {:?}",
            plan.notes
        );

        // A trial install leaves Windows' list of programs alone, as it does the PATH.
        found.places.leave_path = true;
        let plan = super::plan(&found, only(Products::STATS)).expect("should plan");
        assert!(
            !plan.items[0]
                .actions
                .iter()
                .any(|a| matches!(a, Action::ListInstalled { .. })),
            "{:?}",
            plan.items[0].actions
        );
    }

    #[test]
    fn all_three_on_windows_are_planned_in_order_and_downloaded_together() {
        let mut found = with_stats(found(windows(), ON_WINDOWS));
        found.places.start_menu = Some(r"C:\Start".into());
        let plan = plan(&found, only(Products::ALL)).expect("should plan");
        let methods: Vec<Method> = plan.items.iter().map(|i| i.method).collect();
        assert_eq!(
            methods,
            vec![Method::WindowsMsi, Method::CliWindows, Method::StatsWindows]
        );
        assert!((plan.megabytes() - 300.0).abs() < 0.01);
    }

    #[test]
    fn linux_unpacks_stats_links_it_and_puts_it_in_the_menu() {
        let found = with_stats(found(
            linux(Some(Family::Debian), Some(PackageManager::Apt)),
            ALL,
        ));
        let plan = plan(&found, only(Products::STATS)).expect("should plan");
        let item = &plan.items[0];
        assert_eq!(item.method, Method::StatsLinux);
        assert_eq!(item.asset.name, "noctorium-stats-0.7.0-linux-x64.tar.gz");
        assert_eq!(item.published, "b".repeat(64));
        assert_eq!(
            item.actions,
            vec![
                Action::UnpackStats {
                    archive: download_folder().join("noctorium-stats-0.7.0-linux-x64.tar.gz"),
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
        // The PATH the tests run with never has /home/sam/.local/bin on it.
        assert!(
            plan.notes.iter().any(|n| n.contains("`noctorium-stats`")),
            "{:?}",
            plan.notes
        );
        assert!(
            item.start.contains("~/.local/bin/noctorium-stats"),
            "{}",
            item.start
        );
        assert!(!plan.needs_password(), "Stats is for this user alone");

        // With the CLI beside it, ~/.local/bin is said once, by the CLI's note.
        let plan = super::plan(&found, only(Products::CLI.with(Products::STATS))).unwrap();
        let about_the_path = plan
            .notes
            .iter()
            .filter(|n| n.contains("~/.local/bin is not on your PATH"))
            .count();
        assert_eq!(about_the_path, 1, "{:?}", plan.notes);
    }

    #[test]
    fn each_kind_of_mac_copies_its_own_stats_into_applications() {
        for (arch, zip, published) in [
            (Arch::Aarch64, "noctorium-stats-0.7.0-macos-arm64.zip", "c"),
            (Arch::X86_64, "noctorium-stats-0.7.0-macos-x64.zip", "d"),
        ] {
            let found = with_stats(found_on_a_mac(arch, WITH_MACS));
            let plan = plan(&found, only(Products::DESKTOP.with(Products::STATS))).unwrap();
            let methods: Vec<Method> = plan.items.iter().map(|i| i.method).collect();
            assert_eq!(methods, vec![Method::MacDiskImage, Method::StatsMac]);
            let item = &plan.items[1];
            assert_eq!(item.asset.name, zip, "{arch:?}");
            assert_eq!(item.published, published.repeat(64));
            assert_eq!(
                item.actions,
                vec![Action::PlaceZippedApp {
                    archive: download_folder().join(zip),
                    into: PathBuf::from("/Applications"),
                    app: "Noctorium Stats.app".into(),
                }]
            );
            assert!(item.start.contains("Noctorium Stats.app"), "{}", item.start);
            assert!(plan.notes.is_empty(), "{:?}", plan.notes);
        }
    }

    #[test]
    fn a_mac_that_cannot_write_to_applications_says_where_stats_went() {
        let mut found = with_stats(found_on_a_mac(Arch::Aarch64, WITH_MACS));
        found.places.applications = Some("/Users/sam/Applications".into());
        let plan = plan(&found, only(Products::STATS)).unwrap();
        assert!(
            plan.notes
                .iter()
                .any(|n| n.contains("so Noctorium Stats goes into")),
            "{:?}",
            plan.notes
        );
    }

    /// 0.12.2 as it was published: everything but Stats. What the one-line scripts meet the day they go
    /// live, and what any installer meets when it is pointed at a release from before Stats.
    const AS_0_12_2: &[&str] = &[
        "noctorium-0.7.0-1-x86_64.pkg.tar.zst",
        "Noctorium-0.7.0-macos-arm64.dmg",
        "Noctorium-0.7.0-macos-x64.dmg",
        "Noctorium-0.7.0-windows-x64-setup.exe",
        "Noctorium-0.7.0-windows-x64.msi",
        "Noctorium-0.7.0-x86_64.AppImage",
        "Noctorium-0.7.0-x86_64.flatpak",
        "Noctorium-0.7.0.apk",
        "noctorium-0.7.0.x86_64.rpm",
        "noctorium-cli-0.7.0-linux-x64.tar.gz",
        "noctorium-cli-0.7.0-macos-arm64.tar.gz",
        "noctorium-cli-0.7.0-macos-x64.tar.gz",
        "noctorium-cli-0.7.0-windows-x64.zip",
        "Noctorium-Installer-android.apk",
        "noctorium-installer-cli-linux-x64",
        "noctorium-installer-cli-macos",
        "noctorium-installer-cli-windows-x64.exe",
        "noctorium-installer-linux-x64",
        "Noctorium-Installer-windows-x64.exe",
        "Noctorium-Installer-x86_64.AppImage",
        "noctorium_0.7.0_amd64.deb",
        "SHA256SUMS.txt",
    ];

    /// Stats asked for of a release without it is left out, said so, and nothing is downloaded for it --
    /// and whatever else was asked for is planned exactly as it would have been without it.
    #[test]
    fn a_release_from_before_stats_leaves_it_out_and_installs_the_rest() {
        for (found, rest) in [
            (
                found(windows(), AS_0_12_2),
                vec![Method::WindowsMsi, Method::CliWindows],
            ),
            (
                found(
                    linux(Some(Family::Debian), Some(PackageManager::Apt)),
                    AS_0_12_2,
                ),
                vec![Method::Apt, Method::CliLinux],
            ),
            (
                found_on_a_mac(Arch::X86_64, AS_0_12_2),
                vec![Method::MacDiskImage, Method::CliMac],
            ),
        ] {
            assert_eq!(stats_asset(&found), None);

            let alone = plan(&found, only(Products::STATS)).expect("not a failure");
            assert!(alone.items.is_empty(), "{:?}", alone.items);
            assert_eq!(alone.megabytes(), 0.0, "nothing downloaded for it");
            let said = alone.notes.join("\n");
            assert!(
                said.contains("Noctorium Stats is not in v0.7.0 yet"),
                "{said}"
            );
            assert!(said.contains("noctorium-stats-<version>-"), "{said}");

            let everything = plan(&found, only(Products::ALL)).expect("the rest is planned");
            let methods: Vec<Method> = everything.items.iter().map(|i| i.method).collect();
            assert_eq!(methods, rest);
            assert!(everything
                .notes
                .iter()
                .any(|n| n.contains("Noctorium Stats is not in")));
            let without = plan(&found, only(Products::BOTH)).unwrap();
            assert_eq!(
                everything.items.len(),
                without.items.len(),
                "the same plan as if Stats had never been asked for"
            );
            assert!(!without.notes.iter().any(|n| n.contains("Stats")));
        }
    }

    #[test]
    fn stats_without_its_checksum_is_refused() {
        let mut found = with_stats(found(linux(None, None), ALL));
        found.checksums = Some(SUMS.into());
        assert!(matches!(
            plan(&found, only(Products::STATS)),
            Err(Problem::Missing(why)) if why.contains("noctorium-stats-0.7.0-linux-x64.tar.gz")
        ));
    }

    #[test]
    fn the_stats_file_for_this_machine_is_found_for_the_menus() {
        let on_windows = with_stats(found(windows(), ON_WINDOWS));
        assert_eq!(
            stats_asset(&on_windows).map(|a| a.name.as_str()),
            Some("noctorium-stats-0.7.0-windows-x64.zip")
        );
        let mac = with_stats(found_on_a_mac(Arch::Aarch64, WITH_MACS));
        assert_eq!(
            stats_asset(&mac).map(|a| a.name.as_str()),
            Some("noctorium-stats-0.7.0-macos-arm64.zip")
        );
        assert_eq!(stats_asset(&found(windows(), ON_WINDOWS)), None);
    }

    #[test]
    fn the_plan_knows_what_an_earlier_run_left_behind() {
        let here = crate::archive::tests::scratch("on-disk");
        let whole = here.join("whole.msi");
        std::fs::write(&whole, vec![0u8; 1000]).unwrap();
        assert_eq!(OnDisk::at(&whole, 1000), OnDisk::Whole);
        assert_eq!(
            OnDisk::at(&whole, 2000),
            OnDisk::Nothing,
            "a file of the wrong size is not the file"
        );
        let part = here.join("part.msi");
        std::fs::write(fetch::partial_path(&part), vec![0u8; 300]).unwrap();
        assert_eq!(OnDisk::at(&part, 1000), OnDisk::Part(300));
        assert_eq!(OnDisk::at(&part, 200), OnDisk::Nothing);
        assert_eq!(OnDisk::at(&here.join("none.msi"), 1000), OnDisk::Nothing);

        let mut plan = plan(&found(windows(), ON_WINDOWS), Options::default()).unwrap();
        plan.items[0].on_disk = OnDisk::Part(60 * 1_048_576);
        assert!((plan.megabytes_to_download() - 40.0).abs() < 0.01);
        plan.items[0].on_disk = OnDisk::Whole;
        assert_eq!(plan.megabytes_to_download(), 0.0);
    }

    // ------------------------------------------------------------ carrying it out

    use crate::fetch::tests::{Server, Serves};
    use sha2::{Digest, Sha256};

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn bytes(length: usize, seed: u8) -> Vec<u8> {
        (0..length)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    /// One product, fetched from [server] and installed by writing a profile line to [profile] -- an
    /// action that touches nothing but a file of the test's own, and fails when its folder is missing.
    fn item(
        product: Product,
        server: &Server,
        body: &[u8],
        downloads: &Path,
        profile: PathBuf,
    ) -> Item {
        let name = format!("{}.bin", product.name().replace(' ', "-"));
        Item {
            product,
            asset: Asset {
                name: name.clone(),
                url: server.url.clone(),
                size: body.len() as u64,
            },
            published: sha(body),
            // Neither has anything to check before downloading, which is all the method decides here.
            method: if product == Product::Cli {
                Method::CliLinux
            } else {
                Method::AppImage
            },
            escalation: None,
            file: downloads.join(&name),
            on_disk: OnDisk::Nothing,
            actions: vec![Action::AddToProfile { profile }],
            start: String::new(),
        }
    }

    fn plan_of(items: Vec<Item>, downloads: PathBuf) -> Plan {
        Plan {
            tag: "v0.7.0".into(),
            latest: true,
            items,
            places: Places::default(),
            downloads,
            notes: vec![],
        }
    }

    /// The steps, as a short line each, in the order they came.
    fn record(plan: &Plan, install: bool) -> (Vec<String>, Result<(), Problem>) {
        let mut steps = Vec::new();
        let mut report = |step: Step| {
            steps.push(match step {
                Step::Downloading { item, done, .. } => format!("down {item} {done}"),
                Step::Verified { item, how } => format!("checked {item} {how:?}"),
                Step::Installing { item, .. } => format!("installing {item}"),
                Step::Installed { item, .. } => format!("installed {item}"),
                Step::Failed { item, .. } => format!("failed {item}"),
            })
        };
        let outcome = if install {
            carry_out(plan, &mut report)
        } else {
            download_only(plan, &mut report)
        };
        (steps, outcome)
    }

    fn position(steps: &[String], wanted: &str) -> usize {
        steps
            .iter()
            .position(|s| s == wanted)
            .unwrap_or_else(|| panic!("no {wanted} in {steps:?}"))
    }

    /// The case the whole thing exists for: the small download does not wait for the big one, and is
    /// installed while the big one is still coming down.
    #[test]
    fn both_downloads_run_at_once_and_each_is_installed_as_soon_as_it_is_checked() {
        let here = crate::archive::tests::scratch("both-at-once");
        let downloads = here.join("downloads");
        let desktop_body = bytes(3_200_000, 1);
        let cli_body = bytes(320_000, 2);
        let desktop = Server::start(desktop_body.clone(), Serves::Slowly);
        let cli = Server::start(cli_body.clone(), Serves::Slowly);
        let plan = plan_of(
            vec![
                item(
                    Product::Desktop,
                    &desktop,
                    &desktop_body,
                    &downloads,
                    here.join("desktop"),
                ),
                item(Product::Cli, &cli, &cli_body, &downloads, here.join("cli")),
            ],
            downloads.clone(),
        );
        let (steps, outcome) = record(&plan, true);
        assert!(outcome.is_ok(), "{outcome:?}: {steps:?}");

        let desktop_started = position(&steps, "down 0 0");
        let cli_checked = position(&steps, "checked 1 Downloaded");
        let cli_installed = position(&steps, "installed 1");
        let desktop_checked = position(&steps, "checked 0 Downloaded");
        let desktop_still_coming = steps
            .iter()
            .rposition(|s| s.starts_with("down 0 "))
            .unwrap();
        assert!(
            desktop_started < cli_checked && cli_checked < desktop_still_coming,
            "the desktop was downloading while the CLI finished: {steps:?}"
        );
        assert!(
            cli_installed < desktop_checked,
            "the CLI was installed before Noctorium had come down: {steps:?}"
        );
        assert_eq!(steps.last().map(String::as_str), Some("installed 0"));
        // Installed, and their downloads cleared away.
        assert!(here.join("cli").is_file() && here.join("desktop").is_file());
        assert!(!plan.items[0].file.exists() && !plan.items[1].file.exists());
    }

    #[test]
    fn one_product_failing_does_not_stop_the_other() {
        let here = crate::archive::tests::scratch("one-fails");
        let downloads = here.join("downloads");
        let desktop_body = bytes(500_000, 3);
        let cli_body = bytes(200_000, 4);
        let desktop = Server::start(desktop_body.clone(), Serves::Ranges);
        let cli = Server::start(cli_body.clone(), Serves::Ranges);
        let mut wrong = item(
            Product::Desktop,
            &desktop,
            &desktop_body,
            &downloads,
            here.join("d"),
        );
        wrong.published = sha(b"not this");
        let plan = plan_of(
            vec![
                wrong,
                item(Product::Cli, &cli, &cli_body, &downloads, here.join("cli")),
            ],
            downloads,
        );
        let (steps, outcome) = record(&plan, true);
        assert!(
            matches!(&outcome, Err(Problem::WrongChecksum { name }) if name == "Noctorium.bin"),
            "{outcome:?}"
        );
        assert!(steps.contains(&"failed 0".to_string()), "{steps:?}");
        assert!(steps.contains(&"installed 1".to_string()), "{steps:?}");
        assert!(!steps.contains(&"installing 0".to_string()), "{steps:?}");
        assert!(here.join("cli").is_file());
    }

    #[test]
    fn a_failed_install_keeps_its_download_so_trying_again_does_not_fetch_it_again() {
        let here = crate::archive::tests::scratch("install-fails");
        let downloads = here.join("downloads");
        let body = bytes(400_000, 5);
        let server = Server::start(body.clone(), Serves::Ranges);
        // A profile in a folder that is not there, which cannot be written.
        let failing = item(
            Product::Cli,
            &server,
            &body,
            &downloads,
            here.join("missing").join("profile"),
        );
        let plan = plan_of(vec![failing], downloads);
        let (steps, outcome) = record(&plan, true);
        assert!(matches!(outcome, Err(Problem::Local(_))), "{outcome:?}");
        assert_eq!(steps.last().map(String::as_str), Some("failed 0"));
        assert!(plan.items[0].file.is_file(), "the checked download is kept");

        // Trying again, with the folder there now: nothing more is asked of the server.
        std::fs::create_dir_all(here.join("missing")).unwrap();
        let (steps, outcome) = record(&plan, true);
        assert!(outcome.is_ok(), "{outcome:?}");
        assert!(
            steps.contains(&"checked 0 AlreadyHere".to_string()),
            "{steps:?}"
        );
        assert_eq!(server.asked(), vec![None], "downloaded once, not twice");
        assert!(
            !plan.items[0].file.exists(),
            "and removed once it was installed"
        );
    }

    #[test]
    fn downloading_only_checks_everything_and_installs_nothing() {
        let here = crate::archive::tests::scratch("download-only");
        let downloads = here.join("downloads");
        let desktop_body = bytes(600_000, 6);
        let cli_body = bytes(100_000, 7);
        let desktop = Server::start(desktop_body.clone(), Serves::Ranges);
        let cli = Server::start(cli_body.clone(), Serves::Ranges);
        let plan = plan_of(
            vec![
                item(
                    Product::Desktop,
                    &desktop,
                    &desktop_body,
                    &downloads,
                    here.join("d"),
                ),
                item(Product::Cli, &cli, &cli_body, &downloads, here.join("c")),
            ],
            downloads,
        );
        let (steps, outcome) = record(&plan, false);
        assert!(outcome.is_ok(), "{outcome:?}");
        assert!(
            steps.contains(&"checked 0 Downloaded".to_string()),
            "{steps:?}"
        );
        assert!(
            steps.contains(&"checked 1 Downloaded".to_string()),
            "{steps:?}"
        );
        assert!(!steps.iter().any(|s| s.starts_with("install")), "{steps:?}");
        assert_eq!(std::fs::read(&plan.items[0].file).unwrap(), desktop_body);
        assert_eq!(std::fs::read(&plan.items[1].file).unwrap(), cli_body);
        assert!(!here.join("d").exists() && !here.join("c").exists());
    }
}
