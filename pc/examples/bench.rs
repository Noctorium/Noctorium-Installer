//! How long the work that is not waiting on somebody's network takes, measured with the release profile.
//!
//! Two things the installer does with every byte it fetches: hash it, and -- for the Noctorium CLI --
//! inflate it and write it out again. Both are compiled into a program built for size, and this is how to
//! find out what that costs, before and after a change to `[profile.release]`. The third, a download over
//! one connection and over several, is the question of whether splitting a big file into ranges is worth
//! the complication; it is asked of the real release, so the answer is this machine's network and not a
//! guess.
//!
//! The fourth is the installer's own downloading, of both products from a real release, the old way --
//! one after the other, over a connection each -- or any other: at once, and over as many connections
//! each as asked. It goes through the library's download, checksums and all, into a folder that is
//! emptied before every round so that nothing is found from the last.
//!
//! ```text
//! cargo run --release --example bench -- hash FILE [ROUNDS]
//! cargo run --release --example bench -- unpack ARCHIVE [ROUNDS]
//! cargo run --release --example bench -- download URL CONNECTIONS [ROUNDS]
//! cargo run --release --example bench -- both VERSION FOLDER CONNECTIONS one-by-one|at-once [ROUNDS]
//! ```
//!
//! Nothing it writes outlives it but the file it was asked to hash, which is the caller's, and what it
//! downloaded into the folder it was given. The unpacking goes into a folder of its own under the
//! temporary folder, and is cleared away afterwards.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let rounds =
        |at: usize| -> usize { arguments.get(at).and_then(|r| r.parse().ok()).unwrap_or(3) };
    match arguments.first().map(String::as_str) {
        Some("hash") => {
            let file = PathBuf::from(arguments.get(1).expect("hash FILE [ROUNDS]"));
            let size = std::fs::metadata(&file).expect("no such file").len();
            // Read once first, so every round reads from the cache and what is timed is the hashing.
            let _ = hash(&file);
            report("hash", size, (0..rounds(2)).map(|_| hash(&file).1).collect());
        }
        Some("unpack") => {
            let archive = PathBuf::from(arguments.get(1).expect("unpack ARCHIVE [ROUNDS]"));
            let size = std::fs::metadata(&archive).expect("no such archive").len();
            let base = std::env::temp_dir().join(format!("noctorium-bench-{}", std::process::id()));
            let times = (0..rounds(2))
                .map(|round| {
                    let into = base.join(format!("round-{round}")).join("Noctorium CLI");
                    let started = Instant::now();
                    noctorium_installer::archive::install_folder(&archive, &into, "the Noctorium CLI")
                        .unwrap_or_else(|problem| panic!("{problem}"));
                    started.elapsed()
                })
                .collect();
            let _ = std::fs::remove_dir_all(&base);
            report("unpack", size, times);
        }
        Some("download") => {
            let url = arguments.get(1).expect("download URL CONNECTIONS [ROUNDS]");
            let connections: u64 = arguments
                .get(2)
                .and_then(|c| c.parse().ok())
                .expect("download URL CONNECTIONS [ROUNDS]");
            let agent = ureq::AgentBuilder::new().build();
            let mut size = 0;
            let times = (0..rounds(3))
                .map(|_| {
                    let (bytes, took) = download(&agent, url, connections);
                    size = bytes;
                    took
                })
                .collect();
            report(&format!("download x{connections}"), size, times);
        }
        Some("both") => {
            let usage = "both VERSION FOLDER CONNECTIONS one-by-one|at-once [ROUNDS]";
            let version = arguments.get(1).expect(usage);
            let folder = PathBuf::from(arguments.get(2).expect(usage));
            let connections: u64 = arguments.get(3).and_then(|c| c.parse().ok()).expect(usage);
            let at_once = match arguments.get(4).map(String::as_str) {
                Some("at-once") => true,
                Some("one-by-one") => false,
                _ => panic!("{usage}"),
            };
            both(version, &folder, connections, at_once, rounds(5));
        }
        _ => eprintln!(
            "bench hash FILE [ROUNDS] | bench unpack ARCHIVE [ROUNDS] | bench download URL CONNECTIONS [ROUNDS] | bench both VERSION FOLDER CONNECTIONS one-by-one|at-once [ROUNDS]"
        ),
    }
}

/// Both products of release [version], for this machine, downloaded and checked [rounds] times.
fn both(version: &str, folder: &Path, connections: u64, at_once: bool, rounds: usize) {
    use noctorium_installer::flow::{self, Options, Products};
    let found = flow::look(Some(version)).unwrap_or_else(|problem| panic!("{problem}"));
    let plan = flow::plan(
        &found,
        Options {
            products: Products::BOTH,
            ..Options::default()
        },
    )
    .unwrap_or_else(|problem| panic!("{problem}"));
    let bytes: u64 = plan.items.iter().map(|i| i.asset.size).sum();
    let fetch = |item: &flow::Item| {
        let started = Instant::now();
        noctorium_installer::fetch::download_with(
            &item.asset.url,
            folder,
            &item.asset.name,
            item.asset.size,
            &item.published,
            connections,
            |_, _| {},
        )
        .unwrap_or_else(|problem| panic!("{problem}"));
        (item.asset.name.clone(), started.elapsed())
    };
    let mut times = Vec::new();
    for _ in 0..rounds {
        let _ = std::fs::remove_dir_all(folder);
        std::fs::create_dir_all(folder).expect("could not make the folder");
        let started = Instant::now();
        let each: Vec<(String, Duration)> = if at_once {
            std::thread::scope(|scope| {
                let running: Vec<_> = plan
                    .items
                    .iter()
                    .map(|item| scope.spawn(move || fetch(item)))
                    .collect();
                running.into_iter().map(|r| r.join().unwrap()).collect()
            })
        } else {
            plan.items.iter().map(fetch).collect()
        };
        let took = started.elapsed();
        let each: Vec<String> = each
            .iter()
            .map(|(name, t)| format!("{name} {:.1}s", t.as_secs_f64()))
            .collect();
        println!("  round: {:.1}s  ({})", took.as_secs_f64(), each.join(", "));
        times.push(took);
    }
    let how = if at_once { "at once" } else { "one by one" };
    report(&format!("both, {how}, x{connections} each"), bytes, times);
}

/// The same loop the installer hashes with: a buffer at a time, through SHA-256.
fn hash(file: &Path) -> (String, Duration) {
    let started = Instant::now();
    let mut reader = std::fs::File::open(file).expect("could not open the file");
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 128 * 1024];
    loop {
        let read = reader.read(&mut buffer).expect("could not read the file");
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    (digest, started.elapsed())
}

/// Fetches [url] over [connections] ranges at once into memory that is thrown away, and says how many
/// bytes that was and how long it took. Into nothing rather than onto the disk, so what is timed is the
/// network and the TLS, which is the only part that splitting into ranges could change.
fn download(agent: &ureq::Agent, url: &str, connections: u64) -> (u64, Duration) {
    let started = Instant::now();
    // The redirect to the CDN is followed once, by a request for the first byte, so every range goes
    // straight to the same signed URL rather than each being redirected on its own.
    let probe = agent
        .get(url)
        .set("Range", "bytes=0-0")
        .call()
        .expect("the probe failed");
    let total: u64 = probe
        .header("Content-Range")
        .and_then(|range| range.rsplit('/').next())
        .and_then(|total| total.parse().ok())
        .expect("the server did not say how big the file is");
    let target = probe.get_url().to_string();
    drop(probe);

    let share = total.div_ceil(connections);
    let fetched: u64 = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..connections)
            .map(|index| {
                let target = target.clone();
                scope.spawn(move || {
                    let first = index * share;
                    let last = ((index + 1) * share).min(total) - 1;
                    let response = agent
                        .get(&target)
                        .set("Range", &format!("bytes={first}-{last}"))
                        .call()
                        .expect("a range failed");
                    let mut reader = response.into_reader();
                    let mut buffer = vec![0u8; 128 * 1024];
                    let mut got = 0u64;
                    loop {
                        let read = reader.read(&mut buffer).expect("a range stopped early");
                        if read == 0 {
                            break;
                        }
                        got += read as u64;
                    }
                    got
                })
            })
            .collect();
        workers.into_iter().map(|w| w.join().unwrap()).sum()
    });
    assert_eq!(fetched, total, "the ranges did not add up to the file");
    (total, started.elapsed())
}

fn report(what: &str, bytes: u64, mut times: Vec<Duration>) {
    times.sort();
    let megabytes = bytes as f64 / 1_048_576.0;
    let each: Vec<String> = times
        .iter()
        .map(|t| format!("{:.3}s", t.as_secs_f64()))
        .collect();
    let best = times[0].as_secs_f64();
    let median = times[times.len() / 2].as_secs_f64();
    println!(
        "{what}: {megabytes:.1} MB  best {best:.3}s ({:.0} MB/s)  median {median:.3}s ({:.0} MB/s)  all [{}]",
        megabytes / best,
        megabytes / median,
        each.join(", ")
    );
}
