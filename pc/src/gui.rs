//! The installer as a window.
//!
//! Immediate mode, which suits this exactly: the whole interface is a function of how far the install has
//! got, and "how far" arrives as messages from the thread doing the work. There is no widget tree to keep
//! in step with a download, and nothing to forget to update.
//!
//! The colours are Noctorium's own Dusk theme, so the thing that installs the player looks like the
//! player. Pure black would be the Night theme and is too hard an edge for a small window.

use crate::flow::{self, Plan, Step};
use crate::github::Problem;
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

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

/// Opens the window and returns whether Noctorium ended up installed.
///
/// The error is only ever "no window could be opened" -- everything that can go wrong with an install is
/// shown in the window itself, where the person who asked for it is looking.
pub fn run() -> Result<bool, String> {
    let installed = Arc::new(AtomicBool::new(false));
    let flag = installed.clone();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([470.0, 350.0])
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
        Box::new(move |cc| Ok(Box::new(Gui::new(cc, flag)))),
    )
    .map_err(|e| e.to_string())?;

    Ok(installed.load(Ordering::Relaxed))
}

/// What the working thread has to say for itself.
enum Message {
    Found(Box<Plan>),
    Progress(Step),
    Finished(Result<(), Problem>),
}

/// Where the install has got to, which is the whole of what the window draws.
enum Stage {
    Looking,
    Ready(Box<Plan>),
    Downloading { done: u64, total: u64 },
    Installing { needs_password: bool },
    Done,
    Failed(String),
}

struct Gui {
    stage: Stage,
    events: Receiver<Message>,
    sender: Sender<Message>,
    mark: egui::TextureHandle,
    /// Taken from the plan when the install starts, because by then the plan itself is on the thread
    /// doing the work. Whether a password is coming depends on the machine, not on the platform: a
    /// Linux session already running as root is asked for nothing.
    needs_password: bool,
    installed: Arc<AtomicBool>,
}

impl Gui {
    fn new(cc: &eframe::CreationContext<'_>, installed: Arc<AtomicBool>) -> Self {
        let ctx = &cc.egui_ctx;

        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = BACKGROUND;
        visuals.window_fill = BACKGROUND;
        // What a progress bar's unfilled half is drawn in.
        visuals.extreme_bg_color = CARD;
        visuals.override_text_color = Some(TEXT);
        visuals.selection.bg_fill = ACCENT;
        ctx.set_visuals(visuals);

        let image = egui::ColorImage::from_rgba_unmultiplied([MARK_SIDE, MARK_SIDE], MARK);
        let mark = ctx.load_texture("noctorium-mark", image, egui::TextureOptions::LINEAR);

        let (sender, events) = channel();
        let mut gui = Self {
            stage: Stage::Looking,
            events,
            sender,
            mark,
            needs_password: false,
            installed,
        };
        gui.look(ctx);
        gui
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
            let message = match flow::discover() {
                Ok(plan) => Message::Found(Box::new(plan)),
                Err(problem) => Message::Finished(Err(problem)),
            };
            let _ = sender.send(message);
            repaint.request_repaint();
        });
    }

    /// Starts the download and the install on a thread of their own.
    ///
    /// On this thread it would freeze the window for the length of a three hundred megabyte download,
    /// which looks exactly like a program that has crashed.
    fn begin(&mut self, ctx: &egui::Context, plan: Plan) {
        self.needs_password = plan.needs_password();
        self.stage = Stage::Downloading {
            done: 0,
            total: plan.asset.size,
        };
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
            match message {
                Message::Found(plan) => self.stage = Stage::Ready(plan),
                Message::Progress(Step::Downloading { done, total }) => {
                    self.stage = Stage::Downloading { done, total }
                }
                // The gap between this and the install starting is a few milliseconds, so it is said as
                // part of what comes next rather than flashed up on its own.
                Message::Progress(Step::Verified) => {}
                Message::Progress(Step::Installing { .. }) => {
                    self.stage = Stage::Installing {
                        needs_password: self.needs_password,
                    }
                }
                Message::Finished(Ok(())) => {
                    self.installed.store(true, Ordering::Relaxed);
                    self.stage = Stage::Done;
                }
                Message::Finished(Err(problem)) => self.stage = Stage::Failed(problem.to_string()),
            }
        }
    }
}

impl eframe::App for Gui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.take_messages();

        // What to do once the frame is drawn, decided while drawing it: starting a thread in the middle
        // of the closure that is holding the interface would need the borrow checker's permission.
        let mut start: Option<Plan> = None;
        let mut retry = false;
        let mut close = false;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.set_max_width(400.0);
                ui.add_space(20.0);
                ui.add(
                    egui::Image::new(egui::load::SizedTexture::from_handle(&self.mark))
                        .fit_to_exact_size(egui::vec2(72.0, 72.0)),
                );
                ui.add_space(10.0);
                ui.label(egui::RichText::new("Noctorium").size(26.0).color(TEXT));
                ui.add_space(18.0);

                match &self.stage {
                    Stage::Looking => {
                        ui.add(egui::Spinner::new().size(22.0).color(ACCENT));
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new("Asking GitHub for the latest release")
                                .color(SUBTEXT),
                        );
                    }

                    Stage::Ready(plan) => {
                        ui.label(
                            egui::RichText::new(format!("Version {}", plan.version()))
                                .size(17.0)
                                .color(TEXT),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(format!("{:.0} MB to download", plan.megabytes()))
                                .size(13.0)
                                .color(SUBTEXT),
                        );
                        ui.add_space(18.0);
                        let button = egui::Button::new(
                            egui::RichText::new("Install Noctorium")
                                .size(16.0)
                                .color(ON_ACCENT),
                        )
                        .fill(ACCENT)
                        .min_size(egui::vec2(230.0, 40.0));
                        if ui.add(button).clicked() {
                            start = Some((**plan).clone());
                        }
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new(
                                "It is checked against the checksum published with the release before anything is run.",
                            )
                            .size(11.0)
                            .color(SUBTEXT),
                        );
                        if plan.needs_password() {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new("Your password will be asked for at the end.")
                                    .size(11.0)
                                    .color(SUBTEXT),
                            );
                        }
                    }

                    Stage::Downloading { done, total } => {
                        let fraction = if *total > 0 {
                            (*done as f32 / *total as f32).min(1.0)
                        } else {
                            0.0
                        };
                        ui.label(egui::RichText::new("Downloading").size(15.0).color(TEXT));
                        ui.add_space(12.0);
                        ui.add(
                            egui::ProgressBar::new(fraction)
                                .desired_width(320.0)
                                .fill(ACCENT)
                                .text(
                                    egui::RichText::new(format!(
                                        "{:.0} MB of {:.0} MB",
                                        *done as f64 / 1_048_576.0,
                                        *total as f64 / 1_048_576.0
                                    ))
                                    .size(12.0)
                                    .color(ON_ACCENT),
                                ),
                        );
                    }

                    Stage::Installing { needs_password } => {
                        ui.add(egui::Spinner::new().size(22.0).color(ACCENT));
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("Installing").size(15.0).color(TEXT));
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(
                                "The download matched the checksum published with it.",
                            )
                            .size(11.0)
                            .color(SUBTEXT),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(if *needs_password {
                                "Your desktop will ask for your password."
                            } else {
                                "The Noctorium setup takes over from here."
                            })
                            .size(11.0)
                            .color(SUBTEXT),
                        );
                    }

                    Stage::Done => {
                        ui.label(
                            egui::RichText::new("Noctorium is installed.")
                                .size(17.0)
                                .color(TEXT),
                        );
                        ui.add_space(18.0);
                        let button =
                            egui::Button::new(egui::RichText::new("Close").size(15.0).color(ON_ACCENT))
                                .fill(ACCENT)
                                .min_size(egui::vec2(150.0, 36.0));
                        if ui.add(button).clicked() {
                            close = true;
                        }
                    }

                    Stage::Failed(why) => {
                        ui.label(egui::RichText::new("That did not work.").size(17.0).color(TEXT));
                        ui.add_space(10.0);
                        egui::ScrollArea::vertical()
                            .max_height(110.0)
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new(why.as_str()).size(12.0).color(SUBTEXT));
                            });
                        ui.add_space(14.0);
                        // Most of what goes wrong here is a network that was briefly not there, so the
                        // obvious answer is to ask again rather than to start the program again.
                        let again = egui::Button::new(
                            egui::RichText::new("Try again").size(15.0).color(ON_ACCENT),
                        )
                        .fill(ACCENT)
                        .min_size(egui::vec2(150.0, 36.0));
                        if ui.add(again).clicked() {
                            retry = true;
                        }
                        ui.add_space(8.0);
                        let button =
                            egui::Button::new(egui::RichText::new("Close").size(13.0).color(SUBTEXT))
                                .min_size(egui::vec2(150.0, 30.0));
                        if ui.add(button).clicked() {
                            close = true;
                        }
                    }
                }
            });
        });

        if let Some(plan) = start {
            self.begin(ctx, plan);
        }
        if retry {
            self.look(ctx);
        }
        if close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
