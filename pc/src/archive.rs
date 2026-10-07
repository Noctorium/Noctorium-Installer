//! Unpacking the Noctorium CLI, or Noctorium Stats, into a folder of its own, over an older copy if there
//! is one.
//!
//! The terminal player ships as an archive of a whole program folder -- a launcher, its Java runtime and
//! its jars -- rather than as a package, so installing it is unpacking it somewhere the user owns. That is
//! simple until there is already a copy there: unpacking on top of it would leave behind every file the
//! old version had and the new one does not, and an old jar left on the class path is the kind of fault
//! that takes a day to find. So the new copy is unpacked beside the old one, and swapped in whole.
//! Noctorium Stats is one program and a few files beside it, but it is installed the same way, for the
//! same reason and with the same code.

use crate::github::Problem;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

/// The shapes of archive this understands, read from the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    TarGz,
    Zip,
}

fn kind_of(archive: &Path) -> Option<Kind> {
    let name = archive.file_name()?.to_string_lossy().to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(Kind::TarGz)
    } else if name.ends_with(".zip") {
        Some(Kind::Zip)
    } else {
        None
    }
}

/// Unpacks [archive] so that its contents end up as [destination], replacing whatever was there.
///
/// An archive with a single folder at the top -- `noctorium-cli/`, which is how it is published -- has
/// that folder become [destination]; one with several things at the top has them all put inside it.
/// Either way nothing of an earlier copy survives, and if anything goes wrong before the swap the earlier
/// copy is exactly as it was. [product] is what is being installed, as somebody would name it, for the
/// one failure that is usually theirs to put right: the old copy still running.
pub fn install_folder(archive: &Path, destination: &Path, product: &str) -> Result<(), Problem> {
    let kind = kind_of(archive).ok_or_else(|| {
        Problem::Local(format!(
            "{} is neither a .zip nor a .tar.gz, so it cannot be unpacked.",
            archive.display()
        ))
    })?;
    let parent = destination.parent().ok_or_else(|| {
        Problem::Local(format!(
            "{} has no folder above it to unpack into.",
            destination.display()
        ))
    })?;
    let name = destination
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "noctorium-cli".to_string());
    fs::create_dir_all(parent).map_err(|e| couldnt("make", parent, e))?;

    // Beside the destination rather than in the temporary folder, so the swap at the end is a rename on
    // one file system and not a copy across two.
    let staging = parent.join(format!("{name}.partial"));
    let previous = parent.join(format!("{name}.old"));
    remove_if_there(&staging)?;
    fs::create_dir_all(&staging).map_err(|e| couldnt("make", &staging, e))?;

    if let Err(problem) = unpack(archive, kind, &staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(problem);
    }
    let root = single_folder_in(&staging).unwrap_or_else(|| staging.clone());

    // Moved aside rather than deleted first, so a failure between here and the rename below can put it
    // back. On Windows this is also the step that finds out the program is running: a folder with an open
    // file in it cannot be moved.
    remove_if_there(&previous)?;
    let had_one = destination.exists();
    if had_one {
        if let Err(e) = fs::rename(destination, &previous) {
            let _ = fs::remove_dir_all(&staging);
            return Err(Problem::Local(format!(
                "Could not replace the copy already in {}: {e}. If {product} is running, close it and \
                 try again.",
                destination.display()
            )));
        }
    }
    if let Err(e) = fs::rename(&root, destination) {
        if had_one {
            let _ = fs::rename(&previous, destination);
        }
        let _ = fs::remove_dir_all(&staging);
        return Err(couldnt("move the new copy into", destination, e));
    }

    // Both best effort. What is left is clutter, not a broken install.
    let _ = fs::remove_dir_all(&previous);
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    Ok(())
}

fn unpack(archive: &Path, kind: Kind, into: &Path) -> Result<(), Problem> {
    match kind {
        Kind::TarGz => unpack_tar_gz(archive, into),
        Kind::Zip => unpack_zip(archive, into),
    }
}

/// Unpacks [archive] into the folder [into] as it is, with nothing swapped and nothing replaced: what a
/// Mac's own `ditto -x -k` does, for the tests that stand in for it on machines that have none.
#[cfg(test)]
pub(crate) fn unpack_into(archive: &Path, into: &Path) -> Result<(), Problem> {
    let kind = kind_of(archive).ok_or_else(|| {
        Problem::Local(format!(
            "{} is neither a .zip nor a .tar.gz, so it cannot be unpacked.",
            archive.display()
        ))
    })?;
    fs::create_dir_all(into).map_err(|e| couldnt("make", into, e))?;
    unpack(archive, kind, into)
}

fn unpack_tar_gz(archive: &Path, into: &Path) -> Result<(), Problem> {
    let file = File::open(archive).map_err(|e| couldnt("open", archive, e))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(BufReader::new(file)));
    // The permission bits are kept -- the launcher has to stay executable -- but not setuid and its
    // relatives, which no music player has any business carrying.
    tar.set_preserve_permissions(false);
    tar.set_overwrite(true);
    // `unpack` refuses any entry that would land outside the folder, through `..` or an absolute path.
    tar.unpack(into).map_err(|e| {
        Problem::Local(format!(
            "{} could not be unpacked: {e}",
            archive_name(archive)
        ))
    })
}

fn unpack_zip(archive: &Path, into: &Path) -> Result<(), Problem> {
    let unreadable = |e: &dyn std::fmt::Display| {
        Problem::Local(format!(
            "{} could not be unpacked: {e}",
            archive_name(archive)
        ))
    };
    let file = File::open(archive).map_err(|e| couldnt("open", archive, e))?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| unreadable(&e))?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|e| unreadable(&e))?;
        // The name, checked: `enclosed_name` is None for anything that would climb out of the folder,
        // which is the one thing an unpacker must never do whatever the archive says.
        let Some(relative) = entry.enclosed_name() else {
            return Err(Problem::Local(format!(
                "{} contains {}, which points outside the folder it is unpacked into. Refusing to \
                 unpack it.",
                archive_name(archive),
                entry.name()
            )));
        };
        let target = into.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|e| couldnt("make", &target, e))?;
            continue;
        }
        if let Some(folder) = target.parent() {
            fs::create_dir_all(folder).map_err(|e| couldnt("make", folder, e))?;
        }
        let mut out = File::create(&target).map_err(|e| couldnt("write", &target, e))?;
        io::copy(&mut entry, &mut out).map_err(|e| unreadable(&e))?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&target, fs::Permissions::from_mode(mode & 0o777));
        }
    }
    Ok(())
}

/// The one folder an archive put at the top, if that is all it put there.
fn single_folder_in(folder: &Path) -> Option<PathBuf> {
    let mut entries = fs::read_dir(folder).ok()?;
    let only = entries.next()?.ok()?;
    if entries.next().is_some() || !only.file_type().ok()?.is_dir() {
        return None;
    }
    Some(only.path())
}

/// The program to run inside an unpacked Noctorium CLI.
///
/// Looked for rather than assumed. The Windows build is a jpackage image with `noctorium.exe` at the top,
/// the Linux one has `bin/noctorium`, and a build that moves it -- a `bin` folder on Windows too, or a
/// `.bat` wrapper -- should still install rather than fail on a path this program had written down.
pub fn launcher_in(folder: &Path, windows: bool) -> Option<PathBuf> {
    let candidates: &[&str] = if windows {
        &[
            "noctorium.exe",
            "bin/noctorium.exe",
            "noctorium.bat",
            "bin/noctorium.bat",
        ]
    } else {
        &["bin/noctorium", "noctorium"]
    };
    candidates
        .iter()
        .map(|relative| folder.join(relative))
        .find(|candidate| candidate.is_file())
}

pub(crate) fn remove_if_there(folder: &Path) -> Result<(), Problem> {
    if folder.exists() {
        fs::remove_dir_all(folder).map_err(|e| couldnt("clear away", folder, e))?;
    }
    Ok(())
}

fn archive_name(archive: &Path) -> String {
    archive
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| archive.display().to_string())
}

fn couldnt(what: &str, path: &Path, e: io::Error) -> Problem {
    Problem::Local(format!("Could not {what} {}: {e}", path.display()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A folder of the test's own, emptied first, so a run never sees what an earlier one left.
    pub(crate) fn scratch(label: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let folder = std::env::temp_dir().join(format!(
            "noctorium-installer-test-{}-{}-{label}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// A .tar.gz shaped like the published Linux archive: one `noctorium-cli/` folder at the top.
    pub(crate) fn tar_gz(at: &Path, files: &[(&str, &str, u32)]) {
        let file = File::create(at).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        for (path, contents, mode) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(*mode);
            header.set_cksum();
            builder
                .append_data(&mut header, path, contents.as_bytes())
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    }

    pub(crate) fn zip_file(at: &Path, files: &[(&str, &str)]) {
        let mut writer = zip::ZipWriter::new(File::create(at).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (path, contents) in files {
            if path.ends_with('/') {
                writer.add_directory(*path, options).unwrap();
            } else {
                writer.start_file(*path, options).unwrap();
                writer.write_all(contents.as_bytes()).unwrap();
            }
        }
        writer.finish().unwrap();
    }

    #[test]
    fn a_tar_gz_becomes_the_folder_with_its_launcher_still_executable() {
        let here = scratch("tar");
        let archive = here.join("noctorium-cli-1.0.0-linux-x64.tar.gz");
        tar_gz(
            &archive,
            &[
                (
                    "noctorium-cli/bin/noctorium",
                    "#!/bin/sh\necho one\n",
                    0o755,
                ),
                (
                    "noctorium-cli/lib/app/noctorium.cfg",
                    "[Application]\n",
                    0o644,
                ),
            ],
        );
        let destination = here.join("share").join("noctorium-cli");
        install_folder(&archive, &destination, "the Noctorium CLI").expect("should unpack");

        let launcher = destination.join("bin").join("noctorium");
        assert_eq!(
            fs::read_to_string(&launcher).unwrap(),
            "#!/bin/sh\necho one\n"
        );
        assert!(destination.join("lib/app/noctorium.cfg").is_file());
        assert_eq!(launcher_in(&destination, false), Some(launcher.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&launcher).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "the launcher must stay executable");
        }
    }

    /// The case the whole module exists for: an upgrade leaves nothing of the old version behind.
    #[test]
    fn replacing_an_older_copy_leaves_nothing_of_it() {
        let here = scratch("replace");
        let destination = here.join("noctorium-cli");
        let first = here.join("noctorium-cli-1.0.0-linux-x64.tar.gz");
        tar_gz(
            &first,
            &[
                ("noctorium-cli/bin/noctorium", "one", 0o755),
                ("noctorium-cli/lib/app/old-only.jar", "stale", 0o644),
            ],
        );
        install_folder(&first, &destination, "the Noctorium CLI").expect("first install");
        assert!(destination.join("lib/app/old-only.jar").is_file());

        let second = here.join("noctorium-cli-1.1.0-linux-x64.tar.gz");
        tar_gz(&second, &[("noctorium-cli/bin/noctorium", "two", 0o755)]);
        install_folder(&second, &destination, "the Noctorium CLI").expect("upgrade");

        assert_eq!(
            fs::read_to_string(destination.join("bin/noctorium")).unwrap(),
            "two"
        );
        assert!(
            !destination.join("lib/app/old-only.jar").exists(),
            "a jar the new version does not have must not survive the upgrade"
        );
        assert!(!here.join("noctorium-cli.old").exists());
        assert!(!here.join("noctorium-cli.partial").exists());
    }

    /// The Noctorium CLI's own updater swaps itself in beside the install folder, through folders named
    /// after it with `.update-` and `.old-` and something after them, and may leave one behind. They are
    /// its business: an install here goes on as usual, and leaves them exactly as they were.
    #[test]
    fn what_the_clis_own_updater_leaves_beside_it_is_left_alone() {
        let here = scratch("updater");
        let destination = here.join("Noctorium CLI");
        let first = here.join("noctorium-cli-1.0.0-windows-x64.zip");
        zip_file(&first, &[("noctorium-cli/noctorium.exe", "one")]);
        install_folder(&first, &destination, "the Noctorium CLI").expect("first install");

        let theirs = [
            here.join("Noctorium CLI.update-1a2b3c"),
            here.join("Noctorium CLI.old-1a2b3c"),
        ];
        for folder in &theirs {
            fs::create_dir_all(folder).unwrap();
            fs::write(folder.join("noctorium.exe"), "theirs").unwrap();
        }
        let second = here.join("noctorium-cli-1.1.0-windows-x64.zip");
        zip_file(&second, &[("noctorium-cli/noctorium.exe", "two")]);
        install_folder(&second, &destination, "the Noctorium CLI").expect("upgrade");

        assert_eq!(
            fs::read_to_string(destination.join("noctorium.exe")).unwrap(),
            "two"
        );
        for folder in &theirs {
            assert_eq!(
                fs::read_to_string(folder.join("noctorium.exe")).unwrap(),
                "theirs",
                "{}",
                folder.display()
            );
        }
        // Even when the updater was stopped half way, with the install folder moved aside and nothing put
        // back yet: this is a first install, as far as this can tell.
        fs::rename(&destination, here.join("Noctorium CLI.old-4d5e6f")).unwrap();
        install_folder(&second, &destination, "the Noctorium CLI")
            .expect("installs where there is nothing");
        assert!(destination.join("noctorium.exe").is_file());
        assert!(here
            .join("Noctorium CLI.old-4d5e6f/noctorium.exe")
            .is_file());
    }

    #[test]
    fn a_zip_becomes_the_folder_and_its_launcher_is_found_at_the_top() {
        let here = scratch("zip");
        let archive = here.join("noctorium-cli-1.0.0-windows-x64.zip");
        zip_file(
            &archive,
            &[
                ("noctorium-cli/", ""),
                ("noctorium-cli/noctorium.exe", "MZ"),
                ("noctorium-cli/app/noctorium.cfg", "[Application]"),
                ("noctorium-cli/runtime/release", "JAVA_VERSION=21"),
            ],
        );
        let destination = here.join("Programs").join("Noctorium CLI");
        install_folder(&archive, &destination, "the Noctorium CLI").expect("should unpack");

        assert_eq!(
            fs::read_to_string(destination.join("noctorium.exe")).unwrap(),
            "MZ"
        );
        assert!(destination.join("runtime/release").is_file());
        assert_eq!(
            launcher_in(&destination, true),
            Some(destination.join("noctorium.exe"))
        );
    }

    #[test]
    fn a_launcher_in_a_bin_folder_is_found_too() {
        let here = scratch("bin");
        fs::create_dir_all(here.join("bin")).unwrap();
        fs::write(here.join("bin/noctorium.bat"), "@echo off").unwrap();
        assert_eq!(
            launcher_in(&here, true),
            Some(here.join("bin/noctorium.bat"))
        );
        assert_eq!(launcher_in(&here, false), None);
    }

    #[test]
    fn an_archive_with_several_things_at_the_top_has_them_all_put_inside() {
        let here = scratch("flat");
        let archive = here.join("flat.zip");
        zip_file(&archive, &[("noctorium.exe", "MZ"), ("app/a.jar", "jar")]);
        let destination = here.join("Noctorium CLI");
        install_folder(&archive, &destination, "the Noctorium CLI").expect("should unpack");
        assert!(destination.join("noctorium.exe").is_file());
        assert!(destination.join("app/a.jar").is_file());
    }

    #[test]
    fn an_entry_that_climbs_out_of_the_folder_is_refused() {
        let here = scratch("escape");
        let archive = here.join("evil.zip");
        zip_file(&archive, &[("../escaped.txt", "nope")]);
        let destination = here.join("inside").join("Noctorium CLI");
        assert!(matches!(
            install_folder(&archive, &destination, "the Noctorium CLI"),
            Err(Problem::Local(_))
        ));
        assert!(!here.join("inside").join("escaped.txt").exists());
        assert!(!here.join("escaped.txt").exists());
        assert!(
            !destination.exists(),
            "nothing is put in place from an archive that was refused"
        );
    }

    #[test]
    fn something_that_is_not_an_archive_is_said_to_be_not_one() {
        let here = scratch("neither");
        let file = here.join("noctorium.rar");
        fs::write(&file, "x").unwrap();
        assert!(matches!(
            install_folder(&file, &here.join("out"), "the Noctorium CLI"),
            Err(Problem::Local(_))
        ));
    }
}
