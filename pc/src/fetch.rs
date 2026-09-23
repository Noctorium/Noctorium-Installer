//! Asking GitHub for things, and writing one of them to disk while saying how far it has got.

use crate::github::Problem;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Sent on every request. GitHub refuses a request with no user agent outright.
const AGENT: &str = concat!("noctorium-installer/", env!("CARGO_PKG_VERSION"));

/// The repository releases are published to. Overridable so a fork, or a test, can point elsewhere.
pub fn repository() -> String {
    std::env::var("NOCTORIUM_REPOSITORY")
        .unwrap_or_else(|_| "Noctorium/Noctorium-Installer".to_string())
}

fn request(url: &str) -> ureq::Request {
    let mut request = ureq::get(url).set("User-Agent", AGENT);
    // Only ever read from the environment, never stored: this is how the installer reaches a private
    // repository, and how somebody who has been rate limited gets past it.
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.trim().is_empty() {
            request = request.set("Authorization", &format!("Bearer {}", token.trim()));
        }
    }
    request
}

fn send(url: &str) -> Result<ureq::Response, Problem> {
    match request(url).call() {
        Ok(response) => Ok(response),
        Err(ureq::Error::Status(404, _)) => Err(Problem::NotFound),
        Err(ureq::Error::Status(code, _)) => Err(Problem::Refused(code)),
        Err(ureq::Error::Transport(transport)) => Err(Problem::Network(transport.to_string())),
    }
}

/// The body of the latest release, as GitHub's API gives it.
///
/// `/releases/latest` ignores drafts and pre-releases, which is the behaviour wanted here: a draft is a
/// release its author is still looking at.
pub fn latest_release() -> Result<String, Problem> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        repository()
    );
    send(&url)?
        .into_string()
        .map_err(|e| Problem::Network(e.to_string()))
}

/// A small file, read whole. Used for the checksums.
pub fn text(url: &str) -> Result<String, Problem> {
    send(url)?
        .into_string()
        .map_err(|e| Problem::Network(e.to_string()))
}

/// Downloads [url] into [directory] under [name], reporting progress, and returns where it landed
/// together with what it hashes to.
///
/// The hash is computed while the bytes go past rather than by reading the file again afterwards: these
/// are three hundred megabyte files and reading them twice on a slow disk is a minute nobody needs to
/// wait.
pub fn download(
    url: &str,
    directory: &Path,
    name: &str,
    expected_size: u64,
) -> Result<(PathBuf, String), Problem> {
    let destination = directory.join(name);
    let mut file = std::fs::File::create(&destination).map_err(|e| {
        Problem::Local(format!("Could not write to {}: {e}", destination.display()))
    })?;

    let response = send(url)?;
    let total = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(expected_size);

    let mut reader = response.into_reader();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 128 * 1024];
    let mut written: u64 = 0;
    let mut last_report = 0u64;

    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|e| Problem::Network(format!("the download stopped early: {e}")))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read]).map_err(|e| {
            Problem::Local(format!("Could not write to {}: {e}", destination.display()))
        })?;
        written += read as u64;

        // Once a megabyte rather than once a buffer: a progress line that redraws two thousand times a
        // second is its own slowdown.
        if written - last_report >= 1_048_576 {
            last_report = written;
            report(written, total);
        }
    }
    report(written, total);
    println!();

    file.flush().map_err(|e| {
        Problem::Local(format!(
            "Could not finish writing {}: {e}",
            destination.display()
        ))
    })?;
    Ok((destination, hex(&hasher.finalize())))
}

fn report(written: u64, total: u64) {
    let done = written as f64 / 1_048_576.0;
    if total > 0 {
        let percent = (written as f64 / total as f64 * 100.0).min(100.0);
        print!(
            "\r  {:.0}% of {:.0} MB",
            percent,
            total as f64 / 1_048_576.0
        );
    } else {
        print!("\r  {done:.0} MB");
    }
    let _ = std::io::stdout().flush();
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repository_can_be_pointed_elsewhere_but_defaults_to_ours() {
        // Read at call time rather than once, so a test can set it; the default is the real one.
        std::env::remove_var("NOCTORIUM_REPOSITORY");
        assert_eq!(repository(), "Noctorium/Noctorium-Installer");
        std::env::set_var("NOCTORIUM_REPOSITORY", "someone/else");
        assert_eq!(repository(), "someone/else");
        std::env::remove_var("NOCTORIUM_REPOSITORY");
    }

    #[test]
    fn hashes_are_written_the_way_sha256sum_writes_them() {
        // The empty string's SHA-256, which is the one hash everybody can check by eye.
        let digest = Sha256::digest(b"");
        assert_eq!(
            hex(&digest),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
