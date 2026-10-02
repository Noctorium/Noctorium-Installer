//! What GitHub says a release is, and which of its files belongs on this machine.

use serde_json::Value;
use std::fmt;

/// One file attached to a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

impl Asset {
    pub fn megabytes(&self) -> f64 {
        self.size as f64 / 1_048_576.0
    }
}

/// A release, reduced to the two things an installer needs: what it is called and what it carries.
#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub assets: Vec<Asset>,
}

/// What can go wrong between asking GitHub and having something installed.
#[derive(Debug)]
pub enum Problem {
    /// The repository answered 404. For a private repository that is what "no release" looks like.
    NotFound,
    /// A particular release was asked for, and there is no release with that tag.
    NoSuchRelease(String),
    /// GitHub refused the request, most often because too many have come from this address.
    Refused(u16),
    /// The network did not carry it.
    Network(String),
    /// The reply was not the shape a release is.
    Unreadable(String),
    /// There is a release, but nothing in it for this machine.
    NothingForThisMachine,
    /// The release lacks the one thing that was asked for, said in full.
    Missing(String),
    /// A file arrived that is not the file the release says it is.
    WrongChecksum { name: String },
    /// The release carries no SHA256SUMS.txt, so nothing can be checked against anything.
    NoChecksums,
    /// Something on this machine refused: no permission, no disk, no package manager.
    Local(String),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::NotFound => write!(
                f,
                "GitHub has no published release for this repository.\n\
                 If the repository is private, this program cannot see it either: set GITHUB_TOKEN to a \
                 token that can read it, or download the installer from the releases page yourself."
            ),
            Problem::NoSuchRelease(tag) => write!(
                f,
                "There is no published release called {tag}. Drafts cannot be installed from; the releases \
                 page lists every version there is."
            ),
            Problem::Refused(code) => write!(
                f,
                "GitHub refused the request (HTTP {code}). If this is a rate limit, it lifts within the \
                 hour; setting GITHUB_TOKEN raises it immediately."
            ),
            Problem::Network(detail) => write!(f, "Could not reach GitHub: {detail}"),
            Problem::Unreadable(detail) => write!(f, "GitHub's answer could not be read: {detail}"),
            Problem::NothingForThisMachine => write!(
                f,
                "The release carries nothing for this machine. It may have been published for other \
                 platforms only."
            ),
            Problem::Missing(detail) => write!(f, "{detail}"),
            Problem::WrongChecksum { name } => write!(
                f,
                "{name} did not match the checksum published with it, so it has not been run. That is \
                 either a download that went wrong -- try again -- or a file that is not what the \
                 release says it is."
            ),
            Problem::NoChecksums => write!(
                f,
                "The release has no SHA256SUMS.txt, so nothing downloaded from it can be checked. \
                 Refusing to install something unverified."
            ),
            Problem::Local(detail) => write!(f, "{detail}"),
        }
    }
}

/// The processor a file is built for.
///
/// Every release so far is x86-64 only, but the names already say so -- `amd64` in a .deb, `x86_64` in an
/// rpm, `x64` on Windows -- and matching on that word is what stops an ARM machine, the day there are ARM
/// builds beside these, from earnestly downloading one it cannot run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    /// What this program was built for, which is what it is running on.
    pub fn this_machine() -> Option<Arch> {
        Arch::from_name(std::env::consts::ARCH)
    }

    /// Reads the names Rust, uname and the package formats use.
    pub fn from_name(name: &str) -> Option<Arch> {
        match name {
            "x86_64" | "amd64" | "x64" => Some(Arch::X86_64),
            "aarch64" | "arm64" => Some(Arch::Aarch64),
            _ => None,
        }
    }

    /// As a person says it.
    pub fn describe(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86-64",
            Arch::Aarch64 => "ARM64",
        }
    }

    /// Debian's word.
    fn debian(self) -> &'static str {
        match self {
            Arch::X86_64 => "amd64",
            Arch::Aarch64 => "arm64",
        }
    }

    /// Windows' word, which the Noctorium CLI's archives use on both systems.
    fn windows(self) -> &'static str {
        match self {
            Arch::X86_64 => "x64",
            Arch::Aarch64 => "arm64",
        }
    }

    /// The kernel's word, which rpm, pacman, AppImage and Flatpak all use.
    fn kernel(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

/// Which file of a release this machine wants.
///
/// Windows takes the `.exe`, which is the installer most people expect to double click; the `.msi` is
/// left for whoever is deploying it by hand. Linux takes whichever format was chosen -- the caller decides
/// that from the distribution and from what the person asked for, rather than this guessing from names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    /// `Noctorium-<version>-windows-x64-setup.exe`
    WindowsSetup,
    /// `noctorium_<version>_amd64.deb`
    DebianPackage,
    /// `noctorium-<version>.x86_64.rpm`, for Fedora, RHEL and openSUSE alike.
    RpmPackage,
    /// `noctorium-<version>-1-x86_64.pkg.tar.zst`
    ArchPackage,
    /// `Noctorium-<version>-x86_64.AppImage`
    AppImage,
    /// `Noctorium-<version>-x86_64.flatpak`
    Flatpak,
    /// `noctorium-cli-<version>-windows-x64.zip`, the terminal player.
    CliWindows,
    /// `noctorium-cli-<version>-linux-x64.tar.gz`
    CliLinux,
}

impl Wanted {
    /// What comes before the version and what comes after it, lowercased, for this architecture.
    fn shape(self, arch: Arch) -> (&'static str, String) {
        match self {
            Wanted::WindowsSetup => (
                "noctorium-",
                format!("-windows-{}-setup.exe", arch.windows()),
            ),
            Wanted::DebianPackage => ("noctorium_", format!("_{}.deb", arch.debian())),
            Wanted::RpmPackage => ("noctorium-", format!(".{}.rpm", arch.kernel())),
            Wanted::ArchPackage => ("noctorium-", format!("-{}.pkg.tar.zst", arch.kernel())),
            Wanted::AppImage => ("noctorium-", format!("-{}.appimage", arch.kernel())),
            Wanted::Flatpak => ("noctorium-", format!("-{}.flatpak", arch.kernel())),
            Wanted::CliWindows => ("noctorium-cli-", format!("-windows-{}.zip", arch.windows())),
            // The Noctorium CLI says `x64` on Linux too, like its Windows archive, rather than either of
            // the Linux words.
            Wanted::CliLinux => (
                "noctorium-cli-",
                format!("-linux-{}.tar.gz", arch.windows()),
            ),
        }
    }

    /// The name this looks for, as somebody reads it: `noctorium_<version>_amd64.deb`.
    pub fn pattern(self, arch: Arch) -> String {
        let (before, after) = self.shape(arch);
        let after = match self {
            // Lowercased to compare; written the way the release writes them.
            Wanted::AppImage => after.replace(".appimage", ".AppImage"),
            // The package release, which is part of what this reads as the version.
            Wanted::ArchPackage => format!("-1{after}"),
            _ => after,
        };
        let before = match self {
            Wanted::WindowsSetup | Wanted::AppImage | Wanted::Flatpak => "Noctorium-",
            _ => before,
        };
        format!("{before}<version>{after}")
    }

    pub fn matches(self, name: &str, arch: Arch) -> bool {
        let lower = name.to_ascii_lowercase();
        // This program is published to the same release as the application it installs, so anything
        // named after the installer is skipped. Without this, a machine could earnestly download the
        // installer and install it with itself.
        if lower.contains("installer") {
            return false;
        }
        let (before, after) = self.shape(arch);
        let Some(version) = lower
            .strip_prefix(before)
            .and_then(|rest| rest.strip_suffix(after.as_str()))
        else {
            return false;
        };
        // A version starts with a digit. That one rule is what keeps `noctorium-cli-1.0.0-...` from
        // answering for the desktop's `noctorium-<version>-...`, and a signature or a checksum file beside
        // a package from being mistaken for it -- their names do not end where a package's does.
        version.starts_with(|c: char| c.is_ascii_digit()) && !version.contains('/')
    }
}

impl Release {
    /// Reads the release JSON GitHub's API answers with.
    pub fn from_json(body: &str) -> Result<Release, Problem> {
        let root: Value =
            serde_json::from_str(body).map_err(|e| Problem::Unreadable(e.to_string()))?;
        let tag = root
            .get("tag_name")
            .and_then(Value::as_str)
            .ok_or_else(|| Problem::Unreadable("it names no tag".into()))?
            .to_string();
        let assets = root
            .get("assets")
            .and_then(Value::as_array)
            .ok_or_else(|| Problem::Unreadable("it lists no files".into()))?
            .iter()
            .filter_map(|asset| {
                Some(Asset {
                    name: asset.get("name")?.as_str()?.to_string(),
                    url: asset.get("browser_download_url")?.as_str()?.to_string(),
                    size: asset.get("size").and_then(Value::as_u64).unwrap_or(0),
                })
            })
            .collect();
        Ok(Release { tag, assets })
    }

    /// The file this machine should install, or nothing if the release has none.
    pub fn asset_for(&self, wanted: Wanted, arch: Arch) -> Option<&Asset> {
        self.assets
            .iter()
            .find(|asset| wanted.matches(&asset.name, arch))
    }

    pub fn checksums(&self) -> Option<&Asset> {
        self.assets
            .iter()
            .find(|asset| asset.name == "SHA256SUMS.txt")
    }

    /// The version as somebody reads it, without the tag's leading v.
    pub fn version(&self) -> &str {
        self.tag.strip_prefix('v').unwrap_or(&self.tag)
    }
}

/// The checksum published for one file, read out of a `sha256sum` listing.
///
/// The format is the one coreutils writes: a hash, two spaces, a name. Anything else on the line is
/// somebody else's file, and a line that does not parse is skipped rather than failing the lot -- the
/// only line that matters is the one naming the file being installed.
pub fn published_checksum(listing: &str, name: &str) -> Option<String> {
    listing.lines().find_map(|line| {
        let (hash, file) = line.split_once("  ")?;
        let hash = hash.trim();
        if file.trim() != name || hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        Some(hash.to_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: &str = r#"{
      "tag_name": "v0.7.0",
      "assets": [
        {"name": "Noctorium-0.7.0-windows-x64-setup.exe", "browser_download_url": "https://example.test/setup.exe", "size": 340636160},
        {"name": "Noctorium-0.7.0-windows-x64.msi", "browser_download_url": "https://example.test/x.msi", "size": 339905750},
        {"name": "Noctorium-0.7.0.apk", "browser_download_url": "https://example.test/x.apk", "size": 17600515},
        {"name": "noctorium_0.7.0_amd64.deb", "browser_download_url": "https://example.test/x.deb", "size": 251486386},
        {"name": "noctorium-0.7.0.x86_64.rpm", "browser_download_url": "https://example.test/x.rpm", "size": 264015322},
        {"name": "noctorium-0.7.0-1-x86_64.pkg.tar.zst", "browser_download_url": "https://example.test/x.zst", "size": 250000000},
        {"name": "Noctorium-0.7.0-x86_64.AppImage", "browser_download_url": "https://example.test/x.AppImage", "size": 260000000},
        {"name": "Noctorium-0.7.0-x86_64.flatpak", "browser_download_url": "https://example.test/x.flatpak", "size": 270000000},
        {"name": "noctorium-cli-0.7.0-windows-x64.zip", "browser_download_url": "https://example.test/cli.zip", "size": 60000000},
        {"name": "noctorium-cli-0.7.0-linux-x64.tar.gz", "browser_download_url": "https://example.test/cli.tgz", "size": 61000000},
        {"name": "Noctorium-Installer-windows-x64.exe", "browser_download_url": "u", "size": 1},
        {"name": "noctorium-installer-cli-windows-x64.exe", "browser_download_url": "u", "size": 1},
        {"name": "noctorium-installer-linux-x64", "browser_download_url": "u", "size": 1},
        {"name": "noctorium-installer-cli-linux-x64", "browser_download_url": "u", "size": 1},
        {"name": "Noctorium-Installer-x86_64.AppImage", "browser_download_url": "u", "size": 1},
        {"name": "SHA256SUMS.txt", "browser_download_url": "https://example.test/SHA256SUMS.txt", "size": 473}
      ]
    }"#;

    fn picked(release: &Release, wanted: Wanted, arch: Arch) -> Option<&str> {
        release.asset_for(wanted, arch).map(|a| a.name.as_str())
    }

    #[test]
    fn reads_the_tag_and_every_file() {
        let release = Release::from_json(RELEASE).expect("should parse");
        assert_eq!(release.tag, "v0.7.0");
        assert_eq!(release.version(), "0.7.0");
        assert_eq!(release.assets.len(), 16);
    }

    #[test]
    fn every_format_finds_its_own_file() {
        let release = Release::from_json(RELEASE).expect("should parse");
        let x64 = Arch::X86_64;
        assert_eq!(
            picked(&release, Wanted::WindowsSetup, x64),
            Some("Noctorium-0.7.0-windows-x64-setup.exe"),
            "the .msi is for deployment, not for double clicking"
        );
        assert_eq!(
            picked(&release, Wanted::DebianPackage, x64),
            Some("noctorium_0.7.0_amd64.deb")
        );
        assert_eq!(
            picked(&release, Wanted::RpmPackage, x64),
            Some("noctorium-0.7.0.x86_64.rpm")
        );
        assert_eq!(
            picked(&release, Wanted::ArchPackage, x64),
            Some("noctorium-0.7.0-1-x86_64.pkg.tar.zst")
        );
        assert_eq!(
            picked(&release, Wanted::AppImage, x64),
            Some("Noctorium-0.7.0-x86_64.AppImage"),
            "the installer's own AppImage is never the application's"
        );
        assert_eq!(
            picked(&release, Wanted::Flatpak, x64),
            Some("Noctorium-0.7.0-x86_64.flatpak")
        );
        assert_eq!(
            picked(&release, Wanted::CliWindows, x64),
            Some("noctorium-cli-0.7.0-windows-x64.zip")
        );
        assert_eq!(
            picked(&release, Wanted::CliLinux, x64),
            Some("noctorium-cli-0.7.0-linux-x64.tar.gz")
        );
        assert_eq!(
            release.checksums().map(|a| a.name.as_str()),
            Some("SHA256SUMS.txt")
        );
    }

    /// Everything published so far is x86-64. An ARM machine must find nothing rather than something.
    #[test]
    fn an_arm_machine_takes_nothing_built_for_x86() {
        let release = Release::from_json(RELEASE).expect("should parse");
        for wanted in [
            Wanted::WindowsSetup,
            Wanted::DebianPackage,
            Wanted::RpmPackage,
            Wanted::ArchPackage,
            Wanted::AppImage,
            Wanted::Flatpak,
            Wanted::CliWindows,
            Wanted::CliLinux,
        ] {
            assert_eq!(
                picked(&release, wanted, Arch::Aarch64),
                None,
                "{wanted:?} picked an x86-64 file for ARM"
            );
        }
    }

    #[test]
    fn each_architecture_reads_its_own_word_in_each_format() {
        let arm = Arch::Aarch64;
        assert!(Wanted::DebianPackage.matches("noctorium_1.0.0_arm64.deb", arm));
        assert!(Wanted::RpmPackage.matches("noctorium-1.0.0.aarch64.rpm", arm));
        assert!(Wanted::ArchPackage.matches("noctorium-1.0.0-1-aarch64.pkg.tar.zst", arm));
        assert!(Wanted::AppImage.matches("Noctorium-1.0.0-aarch64.AppImage", arm));
        assert!(Wanted::Flatpak.matches("Noctorium-1.0.0-aarch64.flatpak", arm));
        assert!(Wanted::WindowsSetup.matches("Noctorium-1.0.0-windows-arm64-setup.exe", arm));
        assert!(Wanted::CliWindows.matches("noctorium-cli-1.0.0-windows-arm64.zip", arm));
        assert!(Wanted::CliLinux.matches("noctorium-cli-1.0.0-linux-arm64.tar.gz", arm));

        // And the other way round: an x86-64 machine is not handed an ARM file because the rest of the
        // name fits.
        let x64 = Arch::X86_64;
        assert!(!Wanted::DebianPackage.matches("noctorium_1.0.0_arm64.deb", x64));
        assert!(!Wanted::RpmPackage.matches("noctorium-1.0.0.aarch64.rpm", x64));
        assert!(!Wanted::AppImage.matches("Noctorium-1.0.0-aarch64.AppImage", x64));
        assert!(!Wanted::CliLinux.matches("noctorium-cli-1.0.0-linux-arm64.tar.gz", x64));
    }

    #[test]
    fn architecture_names_are_read_in_every_spelling() {
        assert_eq!(Arch::from_name("x86_64"), Some(Arch::X86_64));
        assert_eq!(Arch::from_name("amd64"), Some(Arch::X86_64));
        assert_eq!(Arch::from_name("x64"), Some(Arch::X86_64));
        assert_eq!(Arch::from_name("aarch64"), Some(Arch::Aarch64));
        assert_eq!(Arch::from_name("arm64"), Some(Arch::Aarch64));
        assert_eq!(Arch::from_name("riscv64"), None);
    }

    /// The desktop's names and the CLI's share a prefix, and neither may answer for the other.
    #[test]
    fn the_desktop_and_the_terminal_player_are_never_confused() {
        let x64 = Arch::X86_64;
        assert!(!Wanted::AppImage.matches("noctorium-cli-1.0.0-x86_64.AppImage", x64));
        assert!(!Wanted::ArchPackage.matches("noctorium-cli-1.0.0-1-x86_64.pkg.tar.zst", x64));
        assert!(!Wanted::RpmPackage.matches("noctorium-cli-1.0.0.x86_64.rpm", x64));
        assert!(!Wanted::CliLinux.matches("noctorium-1.0.0-linux-x64.tar.gz", x64));
    }

    #[test]
    fn files_beside_a_package_are_not_the_package() {
        let x64 = Arch::X86_64;
        assert!(!Wanted::ArchPackage.matches("noctorium-1.0.0-1-x86_64.pkg.tar.zst.sig", x64));
        assert!(!Wanted::AppImage.matches("Noctorium-1.0.0-x86_64.AppImage.zsync", x64));
        assert!(
            !Wanted::DebianPackage.matches("noctorium__amd64.deb", x64),
            "no version"
        );
        assert!(!Wanted::WindowsSetup.matches("Noctorium-1.0.0-windows-x64.msi", x64));
    }

    #[test]
    fn the_patterns_read_the_way_the_release_names_its_files() {
        let x64 = Arch::X86_64;
        assert_eq!(
            Wanted::WindowsSetup.pattern(x64),
            "Noctorium-<version>-windows-x64-setup.exe"
        );
        assert_eq!(
            Wanted::DebianPackage.pattern(x64),
            "noctorium_<version>_amd64.deb"
        );
        assert_eq!(
            Wanted::ArchPackage.pattern(x64),
            "noctorium-<version>-1-x86_64.pkg.tar.zst"
        );
        assert_eq!(
            Wanted::AppImage.pattern(x64),
            "Noctorium-<version>-x86_64.AppImage"
        );
        assert_eq!(
            Wanted::CliLinux.pattern(x64),
            "noctorium-cli-<version>-linux-x64.tar.gz"
        );
    }

    #[test]
    fn a_release_without_this_platform_says_so_rather_than_installing_the_wrong_thing() {
        let android_only = r#"{"tag_name":"v9","assets":[{"name":"Noctorium-9.apk","browser_download_url":"u","size":1}]}"#;
        let release = Release::from_json(android_only).expect("should parse");
        assert!(release
            .asset_for(Wanted::WindowsSetup, Arch::X86_64)
            .is_none());
        assert!(release
            .asset_for(Wanted::DebianPackage, Arch::X86_64)
            .is_none());
        assert!(release.checksums().is_none());
    }

    /// The installer is attached to the same release as the application, so it has to skip itself.
    #[test]
    fn the_installer_never_picks_itself() {
        let x64 = Arch::X86_64;
        for name in [
            "Noctorium-Installer-windows-x64.exe",
            "noctorium-installer-cli-windows-x64.exe",
            "noctorium-installer-linux-x64",
            "noctorium-installer-cli-linux-x64",
            "Noctorium-Installer-x86_64.AppImage",
            "Noctorium-Installer-1.0.0-x86_64.AppImage",
        ] {
            for wanted in [
                Wanted::WindowsSetup,
                Wanted::DebianPackage,
                Wanted::AppImage,
                Wanted::CliWindows,
                Wanted::CliLinux,
            ] {
                assert!(!wanted.matches(name, x64), "{wanted:?} matched {name}");
            }
        }
    }

    #[test]
    fn a_reply_that_is_not_a_release_is_refused() {
        assert!(matches!(
            Release::from_json("not json"),
            Err(Problem::Unreadable(_))
        ));
        assert!(matches!(
            Release::from_json(r#"{"message":"Not Found"}"#),
            Err(Problem::Unreadable(_))
        ));
    }

    #[test]
    fn the_checksum_is_found_by_name_and_nothing_else() {
        let listing = "\
1b6e64b90cdcf2633a69eb5cbca74e1c66b60270fcd57772d0ab205183653d3e  Noctorium-0.4.0-windows-x64-setup.exe
342b82f3ccbeb41e4b2890b0b1df0dae85389f481b4405c3bfb9ed0b5cef8432  Noctorium-0.4.0-windows-x64.msi
";
        assert_eq!(
            published_checksum(listing, "Noctorium-0.4.0-windows-x64-setup.exe").as_deref(),
            Some("1b6e64b90cdcf2633a69eb5cbca74e1c66b60270fcd57772d0ab205183653d3e")
        );
        // The .msi's line must not answer for the .exe, and a file with no line has no checksum.
        assert_eq!(published_checksum(listing, "Noctorium-0.4.0.apk"), None);
    }

    #[test]
    fn a_listing_that_is_not_one_yields_nothing() {
        assert_eq!(published_checksum("", "x"), None);
        assert_eq!(
            published_checksum("garbage  x", "x"),
            None,
            "a hash that is not one is not a hash"
        );
        assert_eq!(
            published_checksum(
                "zzzz64b90cdcf2633a69eb5cbca74e1c66b60270fcd57772d0ab205183653d3e  x",
                "x"
            ),
            None,
            "not every 64-character string is hexadecimal"
        );
    }
}
