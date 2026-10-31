//! The window before the browser: the profile opens on a thread of its own, the window says
//! so when the keychain is slow, and why when it cannot open, with a way to try again.

use crate::app::BeaconApp;
use beacon_core::startup::StartupError;
use eframe::egui_wgpu::RenderState;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;
use tokio::runtime::Runtime;

pub struct Startup {
    rt: &'static Runtime,
    urls: Vec<String>,
    render_state: Option<RenderState>,
    state: State,
}

enum State {
    Opening {
        rx: Receiver<anyhow::Result<()>>,
        since: Instant,
    },
    /// `retry` is false for a failure before the lockdown, which a second try in this
    /// process must not get past.
    Failed {
        error: StartupError,
        retry: bool,
    },
    Running(Box<BeaconApp>),
}

impl Startup {
    /// `opened` is how opening the profile folder went, before the window: a failure there
    /// is shown, with no retry.
    pub fn new(cc: &eframe::CreationContext<'_>, rt: &'static Runtime, urls: Vec<String>, opened: anyhow::Result<()>) -> Self {
        // Here rather than in the browser, so the startup screens get it too.
        #[cfg(target_os = "android")]
        crate::android::touch_style(&cc.egui_ctx);
        let state = match opened {
            Ok(()) => opening(),
            Err(e) => failed(&e, false),
        };
        Self {
            rt,
            urls,
            render_state: cc.wgpu_render_state.clone(),
            state,
        }
    }

    /// The profile is open: build the browser, or say why it would not start.
    fn run(&mut self, ctx: &egui::Context) -> State {
        // A clone: after a failure, Try Again still opens the tabs asked for.
        match BeaconApp::new(ctx, self.render_state.as_ref(), self.rt, self.urls.clone()) {
            Ok(app) => State::Running(Box::new(app)),
            Err(e) => failed(&e, true),
        }
    }
}

fn opening() -> State {
    State::Opening {
        rx: beacon_core::startup::open_profile(),
        since: Instant::now(),
    }
}

fn failed(e: &anyhow::Error, retry: bool) -> State {
    log::error!("Beacon could not start: {e:#}");
    State::Failed {
        error: StartupError::from_error(e),
        retry,
    }
}

impl eframe::App for Startup {
    fn on_exit(&mut self) {
        if let State::Running(app) = &mut self.state {
            app.on_exit();
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let State::Running(app) = &mut self.state {
            app.raw_input_hook(ctx, raw_input);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let next = match &mut self.state {
            State::Running(app) => return app.ui(ui, frame),
            State::Opening { rx, since } => match rx.try_recv() {
                Err(TryRecvError::Empty) => {
                    if since.elapsed() >= beacon_core::startup::PATIENCE {
                        waiting(ui);
                    }
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    return;
                }
                Ok(Ok(())) => self.run(&ctx),
                Ok(Err(e)) => failed(&e, true),
                Err(TryRecvError::Disconnected) => failed(&anyhow::anyhow!("the profile thread ended without an answer"), true),
            },
            State::Failed { error, retry } => match failure(ui, error, *retry) {
                Some(Answer::Retry) => opening(),
                Some(Answer::Quit) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                None => return,
            },
        };
        self.state = next;
        ctx.request_repaint();
    }
}

enum Answer {
    Retry,
    Quit,
}

fn page(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::CentralPanel::default().show(ui, |ui| {
        // Clear of the status bar on a phone, where the window runs under it.
        #[cfg(target_os = "android")]
        if let Some((_, top, _, _)) = crate::android::content_rect() {
            ui.add_space(top as f32 / ui.ctx().pixels_per_point());
        }
        ui.add_space(24.0);
        ui.vertical(add);
    });
}

fn waiting(ui: &mut egui::Ui) {
    page(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.strong("Waiting for the keychain...");
        });
        ui.add_space(8.0);
        ui.label(
            "Beacon keeps each profile's key in the keychain. A locked keyring waits for its password; \
             a keychain service that does not answer gives up after two minutes.",
        );
    });
}

fn failure(ui: &mut egui::Ui, error: &StartupError, retry: bool) -> Option<Answer> {
    let mut answer = None;
    page(ui, |ui| {
        ui.strong(&error.reason);
        ui.add_space(8.0);
        if let Some(hint) = &error.hint {
            // Selectable: a hint may be a command to copy into a terminal.
            ui.add(egui::Label::new(hint.as_str()).selectable(true));
            ui.add_space(8.0);
        }
        // With no hint, the detail is all there is to go on.
        egui::CollapsingHeader::new("Details")
            .default_open(error.hint.is_none())
            .show(ui, |ui| {
                ui.add(egui::Label::new(error.detail.as_str()).selectable(true));
            });
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            if retry && ui.button("Try Again").clicked() {
                answer = Some(Answer::Retry);
            }
            if ui.button("Quit").clicked() {
                answer = Some(Answer::Quit);
            }
        });
    });
    answer
}
