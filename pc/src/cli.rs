//! The installer in a terminal: what `noctorium-installer-cli` is, and what the window program falls back
//! to on a machine with no display.
//!
//! It says what it found before it does anything -- the machine, the release, the file, the exact command
//! -- and asks. Everything it asks has a default, and `--yes` takes them all, so the same program serves
//! somebody at a prompt and a script that wants Noctorium on a fleet of machines. Colour is a courtesy:
//! off when output is not a terminal, when NO_COLOR is set, or when asked.

use crate::flow::{self, Format, Found, Offer, Options, Plan, Products, Step};
use crate::github::{Arch, Asset, Problem};
use crate::install::Method;
use crate::system::{Asking, Os};
use std::io::{self, BufRead, IsTerminal, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

pub const USAGE: &str = "\
Installs Noctorium, the music player, or the Noctorium CLI, from the latest release on GitHub. It shows
what it found and asks before it changes anything; every question has a default, which --yes takes.

Usage: noctorium-installer-cli [options]

  -y, --yes             ask nothing: take the defaults and install
      --product WHAT    desktop (the default), cli, or both
      --format HOW      Linux only: auto (the default), deb, rpm, arch, appimage or flatpak
      --version X.Y.Z   install release vX.Y.Z rather than the latest
      --dry-run         show what would be downloaded and run, and stop there
      --list            list the release's files and their sizes
      --no-color        plain text; NO_COLOR in the environment does the same
  -V, --about           this installer's own version
  -h, --help            this

  auto is the distribution's own package where it is Debian, Ubuntu, Fedora, openSUSE or Arch, or one of
  their relatives, and the AppImage everywhere else. Windows has its installer and a Mac its disk image,
  for Apple silicon or Intel as the Mac is, and neither takes --format.

Environment:
  GITHUB_TOKEN          raises GitHub's rate limit
  NOCTORIUM_REPOSITORY  the repository to install from, as owner/name
  NO_COLOR              no colour

Exit status: 0 when it did what was asked, 1 when it did not -- including when the answer to \"Install?\"
was no -- and 2 when the options could not be understood.

Examples:
  noctorium-installer-cli                          ask, then install
  noctorium-installer-cli --yes                    install Noctorium the way this machine prefers
  noctorium-installer-cli --product both -y        Noctorium and the Noctorium CLI
  noctorium-installer-cli --format appimage        the AppImage, whatever the distribution
  noctorium-installer-cli --version 0.6.0 --dry-run
";

/// What the command line asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    pub yes: bool,
    pub products: Option<Products>,
    pub format: Option<Format>,
    /// Without the leading v.
    pub version: Option<String>,
    pub dry_run: bool,
    pub list: bool,
    pub no_color: bool,
}

/// What to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Help,
    About,
    Install(Args),
}

/// A command line that could not be understood, and why, in a sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(pub String);

/// Reads the arguments, not including the program's own name.
///
/// Values go either after the flag or after an `=`: `--product cli` and `--product=cli` are the same. A
/// flag nobody knows is an error rather than something to skip: a script that misspells `--yes` should
/// find out now, not when it hangs waiting for an answer.
pub fn parse(arguments: &[String]) -> Result<Request, UsageError> {
    let mut args = Args::default();
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        index += 1;
        let (flag, inline) = match argument.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (argument.as_str(), None),
        };

        // Takes the flag's value, from after the = or from the next argument -- but not when the next
        // argument is another flag, which means the value was forgotten.
        let mut value = |what: &str| -> Result<String, UsageError> {
            if let Some(value) = inline.clone() {
                return Ok(value);
            }
            match arguments.get(index) {
                Some(next) if !next.starts_with('-') => {
                    index += 1;
                    Ok(next.clone())
                }
                _ => Err(UsageError(format!("{flag} needs {what}."))),
            }
        };

        let takes_no_value = |flag: &str| -> Result<(), UsageError> {
            match inline {
                Some(_) => Err(UsageError(format!("{flag} takes no value."))),
                None => Ok(()),
            }
        };

        match flag {
            "-h" | "--help" => return Ok(Request::Help),
            "-V" | "--about" => return Ok(Request::About),
            "-y" | "--yes" => {
                takes_no_value(flag)?;
                args.yes = true
            }
            "--dry-run" => {
                takes_no_value(flag)?;
                args.dry_run = true
            }
            "--list" => {
                takes_no_value(flag)?;
                args.list = true
            }
            "--no-color" | "--no-colour" => {
                takes_no_value(flag)?;
                args.no_color = true
            }
            "--product" => {
                let word = value("a product: desktop, cli or both")?;
                args.products = Some(Products::parse(&word).ok_or_else(|| {
                    UsageError(format!(
                        "--product takes desktop, cli or both, not \"{word}\"."
                    ))
                })?);
            }
            "--format" => {
                let word = value("a format: auto, deb, rpm, arch, appimage or flatpak")?;
                args.format = Some(Format::parse(&word).ok_or_else(|| {
                    UsageError(format!(
                        "--format takes auto, deb, rpm, arch, appimage or flatpak, not \"{word}\"."
                    ))
                })?);
            }
            "--version" => {
                let word = value(
                    "a release, such as --version 0.6.0 (this installer's own version is -V)",
                )?;
                let version = word.trim_start_matches('v');
                if !looks_like_a_version(version) {
                    return Err(UsageError(format!(
                        "\"{word}\" is not a version. Releases are numbered like 0.6.0."
                    )));
                }
                args.version = Some(version.to_string());
            }
            other if other.starts_with('-') => {
                return Err(UsageError(format!("{other} is not an option this knows.")))
            }
            other => {
                return Err(UsageError(format!(
                    "\"{other}\" is not an option. Options start with a dash."
                )))
            }
        }
    }
    Ok(Request::Install(args))
}

/// `1.2.3`, or `1.2.3-beta.1`: digits and dots, then anything a tag allows after a hyphen.
fn looks_like_a_version(version: &str) -> bool {
    let (numbers, suffix) = match version.split_once('-') {
        Some((numbers, suffix)) => (numbers, Some(suffix)),
        None => (version, None),
    };
    !numbers.is_empty()
        && numbers
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        && suffix.is_none_or(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        })
}

/// Who started this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    /// `noctorium-installer-cli`, at a terminal.
    Terminal,
    /// The window program, with `--cli` or with no display. It has always installed without asking
    /// anything there, and still does.
    Window,
}

/// Parses, runs, and turns the outcome into an exit status. The whole of either binary's terminal mode.
pub fn main(arguments: &[String], launch: Launch) -> ExitCode {
    let request = match parse(arguments) {
        Ok(request) => request,
        Err(UsageError(why)) => {
            let term = Term::detect(false, &io::stderr());
            eprintln!("{}", term.paint(Paint::Bad, &format!("{why}\n")));
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let mut args = match request {
        Request::Help => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Request::About => {
            println!("noctorium-installer {}", crate::VERSION);
            return ExitCode::SUCCESS;
        }
        Request::Install(args) => args,
    };
    if launch == Launch::Window {
        args.yes = true;
    }

    let term = Term::detect(args.no_color, &io::stdout());
    match run(&args, &term) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(why)) => {
            term.error(&why);
            eprintln!("\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::NoAnswer) => {
            term.error(
                "No answer came: standard input closed before the question was answered. Pass --yes to \
                 install without being asked.",
            );
            ExitCode::from(2)
        }
        Err(Failure::Declined) => {
            println!("\n  Nothing was installed.");
            ExitCode::FAILURE
        }
        Err(Failure::Problem(problem)) => {
            term.error(&problem.to_string());
            ExitCode::FAILURE
        }
    }
}

enum Failure {
    Problem(Problem),
    Usage(String),
    NoAnswer,
    Declined,
}

impl From<Problem> for Failure {
    fn from(problem: Problem) -> Self {
        Failure::Problem(problem)
    }
}

fn run(args: &Args, term: &Term) -> Result<(), Failure> {
    // Refused before GitHub is asked anything: this is a mistake in the command, and finding it out after
    // a round trip would be finding it out late. `auto` is what these systems do anyway, so it is let be.
    let this_system = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    };
    if let Some(name) = flow::only_one_way(this_system) {
        if !matches!(args.format, None | Some(Format::Auto)) {
            return Err(Failure::Usage(format!(
                "There is only one way to install Noctorium on {name}, so --format is for Linux."
            )));
        }
    }
    let interactive = !args.yes;
    term.banner();

    let asking_for = match &args.version {
        Some(version) => format!("v{version}"),
        None => "the latest release".into(),
    };
    term.status(&format!("Asking GitHub for {asking_for}..."));
    let found = flow::look(args.version.as_deref())?;
    term.clear_status();
    show_found(term, &found);

    if args.list {
        show_files(term, &found);
        return Ok(());
    }

    let products = match args.products {
        Some(products) => products,
        None if interactive => choose_products(term, &found)?,
        None => Products::Desktop,
    };
    let format = match args.format {
        Some(format) => format,
        None if interactive && products != Products::Cli && found.system.os == Os::Linux => {
            choose_format(term, &found)?
        }
        None => Format::Auto,
    };

    let options = Options {
        products,
        format,
        asking: Asking::Terminal {
            interactive: io::stdin().is_terminal(),
        },
    };
    let plan = flow::plan(&found, options)?;
    show_plan(term, &plan);

    if args.dry_run {
        println!(
            "\n  {}",
            term.paint(
                Paint::Dim,
                "A dry run: nothing has been downloaded, and nothing on this machine has changed."
            )
        );
        return Ok(());
    }
    if interactive && !confirm(term, "Install?")? {
        return Err(Failure::Declined);
    }

    carry_out(term, &plan)?;
    finish(term, &plan);
    Ok(())
}

// ---------------------------------------------------------------- what was found

fn show_found(term: &Term, found: &Found) {
    let system = &found.system;
    let mut parts = vec![system.describe()];
    match (system.os, system.arch) {
        // As Apple says it, which is how a Mac's owner knows it: About This Mac says Chip or Processor,
        // not an instruction set.
        (Os::MacOs, Some(Arch::Aarch64)) => parts.push("Apple silicon".into()),
        (Os::MacOs, Some(Arch::X86_64)) => parts.push("Intel".into()),
        (_, Some(arch)) => parts.push(arch.describe().to_string()),
        (_, None) => parts.push(system.arch_name.to_string()),
    }
    if system.os == Os::Linux {
        let mut tools: Vec<&str> = system
            .package_manager
            .map(|m| m.describe())
            .into_iter()
            .collect();
        if system.flatpak {
            tools.push("flatpak");
        }
        if tools.is_empty() {
            parts.push("no package manager this knows".into());
        } else {
            parts.push(tools.join(", "));
        }
    }
    term.field("System", &parts.join(&format!(" {} ", term.glyphs.dot)));

    let which = if found.latest {
        "the latest"
    } else {
        "as asked"
    };
    term.field(
        "Release",
        &format!(
            "{} {}",
            term.paint(Paint::Bold, &found.release.tag),
            term.paint(Paint::Dim, &format!("({which})"))
        ),
    );
}

fn show_files(term: &Term, found: &Found) {
    println!(
        "\n  {}",
        term.paint(Paint::Bold, &format!("Files in {}", found.release.tag))
    );
    let ours: Vec<(String, &'static str)> = flow::offers(found)
        .into_iter()
        .filter_map(|offer| offer.asset.map(|a| (a.name, "Noctorium")))
        .chain(cli_asset(found).map(|a| (a.name.clone(), "Noctorium CLI")))
        .collect();
    let width = found
        .release
        .assets
        .iter()
        .map(|a| a.name.chars().count())
        .max()
        .unwrap_or(0);
    for asset in &found.release.assets {
        let tag = ours
            .iter()
            .find(|(name, _)| *name == asset.name)
            .map(|(_, product)| {
                term.paint(
                    Paint::Accent,
                    &format!("  {} {product} here", term.glyphs.back),
                )
            })
            .unwrap_or_default();
        println!("    {:<width$}  {:>9}{tag}", asset.name, size(asset.size),);
    }
}

fn cli_asset(found: &Found) -> Option<&Asset> {
    let arch = found.system.arch?;
    let wanted = Method::cli_for(found.system.os).wanted();
    found.release.asset_for(wanted, arch)
}

// ---------------------------------------------------------------- questions

fn choose_products(term: &Term, found: &Found) -> Result<Products, Failure> {
    let cli = cli_asset(found).is_some();
    let missing = format!("Not in {} yet.", found.release.tag);
    let choices = [
        Choice {
            label: "Noctorium".into(),
            detail: "the music player, in a window".into(),
            unavailable: None,
            recommended: true,
        },
        Choice {
            label: "Noctorium CLI".into(),
            detail: "the same player, in a terminal".into(),
            unavailable: (!cli).then(|| missing.clone()),
            recommended: false,
        },
        Choice {
            label: "Both".into(),
            detail: String::new(),
            unavailable: (!cli).then(|| missing.clone()),
            recommended: false,
        },
    ];
    let picked = term.menu("What would you like to install?", &choices, 0)?;
    Ok([Products::Desktop, Products::Cli, Products::Both][picked])
}

fn choose_format(term: &Term, found: &Found) -> Result<Format, Failure> {
    let offers: Vec<Offer> = flow::offers(found);
    let available = offers.iter().filter(|o| o.unavailable.is_none()).count();
    // One way, or none, is not a question. The plan says which it is, or why there is none.
    if available < 2 {
        return Ok(Format::Auto);
    }
    let choices: Vec<Choice> = offers
        .iter()
        .map(|offer| Choice {
            label: offer.method.describe().into(),
            detail: offer.method.explain().into(),
            unavailable: offer.unavailable.clone(),
            recommended: offer.recommended,
        })
        .collect();
    let default = offers.iter().position(|o| o.recommended).unwrap_or(0);
    let picked = term.menu("How should Noctorium be installed?", &choices, default)?;
    Ok(Format::of(offers[picked].method))
}

struct Choice {
    label: String,
    detail: String,
    unavailable: Option<String>,
    recommended: bool,
}

fn confirm(term: &Term, question: &str) -> Result<bool, Failure> {
    loop {
        print!(
            "\n  {} {} ",
            term.paint(Paint::Bold, question),
            term.paint(Paint::Dim, "[Y/n]")
        );
        let answer = read_answer()?;
        match answer.to_ascii_lowercase().as_str() {
            "" | "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("  {}", term.paint(Paint::Dim, "y or n, please.")),
        }
    }
}

fn read_answer() -> Result<String, Failure> {
    let _ = io::stdout().flush();
    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => Err(Failure::NoAnswer),
        Ok(_) => {
            // PowerShell starts what it pipes with a byte-order mark, which is not part of the answer.
            let answer = line.trim().trim_start_matches('\u{FEFF}').to_string();
            // Echoed when the answer came from a pipe, so a transcript reads as a conversation.
            if !io::stdin().is_terminal() {
                println!("{answer}");
            }
            Ok(answer)
        }
    }
}

// ---------------------------------------------------------------- the plan

fn show_plan(term: &Term, plan: &Plan) {
    println!(
        "
  {}",
        term.paint(Paint::Bold, "The plan")
    );
    for item in &plan.items {
        println!(
            "    {} {}  {}",
            term.paint(Paint::AccentBold, item.product.name()),
            term.paint(Paint::Bold, plan.version()),
            term.paint(Paint::Dim, item.method.describe())
        );
        term.detail(
            "download",
            &format!(
                "{}  {}",
                item.asset.name,
                term.paint(Paint::Dim, &size(item.asset.size))
            ),
        );
        for (index, action) in item.actions.iter().enumerate() {
            term.detail(
                if index == 0 { "install" } else { "then" },
                &action.describe(&plan.places),
            );
        }
    }
    if plan.items.len() > 1 {
        term.detail("in all", &size((plan.megabytes() * 1_048_576.0) as u64));
    }
    for note in &plan.notes {
        println!();
        term.warn(note);
    }
    if let Some(tool) = plan.items.iter().find_map(|i| i.escalation) {
        println!();
        term.say(&format!(
            "Installing a package needs administrator rights, so {tool} will ask for your password."
        ));
    }
}

fn carry_out(term: &Term, plan: &Plan) -> Result<(), Problem> {
    let mut bar: Option<Bar> = None;
    let result = flow::carry_out(plan, &mut |step| match step {
        Step::Downloading { item, done, total } => {
            let bar = bar.get_or_insert_with(|| {
                println!(
                    "\n  {} {}",
                    term.paint(Paint::Accent, term.glyphs.down),
                    term.paint(Paint::Bold, &plan.items[item].asset.name)
                );
                Bar::new()
            });
            bar.update(term, done, total);
        }
        Step::Verified { hash, .. } => {
            if let Some(mut finished) = bar.take() {
                finished.finish(term);
            }
            let short = format!("{}...{}", &hash[..8], &hash[hash.len() - 8..]);
            println!(
                "  {} Checksum matches SHA256SUMS.txt  {}",
                term.paint(Paint::Good, term.glyphs.tick),
                term.paint(Paint::Dim, &short)
            );
        }
        Step::Installing { command, .. } => {
            println!(
                "  {} {}",
                term.paint(Paint::Accent, term.glyphs.arrow),
                term.paint(Paint::Bold, &command)
            );
            let _ = io::stdout().flush();
        }
        Step::Installed { item } => {
            println!(
                "  {} {} is installed.",
                term.paint(Paint::Good, term.glyphs.tick),
                plan.items[item].product.name()
            );
        }
    });
    // A download that stopped half way leaves its bar mid-line; the error goes underneath it.
    if bar.is_some() && result.is_err() {
        println!();
    }
    result
}

fn finish(term: &Term, plan: &Plan) {
    println!();
    for item in &plan.items {
        println!(
            "  {} {}",
            term.paint(
                Paint::AccentBold,
                &format!("{} {}", item.product.name(), plan.version())
            ),
            term.paint(Paint::Bold, "is ready.")
        );
        term.say(&item.start);
    }
}

// ---------------------------------------------------------------- progress

/// The download line: a bar, how much, how fast, and how long is left.
struct Bar {
    started: Instant,
    drawn: Option<Instant>,
    /// The last sample the speed was worked out from.
    sample: (Instant, u64),
    /// Bytes a second, smoothed so one slow second does not swing the estimate by a minute.
    rate: Option<f64>,
    /// How much of the line was written last time, so a shorter line can cover it.
    width: usize,
    /// For output that is not a terminal: the last tenth reported.
    tenth: u64,
    done: u64,
    total: u64,
}

impl Bar {
    fn new() -> Bar {
        let now = Instant::now();
        Bar {
            started: now,
            drawn: None,
            sample: (now, 0),
            rate: None,
            width: 0,
            tenth: 0,
            done: 0,
            total: 0,
        }
    }

    fn update(&mut self, term: &Term, done: u64, total: u64) {
        self.done = done;
        self.total = total;
        let now = Instant::now();
        let (then, before) = self.sample;
        let elapsed = now.duration_since(then).as_secs_f64();
        if elapsed >= 0.5 {
            let instant = done.saturating_sub(before) as f64 / elapsed;
            self.rate = Some(match self.rate {
                Some(rate) => rate * 0.7 + instant * 0.3,
                None => instant,
            });
            self.sample = (now, done);
        }

        if !term.live {
            // A log gets a line every tenth, not a redraw it cannot show.
            let tenth = (done * 10).checked_div(total).unwrap_or(0);
            if tenth > self.tenth && tenth < 10 {
                self.tenth = tenth;
                println!(
                    "    {:>3}%  {} of {}",
                    tenth * 10,
                    megabytes(done),
                    megabytes(total)
                );
            }
            return;
        }
        let due = self
            .drawn
            .is_none_or(|drawn| now.duration_since(drawn) >= Duration::from_millis(100));
        if due || (total > 0 && done >= total) {
            self.drawn = Some(now);
            self.draw(term, false);
        }
    }

    fn draw(&mut self, term: &Term, finished: bool) {
        let fraction = if self.total > 0 {
            (self.done as f64 / self.total as f64).min(1.0)
        } else {
            0.0
        };
        const CELLS: usize = 24;
        let filled = (fraction * CELLS as f64).round() as usize;
        let bar = format!(
            "{}{}",
            term.paint(Paint::Accent, &term.glyphs.full.repeat(filled)),
            term.paint(Paint::Dim, &term.glyphs.empty.repeat(CELLS - filled))
        );
        let speed = self
            .rate
            .filter(|r| *r > 0.0)
            .map(|r| format!("{}/s", megabytes(r as u64)))
            .unwrap_or_default();
        let tail = if finished {
            format!("in {}", duration(self.started.elapsed().as_secs_f64()))
        } else {
            match self.rate.filter(|r| *r > 0.0) {
                Some(rate) if self.total > self.done => {
                    format!("{} left", duration((self.total - self.done) as f64 / rate))
                }
                _ => String::new(),
            }
        };
        let numbers = format!(
            "{:>3.0}%  {} / {}  {speed}  {tail}",
            fraction * 100.0,
            megabytes_bare(self.done),
            megabytes(self.total)
        );
        let numbers = numbers.trim_end();
        let visible = 4 + CELLS + 2 + numbers.chars().count();
        let padding = " ".repeat(self.width.saturating_sub(visible));
        print!("\r    {bar}  {numbers}{padding}");
        self.width = visible;
        let _ = io::stdout().flush();
    }

    fn finish(&mut self, term: &Term) {
        if term.live {
            self.draw(term, true);
            println!();
        } else {
            let seconds = self.started.elapsed().as_secs_f64();
            println!(
                "    100%  {} in {}",
                megabytes(self.done),
                duration(seconds)
            );
        }
    }
}

fn size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        megabytes(bytes)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{} MB", megabytes_bare(bytes))
}

fn megabytes_bare(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_048_576.0)
}

fn duration(seconds: f64) -> String {
    let seconds = seconds.round() as u64;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

// ---------------------------------------------------------------- the terminal itself

/// What is drawn with: the character set this console can be trusted to show.
struct Glyphs {
    mark: &'static str,
    tick: &'static str,
    cross: &'static str,
    warn: &'static str,
    arrow: &'static str,
    down: &'static str,
    back: &'static str,
    dot: &'static str,
    full: &'static str,
    empty: &'static str,
}

/// Everything here is in WGL4, the set every Windows console font has drawn since Windows 95, except the
/// tick, which Consolas lacks -- so the old console host gets a square root sign instead, which reads the
/// same and does not turn into a box.
const RICH: Glyphs = Glyphs {
    mark: "\u{266B}",
    tick: "\u{2713}",
    cross: "\u{00D7}",
    warn: "!",
    arrow: "\u{2192}",
    down: "\u{2193}",
    back: "\u{2190}",
    dot: "\u{00B7}",
    full: "\u{2588}",
    empty: "\u{2591}",
};

const CONSERVATIVE: Glyphs = Glyphs {
    tick: "\u{221A}",
    ..RICH
};

#[derive(Clone, Copy)]
enum Paint {
    Accent,
    AccentBold,
    Bold,
    Dim,
    Good,
    Warn,
    Bad,
}

struct Term {
    colour: bool,
    /// Twenty-four bit colour, rather than the 256-colour approximation of it.
    truecolor: bool,
    /// Whether output is a terminal that a line can be redrawn on.
    live: bool,
    width: usize,
    glyphs: &'static Glyphs,
}

impl Term {
    fn detect(no_color: bool, stream: &dyn IsTerminal) -> Term {
        let live = stream.is_terminal();
        let asked_not = no_color
            || std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
            || std::env::var("TERM").is_ok_and(|t| t == "dumb");
        let colour = live && !asked_not && console::enable_colour();
        let truecolor = cfg!(windows)
            || std::env::var("COLORTERM").is_ok_and(|c| c == "truecolor" || c == "24bit");
        // Windows Terminal and the editors' terminals draw the tick; the old console host does not.
        let rich = !cfg!(windows)
            || std::env::var_os("WT_SESSION").is_some()
            || std::env::var_os("TERM_PROGRAM").is_some();
        Term {
            colour,
            truecolor,
            live,
            width: console::width().clamp(40, 110),
            glyphs: if rich { &RICH } else { &CONSERVATIVE },
        }
    }

    fn paint(&self, paint: Paint, text: &str) -> String {
        if !self.colour || text.is_empty() {
            return text.to_string();
        }
        let code = match (paint, self.truecolor) {
            // Noctorium's accent, #B47CFF, and 141 is the nearest of the 256.
            (Paint::Accent, true) => "38;2;180;124;255",
            (Paint::Accent, false) => "38;5;141",
            (Paint::AccentBold, true) => "1;38;2;180;124;255",
            (Paint::AccentBold, false) => "1;38;5;141",
            (Paint::Bold, _) => "1",
            (Paint::Dim, true) => "38;2;150;142;166",
            (Paint::Dim, false) => "38;5;246",
            (Paint::Good, true) => "38;2;124;217;146",
            (Paint::Good, false) => "38;5;114",
            (Paint::Warn, true) => "38;2;255;200;87",
            (Paint::Warn, false) => "38;5;221",
            (Paint::Bad, true) => "38;2;255;110;110",
            (Paint::Bad, false) => "38;5;203",
        };
        format!("\x1b[{code}m{text}\x1b[0m")
    }

    fn banner(&self) {
        println!();
        println!(
            "  {} {}  {}",
            self.paint(Paint::AccentBold, self.glyphs.mark),
            self.paint(Paint::AccentBold, "Noctorium"),
            self.paint(Paint::Dim, &format!("installer {}", crate::VERSION))
        );
        println!();
    }

    /// A line that is replaced by whatever comes next, when it can be.
    fn status(&self, text: &str) {
        if self.live {
            print!("  {}", self.paint(Paint::Dim, text));
            let _ = io::stdout().flush();
        } else {
            println!("  {text}");
        }
    }

    fn clear_status(&self) {
        if self.live {
            print!("\r{}\r", " ".repeat(self.width.saturating_sub(1)));
        }
    }

    fn field(&self, name: &str, value: &str) {
        println!(
            "  {}  {value}",
            self.paint(Paint::Dim, &format!("{name:<8}"))
        );
    }

    fn detail(&self, name: &str, value: &str) {
        println!(
            "      {}  {value}",
            self.paint(Paint::Dim, &format!("{name:<8}"))
        );
    }

    fn say(&self, text: &str) {
        for line in wrap(text, self.width.saturating_sub(4)) {
            println!("  {line}");
        }
    }

    fn warn(&self, text: &str) {
        let lines = wrap(text, self.width.saturating_sub(6));
        for (index, line) in lines.iter().enumerate() {
            let mark = if index == 0 { self.glyphs.warn } else { " " };
            println!("  {} {line}", self.paint(Paint::Warn, mark));
        }
    }

    fn error(&self, text: &str) {
        let colour = self.colour && io::stderr().is_terminal();
        let painted = |t: &str| {
            if colour {
                self.paint(Paint::Bad, t)
            } else {
                t.to_string()
            }
        };
        eprintln!();
        for (index, line) in wrap(text, self.width.saturating_sub(6)).iter().enumerate() {
            let mark = if index == 0 { self.glyphs.cross } else { " " };
            eprintln!("  {} {line}", painted(mark));
        }
    }

    /// A numbered list and a prompt, asked until the answer is one of the numbers.
    fn menu(&self, question: &str, choices: &[Choice], default: usize) -> Result<usize, Failure> {
        println!(
            "
  {}",
            self.paint(Paint::Bold, question)
        );
        let width = choices
            .iter()
            .map(|c| c.label.chars().count())
            .max()
            .unwrap_or(0);
        for (index, choice) in choices.iter().enumerate() {
            let number = self.paint(Paint::Accent, &format!("{}", index + 1));
            let label = format!("{:<width$}", choice.label);
            match &choice.unavailable {
                Some(why) => println!(
                    "    {number}  {}  {}",
                    self.paint(Paint::Dim, &label),
                    self.paint(Paint::Dim, why)
                ),
                None => {
                    let recommended = if choice.recommended {
                        self.paint(Paint::Good, "  recommended")
                    } else {
                        String::new()
                    };
                    println!(
                        "    {number}  {label}  {}{recommended}",
                        self.paint(Paint::Dim, &choice.detail)
                    );
                }
            }
        }
        loop {
            print!(
                "  {} {} ",
                self.paint(Paint::Bold, &format!("Choose 1-{}", choices.len())),
                self.paint(Paint::Dim, &format!("[{}]", default + 1))
            );
            let answer = read_answer()?;
            let picked = if answer.is_empty() {
                Some(default)
            } else {
                answer
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=choices.len()).contains(n))
                    .map(|n| n - 1)
            };
            match picked {
                Some(index) => match &choices[index].unavailable {
                    Some(why) => println!(
                        "  {}",
                        self.paint(Paint::Warn, &format!("{why} Choose another."))
                    ),
                    None => return Ok(index),
                },
                None => println!(
                    "  {}",
                    self.paint(
                        Paint::Dim,
                        &format!("A number from 1 to {}, please.", choices.len())
                    )
                ),
            }
        }
    }
}

/// Splits [text] into lines no wider than [width], at spaces, keeping any line breaks it already has.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(20);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

/// The parts that differ between Windows and everything else.
mod console {
    /// Colour needs the console told to read escape sequences on Windows, which it does not by default
    /// in the old console host. Where it will not, there is no colour rather than a screen of `[38;2;`.
    #[cfg(windows)]
    pub fn enable_colour() -> bool {
        use windows_sys::Win32::System::Console::{
            GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
        };
        let mut all = true;
        for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: plain calls on this process's own standard handles.
            unsafe {
                let handle = GetStdHandle(which);
                let mut mode = 0;
                if GetConsoleMode(handle, &mut mode) == 0 {
                    // Not a console -- redirected -- which is fine for stderr and decided elsewhere.
                    continue;
                }
                if SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) == 0 {
                    all = false;
                }
            }
        }
        all
    }

    #[cfg(not(windows))]
    pub fn enable_colour() -> bool {
        true
    }

    /// Columns, from the terminal, then COLUMNS, then the traditional eighty.
    pub fn width() -> usize {
        if let Some(columns) = asked() {
            return columns;
        }
        std::env::var("COLUMNS")
            .ok()
            .and_then(|c| c.parse().ok())
            .unwrap_or(80)
    }

    #[cfg(windows)]
    fn asked() -> Option<usize> {
        use windows_sys::Win32::System::Console::{
            GetConsoleScreenBufferInfo, GetStdHandle, CONSOLE_SCREEN_BUFFER_INFO, STD_OUTPUT_HANDLE,
        };
        // SAFETY: the structure is plain data and is only read after the call says it filled it in.
        unsafe {
            let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
            if GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info) == 0 {
                return None;
            }
            let columns = (info.srWindow.Right - info.srWindow.Left + 1) as usize;
            (columns > 0).then_some(columns)
        }
    }

    #[cfg(unix)]
    fn asked() -> Option<usize> {
        // SAFETY: TIOCGWINSZ writes a winsize, which is plain data, and nothing else.
        unsafe {
            let mut size: libc::winsize = std::mem::zeroed();
            if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) != 0 {
                return None;
            }
            (size.ws_col > 0).then_some(size.ws_col as usize)
        }
    }

    #[cfg(not(any(windows, unix)))]
    fn asked() -> Option<usize> {
        None
    }

    /// Whether this console window exists only because this program was double clicked, and so will
    /// vanish the moment it exits, taking whatever it said with it.
    #[cfg(windows)]
    pub fn alone_in_its_window() -> bool {
        use windows_sys::Win32::System::Console::GetConsoleProcessList;
        let mut processes = [0u32; 2];
        // SAFETY: the buffer is the length passed.
        let count = unsafe { GetConsoleProcessList(processes.as_mut_ptr(), 2) };
        count == 1
    }

    #[cfg(not(windows))]
    pub fn alone_in_its_window() -> bool {
        false
    }
}

/// Holds a double-clicked console open until it has been read.
///
/// Started from Explorer, a console program gets a window of its own that closes the instant it exits --
/// with whatever it just said, which after an install is the only explanation of how to start the thing.
/// Started from a terminal, the terminal stays and this does nothing.
pub fn hold_if_double_clicked() {
    if console::alone_in_its_window() && std::env::var_os("NOCTORIUM_NO_PAUSE").is_none() {
        print!("\n  Press Enter to close.");
        let _ = io::stdout().flush();
        let mut line = String::new();
        let _ = io::stdin().read_line(&mut line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    fn install(line: &str) -> Args {
        match parse(&arguments(line)) {
            Ok(Request::Install(args)) => args,
            other => panic!("{line}: expected an install, got {other:?}"),
        }
    }

    fn refused(line: &str) -> String {
        match parse(&arguments(line)) {
            Err(UsageError(why)) => why,
            other => panic!("{line}: expected a usage error, got {other:?}"),
        }
    }

    #[test]
    fn nothing_at_all_means_ask_then_install_the_desktop_application() {
        assert_eq!(install(""), Args::default());
    }

    #[test]
    fn every_flag_is_read() {
        let args = install(
            "--yes --product both --format appimage --version 0.6.0 --dry-run --list --no-color",
        );
        assert_eq!(
            args,
            Args {
                yes: true,
                products: Some(Products::Both),
                format: Some(Format::AppImage),
                version: Some("0.6.0".into()),
                dry_run: true,
                list: true,
                no_color: true,
            }
        );
        assert!(install("-y").yes);
        assert!(install("--no-colour").no_color, "and in British");
    }

    #[test]
    fn values_go_after_the_flag_or_after_an_equals_sign() {
        assert_eq!(install("--product=cli").products, Some(Products::Cli));
        assert_eq!(install("--format=deb").format, Some(Format::Deb));
        assert_eq!(install("--version=1.2.3").version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn a_version_may_be_written_with_its_v_and_a_suffix() {
        assert_eq!(
            install("--version v0.6.0").version.as_deref(),
            Some("0.6.0")
        );
        assert_eq!(
            install("--version 1.2.3-beta.1").version.as_deref(),
            Some("1.2.3-beta.1")
        );
    }

    #[test]
    fn help_and_about_answer_whatever_else_is_there() {
        assert_eq!(parse(&arguments("--yes --help")), Ok(Request::Help));
        assert_eq!(parse(&arguments("-h")), Ok(Request::Help));
        assert_eq!(parse(&arguments("-V")), Ok(Request::About));
        assert_eq!(parse(&arguments("--about")), Ok(Request::About));
    }

    #[test]
    fn an_unknown_flag_is_an_error_not_something_to_skip() {
        assert!(refused("--yse").contains("--yse"));
        assert!(refused("--cli").contains("--cli"));
        assert!(refused("install").contains("Options start with a dash"));
    }

    #[test]
    fn a_flag_without_its_value_says_what_it_wanted() {
        assert!(refused("--product").contains("desktop, cli or both"));
        assert!(refused("--format --yes").contains("auto, deb, rpm"));
        let why = refused("--version");
        assert!(
            why.contains("--version 0.6.0") && why.contains("-V"),
            "{why}"
        );
    }

    #[test]
    fn a_value_that_is_not_one_is_refused() {
        assert!(refused("--product everything").contains("everything"));
        assert!(refused("--format snap").contains("snap"));
        assert!(refused("--version latest").contains("not a version"));
        assert!(refused("--version 1..2").contains("not a version"));
        assert!(refused("--version 1.2-").contains("not a version"));
        assert!(refused("--yes=no").contains("takes no value"));
    }

    #[test]
    fn versions_are_numbers_and_dots_with_an_optional_suffix() {
        assert!(looks_like_a_version("0.6.0"));
        assert!(looks_like_a_version("10"));
        assert!(looks_like_a_version("1.2.3-rc.1"));
        assert!(!looks_like_a_version(""));
        assert!(!looks_like_a_version("a.b"));
        assert!(!looks_like_a_version("1.2.3-"));
        assert!(!looks_like_a_version("1.2/3"));
    }

    #[test]
    fn long_sentences_are_wrapped_at_spaces_and_keep_their_breaks() {
        let lines = wrap(
            "one two three four five six seven eight nine ten eleven twelve\nnext",
            20,
        );
        assert_eq!(
            lines,
            vec![
                "one two three four",
                "five six seven eight",
                "nine ten eleven",
                "twelve",
                "next"
            ]
        );
        assert!(lines.iter().all(|l| l.chars().count() <= 20));
    }

    #[test]
    fn sizes_and_durations_read_naturally() {
        assert_eq!(size(254_193_856), "242.4 MB");
        assert_eq!(size(769), "769 bytes");
        assert_eq!(size(4096), "4 KB");
        assert_eq!(duration(7.4), "7s");
        assert_eq!(duration(65.0), "1m 05s");
        assert_eq!(duration(3_725.0), "1h 02m");
    }
}
