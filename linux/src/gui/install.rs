// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Installing RealVNC's libraries on Connect, before the session starts
//! (`realvnc::install`): the connecting screen, showing each step of the
//! installation as the add-on's screen does, with Cancel; and where
//! `AutoFetchLibraries` is `prompt`, the add-on's screen itself as a panel
//! over it, styled as the server's signature prompt, which asks first
//! (`prompts::library_panel`). It is a screen of its own rather than a
//! session's, since the RealVNC backend cannot be created until the
//! libraries are there.

use super::csd;
use super::dialog::{BUTTON, BUTTON_INSET};
use super::prompts::{self, LibraryPanel};
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
    source: Source,
    shared: Arc<Shared>,
    /// The panel, where the installation waits to be asked for.
    panel: Option<Panel>,
    /// Whether the installation's thread runs.
    working: bool,
    /// Why the last attempt failed, shown in the panel.
    failure: Option<String>,
}

/// The panel's fixed text.
struct Panel {
    other_build: Option<String>,
    explanation: String,
    pins: String,
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
        let mut installing = Self::new(request, dir, source, why, wake, None);
        installing.run();
        installing
    }

    /// The panel over the connecting screen, which installs from `source`
    /// once asked to.
    pub fn ask(request: Request, dir: PathBuf, source: Source, why: String, wake: Box<dyn Fn() + Send + Sync>) -> Self {
        let panel = Panel {
            other_build: None,
            explanation: realvnc::explanation(&dir),
            pins: realvnc::pins(),
        };
        Self::new(request, dir, source, why, wake, Some(panel))
    }

    /// With the first line saying the libraries in `dir`, where the session
    /// looks for them, are another build.
    pub fn other_build(mut self, dir: &std::path::Path) -> Self {
        if let Some(panel) = &mut self.panel {
            panel.other_build = Some(realvnc::other_build(dir));
        }
        self
    }

    fn new(
        request: Request,
        dir: PathBuf,
        source: Source,
        why: String,
        wake: Box<dyn Fn() + Send + Sync>,
        panel: Option<Panel>,
    ) -> Self {
        let shared = Arc::new(Shared {
            cancel: AtomicBool::new(false),
            state: Mutex::new(State::default()),
            wake,
        });
        Installing {
            request,
            dir,
            why,
            source,
            shared,
            panel,
            working: false,
            failure: None,
        }
    }

    /// Starts the installation's thread.
    fn run(&mut self) {
        *self.shared.state.lock().unwrap() = State::default();
        self.working = true;
        self.failure = None;
        let worker = Arc::clone(&self.shared);
        let (source, to) = (self.source.clone(), self.dir.clone());
        let spawned = std::thread::Builder::new().name("realvnc-install".into()).spawn(move || {
            let result = realvnc::install(&source, &to, &*worker);
            worker.state.lock().unwrap().result = Some(result);
            (worker.wake)();
        });
        if let Err(e) = spawned {
            log::warn!("could not start a thread to install RealVNC's library on: {e}");
            self.shared.state.lock().unwrap().result = Some(Err(realvnc::Error::Failed("That did not work.".into())));
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
            // The panel shows why, and offers it again, as the add-on does.
            Some(Err(realvnc::Error::Failed(why))) if self.panel.is_some() => {
                self.working = false;
                self.failure = Some(why);
            }
            Some(Err(realvnc::Error::Failed(why))) => return Some(Outcome::Back(format!("Could not connect: {why}"))),
            Some(Err(realvnc::Error::Cancelled)) => return Some(self.cancelled()),
            None => {}
        }
        // The panel has the status; the screen under it only the address.
        let (status, progress) = if self.panel.is_some() { (None, None) } else { (status, progress) };
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
        if let Some(panel) = &self.panel {
            let (status, progress) = {
                let state = self.shared.state.lock().unwrap();
                (state.status.clone(), state.progress)
            };
            let shown = LibraryPanel {
                address: &self.request.address,
                other_build: panel.other_build.as_deref(),
                explanation: &panel.explanation,
                pins: &panel.pins,
                status: if self.working { status.as_deref() } else { self.failure.as_deref() },
                failed: !self.working && self.failure.is_some(),
                progress: progress.filter(|_| self.working),
                working: self.working,
            };
            match prompts::library_panel(&ctx, &shown) {
                Some(true) => self.run(),
                Some(false) => cancel = true,
                None => {}
            }
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
