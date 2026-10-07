//! The installer as a window.
//!
//! Immediate mode, which suits this exactly: the whole interface is a function of how far the install has
//! got, and "how far" arrives as messages from the thread doing the work. There is no widget tree to keep
//! in step with a download, and nothing to forget to update.
//!
//! The colours are Noctorium's own Dusk theme, so the thing that installs the player looks like the
//! player. Pure black would be the Night theme and is too hard an edge for a small window.

use eframe::egui;
use noctorium_installer::fetch::Fetched;
use noctorium_installer::flow::{
    self, Format, Found, Offer, OnDisk, Options, Plan, Product, Products, Step,
};
use noctorium_installer::github::Problem;
use noctorium_installer::install::{Action, Method};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

#[cfg(test)]
mod offscreen;

/// The mark, decoded by the build script into raw pixels.
const MARK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mark.rgba"));
include!(concat!(env!("OUT_DIR"), "/mark.rs"));

const BACKGROUND: egui::Color32 = egui::Color32::from_rgb(0x0D, 0x0B, 0x12);
const CARD: egui::Color32 = egui::Color32::from_rgb(0x1D, 0x18, 0x26);
const TEXT: egui::Color32 = egui::Color32::from_rgb(0xF3, 0xF1, 0xF8);
const SUBTEXT: egui::Color32 = egui::Color32::from_rgb(0xC9, 0xC2, 0xD6);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0xB4, 0x7C, 0xFF);
/// Writing on the accent. The accent is a pale violet, so what sits on it has to be dark.
const ON_ACCENT: egui::Color32 = egui::Color32::from_rgb(0x1A, 0x14, 0x25);

/// The window's size. Taller on Linux, which has a row for choosing the format; the content scrolls if a
/// release ever has more to say than fits.
const SIZE: [f32; 2] = [470.0, if cfg!(windows) { 500.0 } else { 550.0 }];

/// Opens the window and returns whether everything that was chosen ended up installed.
///
/// The error is only ever "no window could be opened" -- everything that can go wrong with an install is
/// shown in the window itself, where the person who asked for it is looking.
pub fn run() -> Result<bool, String> {
    let installed = Arc::new(AtomicBool::new(false));
    let flag = installed.clone();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(SIZE)
            .with_resizable(false)
            .with_title("Install Noctorium")
            .with_icon(egui::IconData {
                rgba: MARK.to_vec(),
                width: MARK_SIDE as u32,
                height: MARK_SIDE as u32,
            }),
        ..Default::default()
    };

    eframe::run_native(
        "Noctorium installer",
        options,
        Box::new(move |cc| {
            let mut gui = Gui::new(&cc.egui_ctx, flag);
            gui.look(&cc.egui_ctx);
            Ok(Box::new(gui))
        }),
    )
    .map_err(|e| e.to_string())?;

    Ok(installed.load(Ordering::Relaxed))
}

/// What the working thread has to say for itself.
enum Message {
    Found(Box<Found>),
    Progress(Step),
    Finished(Result<(), Problem>),
}

/// Where the install has got to, which is the whole of what the window draws.
enum Stage {
    Looking,
    Ready(Box<Choosing>),
    /// Downloading and installing, a row for each product, and then how each of them ended.
    Working(Box<Working>),
    /// Something stopped the install before any product was started on: no network, no release, or
    /// Noctorium open.
    Failed(String),
}

/// What somebody chose, kept apart from what was found so that trying again after a failure starts from
/// the same choice rather than from the defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Wanted {
    desktop: bool,
    cli: bool,
    stats: bool,
    /// The .msi's own wizard, for choosing the folder. Windows only.
    wizard: bool,
}

impl Default for Wanted {
    fn default() -> Self {
        Wanted {
            desktop: true,
            cli: false,
            stats: false,
            wizard: false,
        }
    }
}

impl Wanted {
    fn products(&self) -> Option<Products> {
        Products::from_choice(self.desktop, self.cli, self.stats)
    }
}

/// What was found, what has been chosen, and what that comes to.
///
/// The plan is remade from what was found whenever the choice changes, which asks GitHub nothing: the
/// release and its checksums were fetched once, when the window opened.
struct Choosing {
    found: Found,
    /// The ways this machine can install Noctorium. One on Windows; on Linux the distribution's package,
    /// the AppImage and the Flatpak, each saying whether this release has it.
    offers: Vec<Offer>,
    chosen: usize,
    wanted: Wanted,
    /// The Noctorium CLI's download, or nothing when this release has none for this machine.
    cli_size: Option<u64>,
    /// Noctorium Stats' download, or nothing when this release has none for this machine.
    stats_size: Option<u64>,
    plan: Result<Plan, String>,
}

impl Choosing {
    fn new(found: Found, wanted: Wanted) -> Choosing {
        let offers = flow::offers(&found);
        let chosen = offers.iter().position(|o| o.recommended).unwrap_or(0);
        let cli_size = flow::cli_asset(&found).map(|a| a.size);
        let stats_size = flow::stats_asset(&found).map(|a| a.size);
        let mut choosing = Choosing {
            found,
            offers,
            chosen,
            wanted: Wanted {
                cli: wanted.cli && cli_size.is_some(),
                stats: wanted.stats && stats_size.is_some(),
                ..wanted
            },
            cli_size,
            stats_size,
            plan: Err(String::new()),
        };
        choosing.replan();
        choosing
    }

    fn replan(&mut self) {
        let Some(products) = self.wanted.products() else {
            self.plan = Err(
                "Choose Noctorium, the Noctorium CLI or Noctorium Stats -- or more than one."
                    .into(),
            );
            return;
        };
        // The recommended one is what auto picks, and is asked for as auto, so it is decided in one
        // place; anything else is asked for by name.
        let format = match self.offers.get(self.chosen) {
            Some(offer) if !offer.recommended => Format::of(offer.method),
            _ => Format::Auto,
        };
        let options = Options {
            products,
            format,
            wizard: self.wanted.wizard && self.wizard_possible(),
            ..Options::default()
        };
        self.plan = flow::plan(&self.found, options).map_err(|problem| problem.to_string());
    }

    /// Whether there is a wizard to offer: Noctorium chosen, on Windows, from an .msi. The setup .exe of
    /// a release with no .msi is nothing but its wizard, and needs no box ticked to get it.
    fn wizard_possible(&self) -> bool {
        self.wanted.desktop
            && self
                .offers
                .get(self.chosen)
                .is_some_and(|o| o.method == Method::WindowsMsi && o.unavailable.is_none())
    }

    fn desktop_size(&self) -> Option<u64> {
        self.offers
            .get(self.chosen)
            .and_then(|o| o.asset.as_ref())
            .map(|a| a.size)
    }
}

/// The install, once it has been started: the plan it was started with, and how far each product is.
struct Working {
    plan: Plan,
    rows: Vec<Row>,
    /// Set when the work has finished, one way or another.
    finished: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Row {
    Downloading { done: u64, total: u64 },
    Checked(Fetched),
    Installing,
    Installed(Option<String>),
    Failed(String),
}

impl Working {
    fn new(plan: Plan) -> Working {
        let rows = plan
            .items
            .iter()
            .map(|item| Row::Downloading {
                done: match item.on_disk {
                    OnDisk::Part(have) => have,
                    _ => 0,
                },
                total: item.asset.size,
            })
            .collect();
        Working {
            plan,
            rows,
            finished: false,
        }
    }

    fn step(&mut self, step: Step) {
        match step {
            Step::Downloading { item, done, total } => {
                self.rows[item] = Row::Downloading { done, total }
            }
            Step::Verified { item, how } => self.rows[item] = Row::Checked(how),
            Step::Installing { item, .. } => self.rows[item] = Row::Installing,
            Step::Installed { item, note } => self.rows[item] = Row::Installed(note),
            Step::Failed { item, why } => self.rows[item] = Row::Failed(why),
        }
    }

    fn all_installed(&self) -> bool {
        self.rows.iter().all(|row| matches!(row, Row::Installed(_)))
    }

    /// The product being installed right now, if one is.
    fn installing(&self) -> Option<usize> {
        self.rows.iter().position(|row| *row == Row::Installing)
    }
}

struct Gui {
    stage: Stage,
    events: Receiver<Message>,
    sender: Sender<Message>,
    mark: egui::TextureHandle,
    wanted: Wanted,
    installed: Arc<AtomicBool>,
}

impl Gui {
    fn new(ctx: &egui::Context, installed: Arc<AtomicBool>) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = BACKGROUND;
        visuals.window_fill = BACKGROUND;
        // What a progress bar's unfilled half is drawn in, and the inside of a checkbox.
        visuals.extreme_bg_color = CARD;
        visuals.override_text_color = Some(TEXT);
        visuals.selection.bg_fill = ACCENT;
        ctx.set_visuals(visuals);

        let image = egui::ColorImage::from_rgba_unmultiplied([MARK_SIDE, MARK_SIDE], MARK);
        let mark = ctx.load_texture("noctorium-mark", image, egui::TextureOptions::LINEAR);

        let (sender, events) = channel();
        Self {
            stage: Stage::Looking,
            events,
            sender,
            mark,
            wanted: Wanted::default(),
            installed,
        }
    }

    /// Asks GitHub what there is to install.
    ///
    /// Done on opening rather than behind a button, because knowing what is on offer is the first thing
    /// anybody wants and asking changes nothing on the machine. Also what a failed attempt goes back to.
    fn look(&mut self, ctx: &egui::Context) {
        self.stage = Stage::Looking;
        let sender = self.sender.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let message = match flow::look(None) {
                Ok(found) => Message::Found(Box::new(found)),
                Err(problem) => Message::Finished(Err(problem)),
            };
            let _ = sender.send(message);
            repaint.request_repaint();
        });
    }

    /// Starts the downloads and the installs on a thread of their own.
    ///
    /// On this thread it would freeze the window for the length of a three hundred megabyte download,
    /// which looks exactly like a program that has crashed.
    fn begin(&mut self, ctx: &egui::Context, plan: Plan) {
        self.stage = Stage::Working(Box::new(Working::new(plan.clone())));
        let sender = self.sender.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let outcome = flow::carry_out(&plan, &mut |step| {
                let _ = sender.send(Message::Progress(step));
                repaint.request_repaint();
            });
            let _ = sender.send(Message::Finished(outcome));
            repaint.request_repaint();
        });
    }

    fn take_messages(&mut self) {
        while let Ok(message) = self.events.try_recv() {
            match (message, &mut self.stage) {
                (Message::Found(found), _) => {
                    self.stage = Stage::Ready(Box::new(Choosing::new(*found, self.wanted)))
                }
                (Message::Progress(step), Stage::Working(working)) => working.step(step),
                (Message::Progress(_), _) => {}
                (Message::Finished(outcome), Stage::Working(working)) => {
                    working.finished = true;
                    let installed = working.all_installed();
                    self.installed.store(installed, Ordering::Relaxed);
                    // A failure no product was told of is one that stopped everything before it began,
                    // and is said on its own rather than under products that never started.
                    let said = working.rows.iter().any(|row| matches!(row, Row::Failed(_)));
                    if let (Err(problem), false) = (outcome, said) {
                        self.stage = Stage::Failed(problem.to_string());
                    }
                }
                (Message::Finished(Err(problem)), _) => {
                    self.stage = Stage::Failed(problem.to_string())
                }
                (Message::Finished(Ok(())), _) => {}
            }
        }
    }

    /// One frame: what has happened since the last, drawn.
    fn frame(&mut self, ctx: &egui::Context) {
        self.take_messages();

        // What to do once the frame is drawn, decided while drawing it: starting a thread in the middle
        // of the closure that is holding the interface would need the borrow checker's permission.
        let mut start: Option<Plan> = None;
        let mut changed: Option<(usize, Wanted)> = None;
        let mut retry = false;
        let mut close = false;

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.set_max_width(400.0);
                        ui.add_space(18.0);
                        ui.add(
                            egui::Image::new(egui::load::SizedTexture::from_handle(&self.mark))
                                .fit_to_exact_size(egui::vec2(72.0, 72.0)),
                        );
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("Noctorium").size(26.0).color(TEXT));
                        ui.add_space(14.0);

                        match &self.stage {
                            Stage::Looking => looking(ui),
                            Stage::Ready(choosing) => ready(ui, choosing, &mut start, &mut changed),
                            Stage::Working(working) if working.finished => {
                                ended(ui, working, &mut retry, &mut close)
                            }
                            Stage::Working(working) => progress(ui, working),
                            Stage::Failed(why) => failed(ui, why, &mut retry, &mut close),
                        }
                        ui.add_space(12.0);
                    });
                });
        });

        if let (Some((chosen, wanted)), Stage::Ready(choosing)) = (changed, &mut self.stage) {
            choosing.chosen = chosen;
            choosing.wanted = wanted;
            self.wanted = wanted;
            choosing.replan();
        }
        if let Some(plan) = start {
            self.begin(ctx, plan);
        }
        if retry {
            // After some of it worked, trying again is for the rest: what is installed already is not
            // ticked again, to be downloaded and installed a second time.
            if let Stage::Working(working) = &self.stage {
                let failed = |product: Product| {
                    working
                        .plan
                        .items
                        .iter()
                        .zip(&working.rows)
                        .any(|(item, row)| item.product == product && matches!(row, Row::Failed(_)))
                };
                self.wanted.desktop = failed(Product::Desktop);
                self.wanted.cli = failed(Product::Cli);
                self.wanted.stats = failed(Product::Stats);
            }
            self.look(ctx);
        }
        if close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for Gui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
    }
}

fn small(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).size(11.0).color(SUBTEXT)
}

fn megabytes(bytes: u64) -> String {
    format!("{:.0} MB", bytes as f64 / 1_048_576.0)
}

fn accent_button(text: &str, size: f32, min: egui::Vec2) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text).size(size).color(ON_ACCENT))
        .fill(ACCENT)
        .min_size(min)
}

fn looking(ui: &mut egui::Ui) {
    ui.add(egui::Spinner::new().size(22.0).color(ACCENT));
    ui.add_space(10.0);
    ui.label(egui::RichText::new("Asking GitHub for the latest release").color(SUBTEXT));
}

/// The choice: which products, on Linux which format, on Windows whether to see the setup's wizard --
/// and the button.
fn ready(
    ui: &mut egui::Ui,
    choosing: &Choosing,
    start: &mut Option<Plan>,
    changed: &mut Option<(usize, Wanted)>,
) {
    let mut wanted = choosing.wanted;
    let mut chosen = choosing.chosen;

    ui.label(
        egui::RichText::new(format!("Version {}", choosing.found.version()))
            .size(17.0)
            .color(TEXT),
    );
    ui.add_space(4.0);
    let summary = match &choosing.plan {
        Ok(plan) if plan.megabytes_to_download() < 0.5 => "Downloaded already".to_string(),
        Ok(plan) if plan.items.len() > 1 => format!(
            "{:.0} MB to download, {} at once",
            plan.megabytes_to_download(),
            if plan.items.len() == 2 {
                "both"
            } else {
                "all three"
            }
        ),
        Ok(plan) => format!("{:.0} MB to download", plan.megabytes_to_download()),
        Err(_) if wanted.products().is_none() => "Nothing chosen yet".to_string(),
        Err(_) => {
            let desktop = choosing.desktop_size().filter(|_| wanted.desktop);
            let cli = choosing.cli_size.filter(|_| wanted.cli);
            let stats = choosing.stats_size.filter(|_| wanted.stats);
            format!(
                "{} to download",
                megabytes(desktop.unwrap_or(0) + cli.unwrap_or(0) + stats.unwrap_or(0))
            )
        }
    };
    ui.label(egui::RichText::new(summary).size(13.0).color(SUBTEXT));
    ui.add_space(12.0);

    // A box for each, rather than a list of every combination: they are separate programs, and which of
    // them is wanted is three questions of yes or no. Each says what it is and how big, in the label
    // itself, so a screen reader says it all with the box.
    // What a product this release does not carry says instead of its size, in the label itself rather
    // than only when the pointer is over it: a box that cannot be ticked and does not say why looks
    // broken. Noctorium Stats is the one this is for, against a release from before it.
    let tag = &choosing.found.release.tag;
    let missing = format!("Not in {tag} yet.");
    let or_missing = |what: &str, size: Option<u64>| match size {
        Some(_) => what.to_string(),
        // In place of what it is, which a box that cannot be ticked has less need to say.
        None => format!("not in {tag} yet"),
    };
    ui.allocate_ui_with_layout(
        egui::vec2(330.0, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            let desktop = product_label(
                ui,
                "Noctorium",
                "the music player, in a window",
                choosing.desktop_size(),
            );
            ui.checkbox(&mut wanted.desktop, desktop);
            ui.add_space(4.0);
            let cli = product_label(
                ui,
                "Noctorium CLI",
                &or_missing("the same player, in a terminal", choosing.cli_size),
                choosing.cli_size,
            );
            ui.add_enabled(
                choosing.cli_size.is_some(),
                egui::Checkbox::new(&mut wanted.cli, cli),
            )
            .on_disabled_hover_text(missing.as_str());
            ui.add_space(4.0);
            let stats = product_label(
                ui,
                "Noctorium Stats",
                &or_missing("your listening, in figures", choosing.stats_size),
                choosing.stats_size,
            );
            ui.add_enabled(
                choosing.stats_size.is_some(),
                egui::Checkbox::new(&mut wanted.stats, stats),
            )
            .on_disabled_hover_text(missing.as_str());
        },
    );

    // Only where there is a choice to make, which is Linux, and only for Noctorium itself. The ones this
    // release does not carry, or this machine cannot use, are listed but cannot be picked, so nobody is
    // left wondering where the Flatpak went.
    if wanted.desktop && choosing.offers.len() > 1 {
        ui.add_space(10.0);
        let current = choosing
            .offers
            .get(choosing.chosen)
            .map(|o| o.method.describe())
            .unwrap_or("");
        // In a box of its own width, so that it sits in the middle like everything else rather than
        // at the left of the whole column.
        ui.allocate_ui_with_layout(
            egui::vec2(260.0, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                egui::ComboBox::from_id_salt("format")
                    .width(260.0)
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (index, offer) in choosing.offers.iter().enumerate() {
                            let label = match &offer.unavailable {
                                None if offer.recommended => {
                                    format!("{}  (recommended)", offer.method.describe())
                                }
                                None => offer.method.describe().to_string(),
                                Some(_) => {
                                    format!("{}  (not available)", offer.method.describe())
                                }
                            };
                            let entry = egui::Button::selectable(index == choosing.chosen, label);
                            let entry = ui
                                .add_enabled(offer.unavailable.is_none(), entry)
                                .on_disabled_hover_text(
                                    offer.unavailable.clone().unwrap_or_default(),
                                );
                            if entry.clicked() {
                                chosen = index;
                            }
                        }
                    });
            },
        );
        if let Some(offer) = choosing.offers.get(choosing.chosen) {
            ui.add_space(2.0);
            ui.label(small(offer.method.explain()));
        }
    }

    ui.add_space(16.0);
    let ready = choosing.plan.as_ref().ok();
    let label = match wanted.products() {
        Some(products) if products.count() == 3 => "Install all three",
        Some(products) if products.count() == 2 => "Install both",
        Some(Products::CLI) => "Install Noctorium CLI",
        Some(Products::STATS) => "Install Noctorium Stats",
        Some(_) => "Install Noctorium",
        None => "Install",
    };
    let button = accent_button(label, 16.0, egui::vec2(230.0, 40.0));
    if ui.add_enabled(ready.is_some(), button).clicked() {
        *start = ready.cloned();
    }

    // Out of the way, for the few who want it: the same install, with the setup's own pages, where the
    // folder can be chosen. Everybody else gets the folder Noctorium is already in, or Program Files.
    if choosing.wizard_possible() {
        ui.add_space(8.0);
        ui.checkbox(
            &mut wanted.wizard,
            small("Choose the folder myself, in the Noctorium setup"),
        )
        .on_hover_text(
            "Opens the setup's own pages, as the setup .exe does, instead of installing with a \
             progress bar and nothing to click.",
        );
    }

    ui.add_space(10.0);
    let Some(plan) = ready else {
        if let Err(why) = &choosing.plan {
            ui.label(small(why.as_str()));
        }
        if (wanted, chosen) != (choosing.wanted, choosing.chosen) {
            *changed = Some((chosen, wanted));
        }
        return;
    };
    for note in &plan.notes {
        ui.label(small(note.as_str()));
        ui.add_space(2.0);
    }
    ui.label(small(
        "Everything is checked against the checksum published with the release before anything \
         is run.",
    ));
    if plan.needs_password() {
        ui.add_space(4.0);
        ui.label(small("Your password will be asked for at the end."));
    }
    if (wanted, chosen) != (choosing.wanted, choosing.chosen) {
        *changed = Some((chosen, wanted));
    }
}

/// "Noctorium  the music player, in a window, 327 MB", in two sizes.
fn product_label(
    ui: &egui::Ui,
    name: &str,
    what: &str,
    size: Option<u64>,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let style = ui.style();
    egui::RichText::new(name).size(15.0).color(TEXT).append_to(
        &mut job,
        style,
        egui::FontSelection::Default,
        egui::Align::Center,
    );
    let detail = match size {
        Some(size) => format!("   {what}, {}", megabytes(size)),
        None => format!("   {what}"),
    };
    egui::RichText::new(detail)
        .size(12.0)
        .color(SUBTEXT)
        .append_to(
            &mut job,
            style,
            egui::FontSelection::Default,
            egui::Align::Center,
        );
    job
}

/// A row for each product while the work goes on: its bar, and what it is doing.
fn progress(ui: &mut egui::Ui, working: &Working) {
    let heading = match working.installing() {
        Some(item) => format!("Installing {}", working.plan.items[item].product.name()),
        None => "Downloading".to_string(),
    };
    ui.label(egui::RichText::new(heading).size(15.0).color(TEXT));
    ui.add_space(12.0);
    rows(ui, working);

    if let Some(item) = working.installing() {
        ui.add_space(10.0);
        ui.add(egui::Spinner::new().size(18.0).color(ACCENT));
        ui.add_space(6.0);
        ui.label(small(
            "The download matched the checksum published with it.",
        ));
        let item = &working.plan.items[item];
        let what = if item.escalation.is_some() {
            "Your desktop will ask for your password."
        } else {
            match item.actions.first() {
                Some(Action::InstallMsi { wizard: false, .. }) => {
                    "Windows asks for permission, and then shows its own progress bar."
                }
                Some(Action::InstallMsi { wizard: true, .. }) => {
                    "The Noctorium setup takes over from here."
                }
                Some(Action::Run { .. }) if item.method == Method::WindowsSetup => {
                    "The Noctorium setup takes over from here."
                }
                Some(Action::UnpackCli { .. } | Action::UnpackStats { .. }) => {
                    "Unpacking it into a folder of its own."
                }
                _ => "",
            }
        };
        if !what.is_empty() {
            ui.add_space(4.0);
            ui.label(small(what));
        }
    }
}

fn rows(ui: &mut egui::Ui, working: &Working) {
    for (item, row) in working.plan.items.iter().zip(&working.rows) {
        let (fraction, said) = match row {
            Row::Downloading { done, total } => (
                if *total > 0 {
                    (*done as f32 / *total as f32).min(1.0)
                } else {
                    0.0
                },
                format!("{} of {}", megabytes(*done), megabytes(*total)),
            ),
            Row::Checked(Fetched::AlreadyHere) => {
                (1.0, "Downloaded already, and checked".to_string())
            }
            Row::Checked(_) => (1.0, "Downloaded and checked".to_string()),
            Row::Installing => (1.0, "Installing".to_string()),
            Row::Installed(_) => (1.0, "Installed".to_string()),
            Row::Failed(_) => (0.0, "Not installed".to_string()),
        };
        ui.allocate_ui_with_layout(
            egui::vec2(320.0, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(item.product.name())
                            .size(13.0)
                            .color(TEXT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(said).size(12.0).color(SUBTEXT));
                    });
                });
                // Under the bar rather than inside it, the words: a progress bar draws its text from its
                // left edge across the whole width, not within the part that is filled, so text chosen
                // to read against the accent turns invisible the moment it runs off the end of it.
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(320.0)
                        .fill(ACCENT),
                );
            },
        );
        ui.add_space(10.0);
    }
}

/// How it went, product by product, once nothing more is happening.
fn ended(ui: &mut egui::Ui, working: &Working, retry: &mut bool, close: &mut bool) {
    let installed: Vec<usize> = (0..working.rows.len())
        .filter(|i| matches!(working.rows[*i], Row::Installed(_)))
        .collect();
    let title = match (installed.len(), working.rows.len()) {
        (1, 1) => format!("{} is installed.", working.plan.items[0].product.name()),
        (2, 2) => "Both are installed.".to_string(),
        (all, count) if all == count => "All three are installed.".to_string(),
        (0, _) => "That did not work.".to_string(),
        _ => "That did not all work.".to_string(),
    };
    ui.label(egui::RichText::new(title).size(17.0).color(TEXT));
    ui.add_space(12.0);

    egui::ScrollArea::vertical()
        .id_salt("outcome")
        .max_height(170.0)
        .show(ui, |ui| {
            for (item, row) in working.plan.items.iter().zip(&working.rows) {
                let name = item.product.name();
                match row {
                    Row::Installed(note) => {
                        if working.rows.len() > 1 {
                            ui.label(
                                egui::RichText::new(format!("{name} is installed."))
                                    .size(13.0)
                                    .color(TEXT),
                            );
                        }
                        ui.label(
                            egui::RichText::new(item.start.as_str())
                                .size(12.0)
                                .color(SUBTEXT),
                        );
                        if let Some(note) = note {
                            ui.label(small(note.as_str()));
                        }
                    }
                    Row::Failed(why) => {
                        ui.label(
                            egui::RichText::new(format!("{name} was not installed."))
                                .size(13.0)
                                .color(TEXT),
                        );
                        ui.label(egui::RichText::new(why.as_str()).size(12.0).color(SUBTEXT));
                    }
                    _ => {}
                }
                ui.add_space(8.0);
            }
        });
    ui.add_space(10.0);

    if working.all_installed() {
        if ui
            .add(accent_button("Close", 15.0, egui::vec2(150.0, 36.0)))
            .clicked()
        {
            *close = true;
        }
        return;
    }
    // What was installed stays installed, and what was downloaded stays downloaded: trying again does
    // the rest without fetching it twice.
    if ui
        .add(accent_button("Try again", 15.0, egui::vec2(150.0, 36.0)))
        .clicked()
    {
        *retry = true;
    }
    ui.add_space(8.0);
    let button = egui::Button::new(egui::RichText::new("Close").size(13.0).color(SUBTEXT))
        .min_size(egui::vec2(150.0, 30.0));
    if ui.add(button).clicked() {
        *close = true;
    }
}

fn failed(ui: &mut egui::Ui, why: &str, retry: &mut bool, close: &mut bool) {
    ui.label(
        egui::RichText::new("That did not work.")
            .size(17.0)
            .color(TEXT),
    );
    ui.add_space(10.0);
    egui::ScrollArea::vertical()
        .id_salt("failure")
        .max_height(110.0)
        .show(ui, |ui| {
            ui.label(egui::RichText::new(why).size(12.0).color(SUBTEXT));
        });
    ui.add_space(14.0);
    // Most of what goes wrong here is a network that was briefly not there, so the obvious answer is to
    // ask again rather than to start the program again.
    if ui
        .add(accent_button("Try again", 15.0, egui::vec2(150.0, 36.0)))
        .clicked()
    {
        *retry = true;
    }
    ui.add_space(8.0);
    let button = egui::Button::new(egui::RichText::new("Close").size(13.0).color(SUBTEXT))
        .min_size(egui::vec2(150.0, 30.0));
    if ui.add(button).clicked() {
        *close = true;
    }
}
