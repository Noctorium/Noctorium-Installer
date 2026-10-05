//! Asking GitHub for things, and writing one of them to disk while saying how far it has got.
//!
//! Every request goes through one agent, so the connection to github.com that fetched the checksums is the
//! one the downloads start on, and a second download to the same CDN resumes the TLS session of the first
//! rather than negotiating a new one. Two downloads at once, which is what installing both products is,
//! each have a connection of their own from the same pool.
//!
//! A download is never written under its own name until it is whole and has hashed to what the release
//! says it should. Until then it is `<name>.partial`, which nothing else in this program will run, and
//! which a later attempt picks up from where it stopped.

use crate::github::Problem;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// Sent on every request. GitHub refuses a request with no user agent outright.
const AGENT: &str = concat!("noctorium-installer/", env!("CARGO_PKG_VERSION"));

/// How many times one download is asked for, when the connection breaks part of the way through it.
/// Each attempt after the first asks only for what is still missing, so a flaky connection costs the
/// bytes it dropped and not the whole file again.
const ATTEMPTS: usize = 3;

/// The repository releases are published to. Overridable so a fork, or a test, can point elsewhere.
pub fn repository() -> String {
    std::env::var("NOCTORIUM_REPOSITORY")
        .unwrap_or_else(|_| "Noctorium/Noctorium-Installer".to_string())
}

/// The one agent every request is made with, built the first time it is wanted.
///
/// `ureq::get` builds a new agent for every call, which means a new TLS configuration, a new pool, and a
/// new handshake every time; this keeps one for the life of the program. It also gives every request a
/// deadline to connect and to go on hearing from the other end. Without one, a connection that goes quiet
/// part of the way through three hundred megabytes waits forever, with the progress bar stopped where it
/// was; with one, it fails after a minute of silence, and the download carries on from where it got to.
fn agent() -> &'static ureq::Agent {
    static SHARED: OnceLock<ureq::Agent> = OnceLock::new();
    SHARED.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(30))
            .timeout_read(Duration::from_secs(60))
            .build()
    })
}

fn request(url: &str) -> ureq::Request {
    let mut request = agent().get(url).set("User-Agent", AGENT);
    // Only ever read from the environment, never stored: this is how the installer reaches a private
    // repository, and how somebody who has been rate limited gets past it. ureq drops it when GitHub
    // redirects a download to its CDN, which would refuse a request that carried it.
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.trim().is_empty() {
            request = request.set("Authorization", &format!("Bearer {}", token.trim()));
        }
    }
    request
}

fn problem(error: ureq::Error) -> Problem {
    match error {
        ureq::Error::Status(404, _) => Problem::NotFound,
        ureq::Error::Status(code, _) => Problem::Refused(code),
        ureq::Error::Transport(transport) => Problem::Network(transport.to_string()),
    }
}

fn send(url: &str) -> Result<ureq::Response, Problem> {
    request(url).call().map_err(problem)
}

/// The body of a release, as GitHub's API gives it: the latest, or the one tagged [tag].
///
/// `/releases/latest` ignores drafts and pre-releases, which is the behaviour wanted here: a draft is a
/// release its author is still looking at. A tag asked for by name can be a pre-release -- somebody who
/// types `--version 1.2.0-beta.1` means it -- but never a draft, which GitHub does not serve by tag.
pub fn release(tag: Option<&str>) -> Result<String, Problem> {
    let url = match tag {
        None => format!(
            "https://api.github.com/repos/{}/releases/latest",
            repository()
        ),
        Some(tag) => format!(
            "https://api.github.com/repos/{}/releases/tags/{tag}",
            repository()
        ),
    };
    match send(&url) {
        // The repository exists -- it answered for the latest release a moment ago, or would -- so a 404
        // for one tag means that tag, and saying "no release at all" would send somebody the wrong way.
        Err(Problem::NotFound) if tag.is_some() => {
            Err(Problem::NoSuchRelease(tag.unwrap_or_default().to_string()))
        }
        other => other?
            .into_string()
            .map_err(|e| Problem::Network(e.to_string())),
    }
}

/// A small file, read whole. Used for the checksums.
pub fn text(url: &str) -> Result<String, Problem> {
    send(url)?
        .into_string()
        .map_err(|e| Problem::Network(e.to_string()))
}

/// How a download came to be on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetched {
    /// Asked for whole, this time.
    Downloaded,
    /// Part of it was there already, left by an earlier run that stopped, and only the rest was asked
    /// for. [from] is how much was there.
    Resumed { from: u64 },
    /// The whole of it was there already, from an earlier run, and hashed to what the release says.
    AlreadyHere,
}

/// Where a download is written until it is whole and checked: beside where it is going, so the last step
/// is a rename on one disk rather than a copy.
pub fn partial_path(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".partial");
    destination.with_file_name(name)
}

/// Where a download split into ranges keeps a note of how far each range has got, so that the next run
/// can carry on from them: beside the `.partial`, and removed with it.
pub fn ranges_path(destination: &Path) -> PathBuf {
    let mut name = partial_path(destination)
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".ranges");
    destination.with_file_name(name)
}

/// How many connections one big download is split across.
///
/// Measured on a home connection, GitHub's release CDN gave each connection about 2 MB/s however fast
/// the line was: the 60 MB Noctorium CLI took 24 to 30 seconds over one connection, 15 to 18 over two,
/// 7.6 to 9.1 over four and 4.4 over eight, three times each. Four is most of that, and no more than a
/// browser opens to one host -- and installing both products is two downloads of four.
pub const CONNECTIONS: u64 = 4;

/// Below this a file comes down in one piece. A few seconds on any connection, and splitting it would
/// spend most of them asking.
pub const SPLIT_FROM: u64 = 16 * 1_048_576;

/// Puts the release's file [name] in [directory], checked against [published], and says where it is and
/// how it got there -- calling [progress] with the bytes so far and the total expected. A big file comes
/// down over [CONNECTIONS] at once; see [download_with].
pub fn download(
    url: &str,
    directory: &Path,
    name: &str,
    expected_size: u64,
    published: &str,
    progress: impl FnMut(u64, u64),
) -> Result<(PathBuf, Fetched), Problem> {
    let connections = if expected_size >= SPLIT_FROM {
        CONNECTIONS
    } else {
        1
    };
    download_with(
        url,
        directory,
        name,
        expected_size,
        published,
        connections,
        progress,
    )
}

/// [download], over as many as [connections] at once.
///
/// Nothing is fetched that is already here. A file of that name from an earlier run that hashes to the
/// published checksum is used as it is; one that does not is not that file, and is removed. A
/// `.partial` an earlier run left is kept, and only the rest of the file asked for, with Range requests,
/// which GitHub's CDN honours; a server that answers with the whole file instead is taken at its word and
/// the file started again. Either way the checksum is of the whole file, every byte of it, and a resumed
/// download that fails it is tried once more from nothing, in case what was left behind was never part
/// of this file at all.
///
/// Over one connection the hash is computed while the bytes go past. Over several they arrive out of
/// order, so the file is read back once it is whole -- a fifth of a second for three hundred megabytes,
/// most of it still in memory, against the half a minute or more the extra connections save.
pub fn download_with(
    url: &str,
    directory: &Path,
    name: &str,
    expected_size: u64,
    published: &str,
    connections: u64,
    mut progress: impl FnMut(u64, u64),
) -> Result<(PathBuf, Fetched), Problem> {
    let destination = directory.join(name);
    let partial = partial_path(&destination);
    let record = ranges_path(&destination);
    let local = |what: &str, path: &Path, e: std::io::Error| {
        Problem::Local(format!("Could not {what} {}: {e}", path.display()))
    };

    if let Ok(found) = fs::metadata(&destination) {
        let right_size = expected_size == 0 || found.len() == expected_size;
        // Not reported as progress: nothing is coming down, and a bar that leapt to the end and was then
        // joined by the other download would show a speed nobody's connection has.
        if found.is_file() && right_size && hash_file(&destination)? == published {
            // Whatever a later, abandoned attempt left beside it is of no more use.
            let _ = fs::remove_file(&partial);
            let _ = fs::remove_file(&record);
            return Ok((destination, Fetched::AlreadyHere));
        }
        // Not the file the release names, whatever it is, and so not to be left where it could be taken
        // for it.
        fs::remove_file(&destination).map_err(|e| local("clear away", &destination, e))?;
    }

    let split = connections > 1 && expected_size > 0;
    let mut left = left_behind(&partial, &record, expected_size);
    loop {
        let attempt = match (&left, split) {
            (Left::Ranges(ranges), true) => {
                fetch_ranges(url, &partial, &record, ranges.clone(), &mut progress)
            }
            (Left::Prefix(have), true) => fetch_ranges(
                url,
                &partial,
                &record,
                Ranges::fresh(expected_size, connections, *have),
                &mut progress,
            ),
            (Left::Ranges(ranges), false) => {
                // Carried on in one piece from the stretch at the start that is whole.
                let _ = fs::remove_file(&record);
                fetch_into(url, &partial, ranges.prefix(), expected_size, &mut progress)
                    .map_err(Split::Failed)
            }
            (Left::Prefix(have), false) => {
                fetch_into(url, &partial, *have, expected_size, &mut progress)
                    .map_err(Split::Failed)
            }
        };
        let (digest, resumed_from) = match attempt {
            Ok(done) => done,
            Err(Split::Failed(problem)) => return Err(problem),
            // The server will not hand the file over in pieces, so it is asked for whole.
            Err(Split::NotOffered) => {
                let _ = fs::remove_file(&record);
                let _ = fs::remove_file(&partial);
                fetch_into(url, &partial, 0, expected_size, &mut progress)?
            }
        };
        if digest == published {
            let _ = fs::remove_file(&record);
            fs::rename(&partial, &destination)
                .map_err(|e| local("put the download at", &destination, e))?;
            let how = match resumed_from {
                0 => Fetched::Downloaded,
                from => Fetched::Resumed { from },
            };
            return Ok((destination, how));
        }
        // Removed rather than left about: a file that failed its checksum is the one file nobody should
        // be able to pick up by accident afterwards -- this program included, next time.
        let _ = fs::remove_file(&partial);
        let _ = fs::remove_file(&record);
        if resumed_from == 0 {
            return Err(Problem::WrongChecksum {
                name: name.to_string(),
            });
        }
        left = Left::Prefix(0);
    }
}

/// How much of a download an earlier run left at [destination]'s `.partial`, in whichever shape it
/// left it: for the plan to say before anything is downloaded.
pub fn already_have(destination: &Path, expected_size: u64) -> u64 {
    let partial = partial_path(destination);
    match Ranges::read(&ranges_path(destination)) {
        Some(ranges)
            if ranges.total == expected_size
                && fs::metadata(&partial).is_ok_and(|m| m.len() == expected_size) =>
        {
            ranges.have()
        }
        _ => fs::metadata(&partial)
            .ok()
            .filter(|m| {
                m.is_file()
                    && (expected_size == 0 || m.len() <= expected_size)
                    && !ranges_path(destination).exists()
            })
            .map(|m| m.len())
            .unwrap_or(0),
    }
}

/// What an earlier run left of a download.
enum Left {
    /// The first this many bytes, in a `.partial` that holds nothing else. Nothing at all is `Prefix(0)`.
    Prefix(u64),
    /// A `.partial` of the whole size, filled in as far as its note of ranges says.
    Ranges(Ranges),
}

/// Reads what is left, and clears away whatever cannot be carried on from: a `.partial` longer than the
/// whole, a note of ranges that does not fit its file, or one with no file.
fn left_behind(partial: &Path, record: &Path, expected_size: u64) -> Left {
    let length = fs::metadata(partial)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len());
    if record.exists() {
        match (Ranges::read(record), length) {
            (Some(ranges), Some(length))
                if ranges.total == expected_size && length == expected_size =>
            {
                return Left::Ranges(ranges)
            }
            _ => {
                let _ = fs::remove_file(record);
                let _ = fs::remove_file(partial);
                return Left::Prefix(0);
            }
        }
    }
    match length {
        Some(length) if expected_size == 0 || length <= expected_size => Left::Prefix(length),
        Some(_) => {
            let _ = fs::remove_file(partial);
            Left::Prefix(0)
        }
        None => Left::Prefix(0),
    }
}

/// A download in pieces: its size, and each piece's first byte, the byte after its last, and how much of
/// it has arrived, counted from its first.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ranges {
    total: u64,
    parts: Vec<Part>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Part {
    start: u64,
    end: u64,
    done: u64,
}

impl Part {
    fn length(&self) -> u64 {
        self.end - self.start
    }
}

/// What the note of ranges starts with, so that a file that happens to have its name is not read as one.
const RANGES_HEADER: &str = "noctorium-installer ranges 1";

impl Ranges {
    /// [total] bytes split into [count] pieces, after a first one of [have] bytes that is there already.
    fn fresh(total: u64, count: u64, have: u64) -> Ranges {
        let have = have.min(total);
        let mut parts = Vec::new();
        if have > 0 {
            parts.push(Part {
                start: 0,
                end: have,
                done: have,
            });
        }
        let share = (total - have).div_ceil(count.max(1)).max(1);
        let mut start = have;
        while start < total {
            let end = (start + share).min(total);
            parts.push(Part {
                start,
                end,
                done: 0,
            });
            start = end;
        }
        Ranges { total, parts }
    }

    fn have(&self) -> u64 {
        self.parts.iter().map(|p| p.done).sum()
    }

    /// How much of the file from its first byte is whole, for carrying on in one piece.
    fn prefix(&self) -> u64 {
        let mut whole = 0;
        for part in &self.parts {
            if part.start != whole {
                break;
            }
            whole += part.done;
            if part.done < part.length() {
                break;
            }
        }
        whole
    }

    fn read(path: &Path) -> Option<Ranges> {
        let text = fs::read_to_string(path).ok()?;
        let mut lines = text.lines();
        if lines.next()? != RANGES_HEADER {
            return None;
        }
        let total = lines.next()?.strip_prefix("total ")?.trim().parse().ok()?;
        let mut parts = Vec::new();
        for line in lines.filter(|l| !l.trim().is_empty()) {
            let numbers: Vec<u64> = line
                .split_whitespace()
                .map(|n| n.parse().ok())
                .collect::<Option<_>>()?;
            let [start, end, done] = numbers[..] else {
                return None;
            };
            if start > end || done > end - start || end > total {
                return None;
            }
            parts.push(Part { start, end, done });
        }
        // The pieces have to be the whole file, in order, with nothing missing or twice over.
        let covered = parts
            .iter()
            .try_fold(0, |at, part| (part.start == at).then_some(part.end));
        (covered == Some(total)).then_some(Ranges { total, parts })
    }

    /// Written beside the note and moved over it, so that a run stopped in the middle of writing it leaves
    /// the last one whole rather than half of a new one.
    fn write(&self, path: &Path) -> std::io::Result<()> {
        let mut text = format!("{RANGES_HEADER}\ntotal {}\n", self.total);
        for part in &self.parts {
            text.push_str(&format!("{} {} {}\n", part.start, part.end, part.done));
        }
        let mut fresh = path.as_os_str().to_os_string();
        fresh.push(".new");
        fs::write(&fresh, text)?;
        fs::rename(&fresh, path)
    }
}

/// Why a download in pieces did not finish.
enum Split {
    /// The server sent the whole file, or something else, when asked for a piece of it.
    NotOffered,
    Failed(Problem),
}

/// What the threads fetching the pieces say to the one keeping count.
enum Piece {
    Arrived(usize, u64),
    Ended(Result<(), Split>),
}

/// Fetches whatever [ranges] says is still missing from [partial], each piece on a connection of its
/// own, keeping the note at [record] up to date as they go, and returns what the whole file hashes to and
/// how much of it was there before.
fn fetch_ranges(
    url: &str,
    partial: &Path,
    record: &Path,
    mut ranges: Ranges,
    progress: &mut impl FnMut(u64, u64),
) -> Result<(String, u64), Split> {
    let failed = |what: &str, e: std::io::Error| {
        Split::Failed(Problem::Local(format!(
            "Could not {what} {}: {e}",
            partial.display()
        )))
    };
    let total = ranges.total;
    let before = ranges.have();
    // The whole size from the start, so each piece can be written where it goes.
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(partial)
        .map_err(|e| failed("write to", e))?;
    file.set_len(total).map_err(|e| failed("write to", e))?;
    drop(file);
    ranges
        .write(record)
        .map_err(|e| failed("keep track of", e))?;
    progress(before, total);

    let mut first_failure: Option<Split> = None;
    std::thread::scope(|scope| {
        let (tell, heard) = std::sync::mpsc::channel::<Piece>();
        for (index, part) in ranges.parts.iter().copied().enumerate() {
            if part.done == part.length() {
                continue;
            }
            let tell = tell.clone();
            scope.spawn(move || {
                let report = tell.clone();
                let outcome = fetch_part(url, partial, part, |done| {
                    let _ = report.send(Piece::Arrived(index, done));
                });
                let _ = tell.send(Piece::Ended(outcome));
            });
        }
        drop(tell);

        let (mut reported, mut noted) = (before, before);
        for piece in heard {
            match piece {
                Piece::Arrived(index, done) => {
                    ranges.parts[index].done = done;
                    let have = ranges.have();
                    if have - reported >= 1_048_576 {
                        reported = have;
                        progress(have, total);
                    }
                    // Every few megabytes, not every one: the note is rewritten whole each time.
                    if have - noted >= 8 * 1_048_576 {
                        noted = have;
                        let _ = ranges.write(record);
                    }
                }
                Piece::Ended(Ok(())) => {}
                Piece::Ended(Err(why)) => {
                    // A server that will not split the file outranks a piece that failed: the answer to
                    // the first is to ask for it whole.
                    if first_failure.is_none() || matches!(why, Split::NotOffered) {
                        first_failure = Some(why);
                    }
                }
            }
        }
    });
    // Whatever happened, what arrived is noted, for the next attempt to carry on from.
    let _ = ranges.write(record);
    if let Some(why) = first_failure {
        return Err(why);
    }
    progress(total, total);
    let digest = hash_file(partial).map_err(Split::Failed)?;
    Ok((digest, before))
}

/// Fetches the rest of one piece into its place in [partial], saying how much of it has arrived as it
/// goes -- only once it is written, so that the note never claims what the file does not have.
fn fetch_part(
    url: &str,
    partial: &Path,
    part: Part,
    mut arrived: impl FnMut(u64),
) -> Result<(), Split> {
    let failed = |what: &str, e: std::io::Error| {
        Split::Failed(Problem::Local(format!(
            "Could not {what} {}: {e}",
            partial.display()
        )))
    };
    let mut file = OpenOptions::new()
        .write(true)
        .open(partial)
        .map_err(|e| failed("write to", e))?;
    let mut done = part.done;
    let mut buffer = vec![0u8; 128 * 1024];
    for attempt in 1..=ATTEMPTS {
        let from = part.start + done;
        let response = match request(url)
            .set("Range", &format!("bytes={from}-{}", part.end - 1))
            .call()
        {
            Ok(response) => response,
            Err(ureq::Error::Status(416, _)) => return Err(Split::NotOffered),
            Err(other) => return Err(Split::Failed(problem(other))),
        };
        let starts = response
            .header("Content-Range")
            .and_then(|range| range.trim().strip_prefix("bytes "))
            .and_then(|range| range.split('-').next())
            .and_then(|first| first.trim().parse::<u64>().ok());
        if response.status() != 206 || starts != Some(from) {
            return Err(Split::NotOffered);
        }
        file.seek(SeekFrom::Start(from))
            .map_err(|e| failed("write to", e))?;
        let mut out = BufWriter::with_capacity(1 << 20, &mut file);
        // Never more than the piece, whatever the server sends: the next piece's bytes are not this one's
        // to write.
        let mut reader = response.into_reader().take(part.length() - done);
        let before = done;
        let mut written_since = 0u64;
        let stopped = loop {
            match reader.read(&mut buffer) {
                Ok(0) => break None,
                Ok(read) => {
                    out.write_all(&buffer[..read])
                        .map_err(|e| failed("write to", e))?;
                    done += read as u64;
                    written_since += read as u64;
                    if written_since >= 1_048_576 {
                        out.flush().map_err(|e| failed("write to", e))?;
                        written_since = 0;
                        arrived(done);
                    }
                }
                Err(e) => break Some(e),
            }
        };
        out.flush().map_err(|e| failed("write to", e))?;
        drop(out);
        arrived(done);
        let stopped = match stopped {
            None if done == part.length() => return Ok(()),
            None => "the connection closed before the piece was whole".to_string(),
            Some(e) => e.to_string(),
        };
        if attempt == ATTEMPTS || done == before {
            return Err(Split::Failed(Problem::Network(format!(
                "the download stopped early: {stopped}"
            ))));
        }
    }
    unreachable!("the last attempt returns")
}

/// Fetches [url] onto the end of [partial], which already holds its first [have] bytes, and returns what
/// the whole file hashes to, and how much of it was there before -- nothing, when the server sent the
/// whole of it again.
fn fetch_into(
    url: &str,
    partial: &Path,
    mut have: u64,
    expected_size: u64,
    progress: &mut impl FnMut(u64, u64),
) -> Result<(String, u64), Problem> {
    let local = |what: &str, e: std::io::Error| {
        Problem::Local(format!("Could not {what} {}: {e}", partial.display()))
    };
    // What is there already is hashed first, so the hash at the end is of the whole file and not only
    // of what came down this time.
    let mut hasher = Sha256::new();
    if have > 0 {
        let mut file = File::open(partial).map_err(|e| local("read", e))?;
        have = feed(&mut hasher, &mut file).map_err(|e| local("read", e))?;
    }
    // Not opened to append: on Windows a file opened that way cannot be cut short again, which a server
    // that answers a Range with the whole file needs it to be.
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(partial)
        .map_err(|e| local("write to", e))?;
    file.set_len(have).map_err(|e| local("write to", e))?;
    file.seek(SeekFrom::Start(have))
        .map_err(|e| local("write to", e))?;
    // A megabyte at a time to the disk, rather than every few kilobytes the connection hands over.
    let mut out = BufWriter::with_capacity(1 << 20, file);
    let mut buffer = vec![0u8; 128 * 1024];
    let mut total = expected_size;
    let mut last_report = 0u64;
    let mut carried_on_from = None;

    for attempt in 1..=ATTEMPTS {
        let (mut reader, length) = match ask_from(url, have)? {
            Answer::Rest(response) => {
                carried_on_from.get_or_insert(have);
                let length = content_length(&response).map(|rest| have + rest);
                (response.into_reader(), length)
            }
            Answer::Whole(response) => {
                // Asked for the rest and sent all of it, or asked for all of it: either way from the
                // start, and nothing written so far counts.
                if have > 0 {
                    out.flush().map_err(|e| local("write to", e))?;
                    let file = out.get_mut();
                    file.set_len(0).map_err(|e| local("write to", e))?;
                    file.seek(SeekFrom::Start(0))
                        .map_err(|e| local("write to", e))?;
                    hasher = Sha256::new();
                    have = 0;
                    last_report = 0;
                }
                carried_on_from = Some(0);
                let length = content_length(&response);
                (response.into_reader(), length)
            }
        };
        total = length.unwrap_or(total);
        progress(have, total);
        let before = have;

        let stopped = loop {
            match reader.read(&mut buffer) {
                Ok(0) => break None,
                Ok(read) => {
                    hasher.update(&buffer[..read]);
                    out.write_all(&buffer[..read])
                        .map_err(|e| local("write to", e))?;
                    have += read as u64;
                    // Once a megabyte rather than once a buffer: whoever is watching does not need telling
                    // two thousand times a second, and a progress line that redraws that often is its own
                    // slowdown.
                    if have.saturating_sub(last_report) >= 1_048_576 {
                        last_report = have;
                        progress(have, total);
                    }
                }
                Err(e) => break Some(e),
            }
        };
        // Written out whether it finished or not: what arrived is kept for the next attempt, or the next
        // run, to carry on from.
        out.flush().map_err(|e| local("write to", e))?;
        match stopped {
            None => {
                progress(have, total);
                return Ok((hex(&hasher.finalize()), carried_on_from.unwrap_or(0)));
            }
            // Tried again only when this attempt got somewhere: a connection that breaks at once will
            // break again, and saying so now is better than saying so three times slower.
            Some(_) if attempt < ATTEMPTS && have > before => continue,
            Some(e) => {
                return Err(Problem::Network(format!("the download stopped early: {e}")));
            }
        }
    }
    unreachable!("the last attempt returns")
}

/// What a request for a file from a given byte on was answered with.
enum Answer {
    /// The rest of it, from exactly where it was asked for.
    Rest(ureq::Response),
    /// The whole of it.
    Whole(ureq::Response),
}

fn ask_from(url: &str, from: u64) -> Result<Answer, Problem> {
    if from == 0 {
        return send(url).map(Answer::Whole);
    }
    match request(url).set("Range", &format!("bytes={from}-")).call() {
        Ok(response) if response.status() == 206 => {
            // Content-Range: bytes 1000-1999/2000. Only a range that starts where this one was asked to is
            // the rest of this file; anything else is not to be stitched onto what is here.
            let starts = response
                .header("Content-Range")
                .and_then(|range| range.trim().strip_prefix("bytes "))
                .and_then(|range| range.split('-').next())
                .and_then(|first| first.trim().parse::<u64>().ok());
            if starts == Some(from) {
                Ok(Answer::Rest(response))
            } else {
                send(url).map(Answer::Whole)
            }
        }
        Ok(response) => Ok(Answer::Whole(response)),
        // What is here is not the start of a file this long -- the file was replaced, say, by one
        // shorter -- so it is started again.
        Err(ureq::Error::Status(416, _)) => send(url).map(Answer::Whole),
        Err(other) => Err(problem(other)),
    }
}

fn content_length(response: &ureq::Response) -> Option<u64> {
    response
        .header("Content-Length")
        .and_then(|value| value.trim().parse::<u64>().ok())
}

/// Reads [reader] to the end through [hasher], and says how many bytes that was.
fn feed(hasher: &mut Sha256, reader: &mut impl Read) -> std::io::Result<u64> {
    let mut buffer = vec![0u8; 128 * 1024];
    let mut count = 0u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(count);
        }
        hasher.update(&buffer[..read]);
        count += read as u64;
    }
}

/// What the file at [path] hashes to, written as sha256sum writes it.
pub fn hash_file(path: &Path) -> Result<String, Problem> {
    let failed =
        |e: std::io::Error| Problem::Local(format!("Could not read {}: {e}", path.display()));
    let mut file = File::open(path).map_err(failed)?;
    let mut hasher = Sha256::new();
    feed(&mut hasher, &mut file).map_err(failed)?;
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::archive::tests::scratch;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

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

    /// How the stand-in for GitHub's CDN behaves.
    #[derive(Clone, Copy, PartialEq)]
    pub(crate) enum Serves {
        /// As the CDN does: the whole file, or the range asked for.
        Ranges,
        /// The whole file, whatever was asked, as a server that knows nothing of ranges would.
        AlwaysWhole,
        /// Ranges, but the first answer is cut off after this many bytes, as a dropped connection is.
        CutOnceAfter(usize),
        /// Ranges, and every answer is cut off after this many bytes.
        AlwaysCutAfter(usize),
        /// Ranges, a little at a time, as a slow connection does, so two downloads overlap.
        Slowly,
    }

    /// Every request's range: its first byte, and its last when it said.
    type Asked = Mutex<Vec<(Option<u64>, Option<u64>)>>;

    /// A web server on this machine serving [body], each connection on a thread of its own as a CDN
    /// would, which writes down the Range of every request it was sent: where it started, or `None` for
    /// a request for the whole file, and where it ended, when it said.
    pub(crate) struct Server {
        pub(crate) url: String,
        asked: Arc<Asked>,
    }

    impl Server {
        pub(crate) fn start(body: Vec<u8>, serves: Serves) -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/file.bin", listener.local_addr().unwrap());
            let asked = Arc::new(Mutex::new(Vec::new()));
            let log = asked.clone();
            let body = Arc::new(body);
            std::thread::spawn(move || {
                for (index, stream) in listener.incoming().enumerate() {
                    let Ok(stream) = stream else { return };
                    let (log, body) = (log.clone(), body.clone());
                    std::thread::spawn(move || answer(stream, index, &body, serves, &log));
                }
            });
            Server { url, asked }
        }

        /// Where each request started, in the order they came.
        pub(crate) fn asked(&self) -> Vec<Option<u64>> {
            self.asked
                .lock()
                .unwrap()
                .iter()
                .map(|(from, _)| *from)
                .collect()
        }

        /// Each request's range, first byte and last, in order of the first.
        pub(crate) fn ranges(&self) -> Vec<(Option<u64>, Option<u64>)> {
            let mut ranges = self.asked.lock().unwrap().clone();
            ranges.sort();
            ranges
        }
    }

    fn answer(
        mut stream: std::net::TcpStream,
        index: usize,
        body: &[u8],
        serves: Serves,
        log: &Asked,
    ) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let (mut from, mut to) = (None, None);
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
            if let Some(range) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                let (first, last) = range.trim().split_once('-').unwrap_or((range.trim(), ""));
                from = first.parse::<u64>().ok();
                to = last.parse::<u64>().ok();
            }
        }
        log.lock().unwrap().push((from, to));
        let from = from.filter(|_| serves != Serves::AlwaysWhole);
        let start = from.unwrap_or(0) as usize;
        let end = to
            .filter(|_| from.is_some())
            .map(|to| (to as usize + 1).min(body.len()))
            .unwrap_or(body.len());
        let head = match from {
            Some(_) if start >= body.len() => {
                let _ = stream.write_all(
                    b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                return;
            }
            Some(_) => format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{}\r\nConnection: close\r\n\r\n",
                end - start,
                end - 1,
                body.len()
            ),
            None => format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ),
        };
        let mut rest = &body[start..end];
        match serves {
            Serves::CutOnceAfter(cut) if index == 0 => rest = &rest[..cut.min(rest.len())],
            Serves::AlwaysCutAfter(cut) => rest = &rest[..cut.min(rest.len())],
            _ => {}
        }
        let _ = stream.write_all(head.as_bytes());
        if serves == Serves::Slowly {
            for chunk in rest.chunks(32 * 1024) {
                if stream.write_all(chunk).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        } else {
            let _ = stream.write_all(rest);
        }
    }

    /// Three megabytes and a bit that are not the same all the way through, so a byte out of place shows.
    fn body() -> Vec<u8> {
        (0..3_200_000u32).map(|i| (i * 7 + i / 251) as u8).collect()
    }

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    #[test]
    fn a_download_is_written_under_another_name_until_it_is_whole_and_checked() {
        let here = scratch("fetch-fresh");
        let body = body();
        let server = Server::start(body.clone(), Serves::Ranges);
        let destination = here.join("file.bin");
        let mut seen_partial = false;
        let (path, how) = download(
            &server.url,
            &here,
            "file.bin",
            body.len() as u64,
            &sha(&body),
            |done, _| {
                if done > 0 && done < body.len() as u64 {
                    assert!(
                        !destination.exists(),
                        "a half file must never have the whole one's name"
                    );
                    seen_partial |= partial_path(&destination).is_file();
                }
            },
        )
        .expect("downloads");
        assert_eq!(how, Fetched::Downloaded);
        assert_eq!(path, destination);
        assert_eq!(fs::read(&path).unwrap(), body);
        assert!(seen_partial, "the bytes went to the .partial first");
        assert!(!partial_path(&destination).exists());
        assert_eq!(server.asked(), vec![None]);
    }

    #[test]
    fn a_file_already_here_with_the_published_checksum_is_not_downloaded_again() {
        let here = scratch("fetch-reuse");
        let body = body();
        fs::write(here.join("file.bin"), &body).unwrap();
        let server = Server::start(body.clone(), Serves::Ranges);
        let (path, how) = download(
            &server.url,
            &here,
            "file.bin",
            body.len() as u64,
            &sha(&body),
            |_, _| {},
        )
        .expect("reuses");
        assert_eq!(how, Fetched::AlreadyHere);
        assert_eq!(fs::read(path).unwrap(), body);
        assert!(server.asked().is_empty(), "nothing was asked of the server");
    }

    #[test]
    fn a_file_already_here_that_is_not_the_published_one_is_replaced() {
        let here = scratch("fetch-stale");
        let body = body();
        let mut stale = body.clone();
        stale[1000] ^= 0xFF;
        fs::write(here.join("file.bin"), &stale).unwrap();
        let server = Server::start(body.clone(), Serves::Ranges);
        let (path, how) = download(
            &server.url,
            &here,
            "file.bin",
            body.len() as u64,
            &sha(&body),
            |_, _| {},
        )
        .expect("downloads again");
        assert_eq!(how, Fetched::Downloaded);
        assert_eq!(fs::read(path).unwrap(), body);
    }

    #[test]
    fn a_partial_download_is_resumed_with_a_range_request() {
        let here = scratch("fetch-resume");
        let body = body();
        let destination = here.join("file.bin");
        fs::write(partial_path(&destination), &body[..1_234_567]).unwrap();
        let server = Server::start(body.clone(), Serves::Ranges);
        let mut first_report = None;
        let (path, how) = download(
            &server.url,
            &here,
            "file.bin",
            body.len() as u64,
            &sha(&body),
            |done, total| {
                first_report.get_or_insert((done, total));
            },
        )
        .expect("resumes");
        assert_eq!(how, Fetched::Resumed { from: 1_234_567 });
        assert_eq!(server.asked(), vec![Some(1_234_567)]);
        assert_eq!(
            first_report,
            Some((1_234_567, body.len() as u64)),
            "progress starts from what was there"
        );
        assert_eq!(
            fs::read(path).unwrap(),
            body,
            "and the whole file is checked"
        );
        assert!(!partial_path(&destination).exists());
    }

    #[test]
    fn a_server_that_ignores_the_range_is_taken_at_its_word() {
        let here = scratch("fetch-whole");
        let body = body();
        let destination = here.join("file.bin");
        fs::write(partial_path(&destination), &body[..500_000]).unwrap();
        let server = Server::start(body.clone(), Serves::AlwaysWhole);
        let (path, how) = download(
            &server.url,
            &here,
            "file.bin",
            body.len() as u64,
            &sha(&body),
            |_, _| {},
        )
        .expect("downloads whole");
        assert_eq!(how, Fetched::Downloaded, "the file was started again");
        assert_eq!(
            fs::read(path).unwrap(),
            body,
            "not appended to what was there"
        );
    }

    /// A .partial from something else -- a file replaced under the same name -- resumes into a file that
    /// fails its checksum, and is then fetched once more from nothing.
    #[test]
    fn a_resumed_download_that_fails_its_checksum_is_fetched_again_from_the_start() {
        let here = scratch("fetch-wrong-start");
        let body = body();
        let destination = here.join("file.bin");
        fs::write(partial_path(&destination), vec![0xAAu8; 700_000]).unwrap();
        let server = Server::start(body.clone(), Serves::Ranges);
        let (path, how) = download(
            &server.url,
            &here,
            "file.bin",
            body.len() as u64,
            &sha(&body),
            |_, _| {},
        )
        .expect("downloads again");
        assert_eq!(how, Fetched::Downloaded);
        assert_eq!(server.asked(), vec![Some(700_000), None]);
        assert_eq!(fs::read(path).unwrap(), body);
    }

    #[test]
    fn a_connection_that_drops_is_picked_up_where_it_stopped() {
        let here = scratch("fetch-dropped");
        let body = body();
        let server = Server::start(body.clone(), Serves::CutOnceAfter(1_000_000));
        let (path, how) = download(
            &server.url,
            &here,
            "file.bin",
            body.len() as u64,
            &sha(&body),
            |_, _| {},
        )
        .expect("carries on");
        assert_eq!(how, Fetched::Downloaded, "nothing was here before this run");
        assert_eq!(server.asked(), vec![None, Some(1_000_000)]);
        assert_eq!(fs::read(path).unwrap(), body);
    }

    #[test]
    fn a_download_that_keeps_failing_leaves_what_it_got_for_next_time() {
        let here = scratch("fetch-flaky");
        let body = body();
        let server = Server::start(body.clone(), Serves::AlwaysCutAfter(400_000));
        let outcome = download(
            &server.url,
            &here,
            "file.bin",
            body.len() as u64,
            &sha(&body),
            |_, _| {},
        );
        assert!(matches!(outcome, Err(Problem::Network(_))), "{outcome:?}");
        let destination = here.join("file.bin");
        assert!(!destination.exists());
        assert_eq!(
            fs::read(partial_path(&destination)).unwrap(),
            body[..400_000 * ATTEMPTS],
            "every attempt carried on from the last"
        );
        assert_eq!(server.asked(), vec![None, Some(400_000), Some(800_000)],);
    }

    #[test]
    fn a_download_that_is_not_the_published_file_is_removed() {
        let here = scratch("fetch-mismatch");
        let body = body();
        let server = Server::start(body.clone(), Serves::Ranges);
        let outcome = download(
            &server.url,
            &here,
            "file.bin",
            body.len() as u64,
            &sha(b"something else"),
            |_, _| {},
        );
        assert!(
            matches!(&outcome, Err(Problem::WrongChecksum { name }) if name == "file.bin"),
            "{outcome:?}"
        );
        assert!(!here.join("file.bin").exists());
        assert!(!partial_path(&here.join("file.bin")).exists());
    }

    #[test]
    fn a_partial_longer_than_the_file_is_not_resumed_from() {
        let here = scratch("fetch-too-long");
        let body = body();
        let destination = here.join("file.bin");
        fs::write(partial_path(&destination), vec![1u8; body.len() + 10]).unwrap();
        let server = Server::start(body.clone(), Serves::Ranges);
        let (_, how) = download(
            &server.url,
            &here,
            "file.bin",
            body.len() as u64,
            &sha(&body),
            |_, _| {},
        )
        .expect("downloads");
        assert_eq!(how, Fetched::Downloaded);
        assert_eq!(server.asked(), vec![None]);
    }

    // ------------------------------------------------------------ in pieces

    fn in_pieces(
        here: &Path,
        url: &str,
        body: &[u8],
        connections: u64,
    ) -> Result<(PathBuf, Fetched), Problem> {
        download_with(
            url,
            here,
            "file.bin",
            body.len() as u64,
            &sha(body),
            connections,
            |_, _| {},
        )
    }

    #[test]
    fn a_big_file_comes_down_in_pieces_at_once_and_is_checked_whole() {
        let here = scratch("pieces");
        let body = body();
        let server = Server::start(body.clone(), Serves::Slowly);
        let mut reports = Vec::new();
        let (path, how) = download_with(
            &server.url,
            &here,
            "file.bin",
            body.len() as u64,
            &sha(&body),
            4,
            |done, total| reports.push((done, total)),
        )
        .expect("downloads");
        assert_eq!(how, Fetched::Downloaded);
        assert_eq!(fs::read(&path).unwrap(), body);
        assert_eq!(
            server.ranges(),
            vec![
                (Some(0), Some(799_999)),
                (Some(800_000), Some(1_599_999)),
                (Some(1_600_000), Some(2_399_999)),
                (Some(2_400_000), Some(3_199_999)),
            ]
        );
        assert!(!partial_path(&path).exists() && !ranges_path(&path).exists());
        assert_eq!(reports.first(), Some(&(0, body.len() as u64)));
        assert_eq!(
            reports.last(),
            Some(&(body.len() as u64, body.len() as u64))
        );
        assert!(
            reports.windows(2).all(|w| w[0].0 <= w[1].0),
            "progress only goes forward: {reports:?}"
        );
    }

    /// Two pieces whole, one half there, one not started: only what is missing is asked for.
    #[test]
    fn a_download_in_pieces_is_carried_on_from_its_note() {
        let here = scratch("pieces-resume");
        let body = body();
        let destination = here.join("file.bin");
        let mut partial = body.clone();
        // What has not arrived is not there: nothing in the file says otherwise but the note.
        partial[1_200_000..1_600_000].fill(0);
        partial[2_400_000..].fill(0);
        fs::write(partial_path(&destination), &partial).unwrap();
        let part = |start, end, done| Part { start, end, done };
        let note = Ranges {
            total: body.len() as u64,
            parts: vec![
                part(0, 800_000, 800_000),
                part(800_000, 1_600_000, 400_000),
                part(1_600_000, 2_400_000, 800_000),
                part(2_400_000, 3_200_000, 0),
            ],
        };
        note.write(&ranges_path(&destination)).unwrap();
        assert_eq!(already_have(&destination, body.len() as u64), 2_000_000);

        let server = Server::start(body.clone(), Serves::Ranges);
        let (path, how) = in_pieces(&here, &server.url, &body, 4).expect("carries on");
        assert_eq!(how, Fetched::Resumed { from: 2_000_000 });
        assert_eq!(
            server.ranges(),
            vec![
                (Some(1_200_000), Some(1_599_999)),
                (Some(2_400_000), Some(3_199_999)),
            ]
        );
        assert_eq!(fs::read(path).unwrap(), body);
        assert!(!ranges_path(&destination).exists());
    }

    /// A .partial from a download in one piece is the first piece of one in several.
    #[test]
    fn a_partial_in_one_piece_becomes_the_first_of_several() {
        let here = scratch("pieces-from-one");
        let body = body();
        let destination = here.join("file.bin");
        fs::write(partial_path(&destination), &body[..1_000_000]).unwrap();
        let server = Server::start(body.clone(), Serves::Ranges);
        let (path, how) = in_pieces(&here, &server.url, &body, 2).expect("carries on");
        assert_eq!(how, Fetched::Resumed { from: 1_000_000 });
        assert_eq!(
            server.ranges(),
            vec![
                (Some(1_000_000), Some(2_099_999)),
                (Some(2_100_000), Some(3_199_999)),
            ]
        );
        assert_eq!(fs::read(path).unwrap(), body);
    }

    #[test]
    fn a_server_that_will_not_split_the_file_is_asked_for_it_whole() {
        let here = scratch("pieces-refused");
        let body = body();
        let server = Server::start(body.clone(), Serves::AlwaysWhole);
        let (path, how) = in_pieces(&here, &server.url, &body, 4).expect("downloads whole");
        assert_eq!(how, Fetched::Downloaded);
        assert_eq!(fs::read(&path).unwrap(), body);
        assert!(
            server.asked().contains(&None),
            "asked for it whole in the end: {:?}",
            server.asked()
        );
        assert!(!ranges_path(&path).exists());
    }

    #[test]
    fn a_piece_whose_connection_drops_is_picked_up_where_it_stopped() {
        let here = scratch("pieces-dropped");
        let body = body();
        let server = Server::start(body.clone(), Serves::CutOnceAfter(300_000));
        let (path, _) = in_pieces(&here, &server.url, &body, 4).expect("carries on");
        assert_eq!(fs::read(path).unwrap(), body);
        assert_eq!(server.asked().len(), 5, "four pieces and one of them again");
    }

    #[test]
    fn pieces_that_keep_failing_are_noted_for_next_time() {
        let here = scratch("pieces-flaky");
        let body = body();
        let server = Server::start(body.clone(), Serves::AlwaysCutAfter(100_000));
        let outcome = in_pieces(&here, &server.url, &body, 4);
        assert!(matches!(outcome, Err(Problem::Network(_))), "{outcome:?}");
        let destination = here.join("file.bin");
        assert!(!destination.exists());
        // Three attempts at each of four pieces, a hundred thousand bytes each time.
        assert_eq!(already_have(&destination, body.len() as u64), 1_200_000);

        // And the next run, with a better connection, needs only the rest.
        let better = Server::start(body.clone(), Serves::Ranges);
        let (path, how) = in_pieces(&here, &better.url, &body, 4).expect("finishes");
        assert_eq!(how, Fetched::Resumed { from: 1_200_000 });
        assert_eq!(fs::read(path).unwrap(), body);
    }

    #[test]
    fn a_note_that_does_not_fit_its_file_is_not_trusted() {
        let here = scratch("pieces-bad-note");
        let body = body();
        let destination = here.join("file.bin");
        // A note for a file of another size, and then one that leaves a stretch out.
        fs::write(partial_path(&destination), vec![0u8; body.len()]).unwrap();
        Ranges::fresh(body.len() as u64 + 1, 4, 0)
            .write(&ranges_path(&destination))
            .unwrap();
        assert_eq!(already_have(&destination, body.len() as u64), 0);
        fs::write(
            ranges_path(&destination),
            format!("{RANGES_HEADER}\ntotal 3200000\n0 100 100\n200 3200000 0\n"),
        )
        .unwrap();
        assert!(Ranges::read(&ranges_path(&destination)).is_none());

        let server = Server::start(body.clone(), Serves::Ranges);
        let (path, how) = in_pieces(&here, &server.url, &body, 4).expect("starts again");
        assert_eq!(how, Fetched::Downloaded);
        assert_eq!(fs::read(path).unwrap(), body);
    }

    #[test]
    fn a_file_is_split_into_even_pieces_after_what_is_there() {
        let fresh = Ranges::fresh(10, 4, 0);
        let spans: Vec<(u64, u64)> = fresh.parts.iter().map(|p| (p.start, p.end)).collect();
        assert_eq!(spans, vec![(0, 3), (3, 6), (6, 9), (9, 10)]);
        let after = Ranges::fresh(10, 2, 4);
        let part = |start, end, done| Part { start, end, done };
        assert_eq!(
            after.parts,
            vec![part(0, 4, 4), part(4, 7, 0), part(7, 10, 0)]
        );
        assert_eq!(after.have(), 4);
        assert_eq!(after.prefix(), 4);
        assert_eq!(
            Ranges::fresh(10, 4, 10).parts.len(),
            1,
            "nothing left to fetch"
        );
    }
}
