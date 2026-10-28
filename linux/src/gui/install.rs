// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Installing RealVNC's libraries on Connect, before the session starts
//! (`realvnc::install`): the connecting screen, showing each step of the
//! installation as the add-on's screen does, with Cancel. It is a screen of
//! its own rather than a session's, since the RealVNC backend cannot be
//! created until the libraries are there.

use super::csd;
use super::dialog::{BUTTON, BUTTON_INSET};
use super::session::waiting;
use super::{BACKDROP, Host, Request};
use crate::realvnc::{self, Report, Source, Step};
use egui::{Frame, Id, Order, PointerButton, Sense, Ui};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How an installation ended, for the window to act on.
pub enum Outcome {
    /// The libraries are in [`Installing::dir`]: the session can start.
    Installed,
    /// Back to the dialog, with this as its notice.
    Back(String),
}

pub struct Installing {
    pub request: Request,
    /// Where the libraries are being installed to.
    pub dir: PathBuf,
    /// Why they are needed: the reason a session would fail with, which the
    /// dialog is given if the installation is cancelled.
    why: String,
    shared: Arc<Shared>,
}

/// What the installation's thread and the screen share.
struct Shared {
    cancel: AtomicBool,
    state: Mutex<State>,
    wake: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct State {
    status: Option<String>,
    progress: Option<(u64, Option<u64>)>,
    result: Option<Result<(), realvnc::Error>>,
}

impl Installing {
    /// Starts installing from `source` into `dir` on a thread of its own;
    /// `wake` asks for a frame when there is something new to show.
    pub fn start(request: Request, dir: PathBuf, source: Source, why: String, wake: Box<dyn Fn() + Send + Sync>) -> Self {
        let shared = Arc::new(Shared {
            cancel: AtomicBool::new(false),
            state: Mutex::new(State::default()),
            wake,
        });
        let worker = Arc::clone(&shared);
        let to = dir.clone();
        let spawned = std::thread::Builder::new().name("realvnc-install".into()).spawn(move || {
            let result = realvnc::install(&source, &to, &*worker);
            worker.state.lock().unwrap().result = Some(result);
            (worker.wake)();
        });
        if let Err(e) = spawned {
            log::warn!("could not start a thread to install RealVNC's library on: {e}");
            shared.state.lock().unwrap().result = Some(Err(realvnc::Error::Failed("That did not work.".into())));
        }
        Installing {
            request,
            dir,
            why,
            shared,
        }
    }

    /// The screen's frame in the main window, and how the installation
    /// ended, on the frame it did.
    pub fn ui(&mut self, root: &mut Ui, host: &mut Host) -> Option<Outcome> {
        let ctx = root.ctx().clone();
        let (status, progress, result) = {
            let mut state = self.shared.state.lock().unwrap();
            (state.status.clone(), state.progress, state.result.take())
        };
        match result {
            Some(Ok(())) => return Some(Outcome::Installed),
            Some(Err(realvnc::Error::Failed(why))) => return Some(Outcome::Back(format!("Could not connect: {why}"))),
            Some(Err(realvnc::Error::Cancelled)) => return Some(self.cancelled()),
            None => {}
        }
        let mut cancel = false;
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BACKDROP))
            .show(root, |ui| {
                if host.csd {
                    let background = ui.interact(ui.max_rect(), Id::new("installing-background"), Sense::click_and_drag());
                    if background.drag_started_by(PointerButton::Primary) {
                        csd::drag_window(&ctx, host);
                    }
                }
                cancel = waiting(ui, &self.request.address, status.as_deref(), progress, false);
            });
        if host.csd {
            egui::Area::new(Id::new("window-buttons"))
                .order(Order::Foreground)
                .fixed_pos(ctx.content_rect().min)
                .constrain(false)
                .show(&ctx, |ui| csd::dialog_buttons(ui, host, BUTTON, BUTTON_INSET));
            let _ = csd::resize_edges(&ctx, host, csd::DIALOG_EDGE, csd::DIALOG_EDGE);
        }
        cancel.then(|| self.cancelled())
    }

    /// Stops the installation, and goes back to the dialog with the reason
    /// the libraries were needed.
    fn cancelled(&self) -> Outcome {
        self.shared.cancel.store(true, Ordering::Relaxed);
        Outcome::Back(format!("Could not connect: RealVNC is not available: {}", self.why))
    }
}

impl Drop for Installing {
    /// A window closed, or a screen left, stops the installation.
    fn drop(&mut self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }
}

/// How long each step stays on the screen, before and after it is shown, as
/// the add-on's (`SetupActivity.LINGER`): most are quick, and a line shown too
/// briefly cannot be read.
const LINGER: Duration = Duration::from_millis(500);

impl Shared {
    fn linger(&self) {
        let until = std::time::Instant::now() + LINGER;
        while !self.cancelled() && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Report for Shared {
    fn step(&self, step: Step<'_>) {
        self.linger();
        let mut state = self.state.lock().unwrap();
        state.status = Some(step.text());
        state.progress = None;
        drop(state);
        (self.wake)();
        self.linger();
    }

    fn progress(&self, done: u64, total: Option<u64>) {
        let mut state = self.state.lock().unwrap();
        // Every quarter megabyte, as the add-on: a bar cannot show finer
        // steps, and each one is a frame.
        let last = state.progress.map_or(0, |(d, _)| d);
        if state.progress.is_some() && done.saturating_sub(last) < 256 * 1024 && Some(done) != total {
            return;
        }
        state.progress = Some((done, total));
        drop(state);
        (self.wake)();
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}
