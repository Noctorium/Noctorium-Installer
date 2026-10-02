//! Installs Noctorium on this machine: the part both installers share.
//!
//! Two small programs are built from this. `noctorium-installer` opens a window and is what most people
//! double click; `noctorium-installer-cli` is the same install in a terminal, with menus, flags for
//! scripts and a progress bar. Neither does anything this library does not: ask GitHub what the release
//! is, pick the file that belongs on this machine, check it against the checksum published beside it, and
//! hand it to whatever installs software here.
//!
//! The split inside follows the order things happen in. [`system`] looks at the machine and changes
//! nothing; [`github`] and [`fetch`] ask GitHub; [`flow`] turns the two into a plan and carries it out;
//! [`install`] and [`archive`] are the only parts that write anywhere a person would notice. [`cli`] is the
//! terminal front end, here rather than in its binary because the window program falls back to it on a
//! machine with no display.

pub mod archive;
pub mod cli;
pub mod fetch;
pub mod flow;
pub mod github;
pub mod install;
pub mod system;

/// This installer's own version, as `-V` reports it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
