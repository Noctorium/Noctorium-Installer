//! What GitHub says the latest release is, and which of its files belongs on this machine.

use serde_json::Value;
use std::fmt;

/// One file attached to a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// A release, reduced to the two things an installer needs: what it is called and what it carries.
#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub assets: Vec<Asset>,
}

/// What can go wrong between asking GitHub and having something to install.
#[derive(Debug)]
pub enum Problem {
    /// The repository answered 404. For a private repository that is what "no release" looks like.
    NotFound,
    /// GitHub refused the request, most often because too many have come from this address.
    Refused(u16),
    /// The network did not carry it.
    Network(String),
    /// The reply was not the shape a release is.
    Unreadable(String),
    /// There is a release, but nothing in it for this machine.
    NothingForThisMachine,
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
            Problem::Refused(code) => write!(
                f,
                "GitHub refused the request (HTTP {code}). If this is a rate limit, it lifts within the \
                 hour; setting GITHUB_TOKEN raises it immediately."
            ),
            Problem::Network(detail) => write!(f, "Could not reach GitHub: {detail}"),
            Problem::Unreadable(detail) => write!(f, "GitHub's answer could not be read: {detail}"),
            Problem::NothingForThisMachine => write!(
                f,
                "The latest release carries nothing for this machine. It may have been published for \
                 other platforms only."
            ),
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

/// Which file of a release this machine wants.
///
/// Windows takes the `.exe`, which is the installer most people expect to double click; the `.msi` is
/// left for whoever is deploying it by hand. Linux takes whichever package its package manager speaks,
/// which is why the caller passes that in rather than this guessing from the file names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    WindowsSetup,
    DebianPackage,
    FedoraPackage,
}

impl Wanted {
    fn matches(self, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        // This program is published to the same release as the application it installs, so anything
        // named after the installer is skipped. Without this, a machine could earnestly download the
        // installer and install it with itself.
        if lower.contains("installer") {
            return false;
        }
        match self {
            // Not the .msi, and not any other .exe that might be attached later.
            Wanted::WindowsSetup => lower.ends_with("-setup.exe"),
            Wanted::DebianPackage => lower.ends_with(".deb"),
            Wanted::FedoraPackage => lower.ends_with(".rpm"),
        }
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
    pub fn asset_for(&self, wanted: Wanted) -> Option<&Asset> {
        self.assets.iter().find(|asset| wanted.matches(&asset.name))
    }

    pub fn checksums(&self) -> Option<&Asset> {
        self.assets
            .iter()
            .find(|asset| asset.name == "SHA256SUMS.txt")
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
      "tag_name": "v0.4.0",
      "assets": [
        {"name": "Noctorium-0.4.0-windows-x64-setup.exe", "browser_download_url": "https://example.test/setup.exe", "size": 340636160},
        {"name": "Noctorium-0.4.0-windows-x64.msi", "browser_download_url": "https://example.test/x.msi", "size": 339905750},
        {"name": "Noctorium-0.4.0.apk", "browser_download_url": "https://example.test/x.apk", "size": 17600515},
        {"name": "noctorium_0.4.0_amd64.deb", "browser_download_url": "https://example.test/x.deb", "size": 251486386},
        {"name": "noctorium-0.4.0.x86_64.rpm", "browser_download_url": "https://example.test/x.rpm", "size": 264015322},
        {"name": "SHA256SUMS.txt", "browser_download_url": "https://example.test/SHA256SUMS.txt", "size": 473}
      ]
    }"#;

    #[test]
    fn reads_the_tag_and_every_file() {
        let release = Release::from_json(RELEASE).expect("should parse");
        assert_eq!(release.tag, "v0.4.0");
        assert_eq!(release.assets.len(), 6);
    }

    #[test]
    fn each_platform_gets_its_own_file() {
        let release = Release::from_json(RELEASE).expect("should parse");
        assert_eq!(
            release
                .asset_for(Wanted::WindowsSetup)
                .map(|a| a.name.as_str()),
            Some("Noctorium-0.4.0-windows-x64-setup.exe"),
            "the .msi is for deployment, not for double clicking"
        );
        assert_eq!(
            release
                .asset_for(Wanted::DebianPackage)
                .map(|a| a.name.as_str()),
            Some("noctorium_0.4.0_amd64.deb")
        );
        assert_eq!(
            release
                .asset_for(Wanted::FedoraPackage)
                .map(|a| a.name.as_str()),
            Some("noctorium-0.4.0.x86_64.rpm")
        );
        assert_eq!(
            release.checksums().map(|a| a.name.as_str()),
            Some("SHA256SUMS.txt")
        );
    }

    #[test]
    fn a_release_without_this_platform_says_so_rather_than_installing_the_wrong_thing() {
        let android_only = r#"{"tag_name":"v9","assets":[{"name":"Noctorium-9.apk","browser_download_url":"u","size":1}]}"#;
        let release = Release::from_json(android_only).expect("should parse");
        assert!(release.asset_for(Wanted::WindowsSetup).is_none());
        assert!(release.asset_for(Wanted::DebianPackage).is_none());
        assert!(release.checksums().is_none());
    }

    /// The installer is attached to the same release as the application, so it has to skip itself.
    #[test]
    fn the_installer_never_picks_itself() {
        let with_installers = r#"{
          "tag_name": "v1.0.0",
          "assets": [
            {"name": "Noctorium-Installer-windows-x64.exe", "browser_download_url": "u", "size": 1},
            {"name": "noctorium-installer-linux-x64", "browser_download_url": "u", "size": 1},
            {"name": "Noctorium-1.0.0-windows-x64-setup.exe", "browser_download_url": "u", "size": 2},
            {"name": "noctorium_1.0.0_amd64.deb", "browser_download_url": "u", "size": 2}
          ]
        }"#;
        let release = Release::from_json(with_installers).expect("should parse");
        assert_eq!(
            release
                .asset_for(Wanted::WindowsSetup)
                .map(|a| a.name.as_str()),
            Some("Noctorium-1.0.0-windows-x64-setup.exe")
        );
        assert_eq!(
            release
                .asset_for(Wanted::DebianPackage)
                .map(|a| a.name.as_str()),
            Some("noctorium_1.0.0_amd64.deb")
        );
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
