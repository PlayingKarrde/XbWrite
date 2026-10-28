//! Background threads for scanning inputs, converting games and extracting them.
//!
//! Each file / job runs under `catch_unwind`, so a parser panic on a corrupt image is
//! reported as an error instead of killing the thread and leaving the UI waiting forever.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Instant;

use xbwrite_core::convert::{convert, ConvertOptions, Progress};
use xbwrite_core::extract::{extract, ExtractOptions};
use xbwrite_core::scan::{scan, Game};
use xbwrite_core::source::collect_inputs;
use xbwrite_core::Error;

pub enum ScanMsg {
    Game(PathBuf, Result<Box<Game>, String>),
    Done,
}

pub fn spawn_scan(ctx: eframe::egui::Context, paths: Vec<PathBuf>) -> Receiver<ScanMsg> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for p in collect_inputs(&paths) {
            let r = guarded(|| scan(&p).map(Box::new).map_err(|e| e.to_string()));
            if tx.send(ScanMsg::Game(p, r)).is_err() {
                return;
            }
            ctx.request_repaint();
        }
        let _ = tx.send(ScanMsg::Done);
        ctx.request_repaint();
    });
    rx
}

pub enum Task {
    Convert { output: PathBuf, opts: ConvertOptions },
    Extract { output: PathBuf, opts: ExtractOptions },
}

pub struct Job {
    /// The game's id in the UI.
    pub id: u64,
    pub game: Game,
    pub task: Task,
}

pub enum JobMsg {
    Started(u64),
    Progress(u64, Progress),
    Finished(u64, Result<Vec<PathBuf>, String>),
    Cancelled(u64),
    AllDone,
}

/// Runs the jobs one after another. Cancelling stops the current job and skips the rest.
pub fn spawn_jobs(ctx: eframe::egui::Context, jobs: Vec<Job>, cancel: Arc<AtomicBool>) -> Receiver<JobMsg> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for job in jobs {
            let id = job.id;
            let _ = tx.send(JobMsg::Started(id));
            ctx.request_repaint();
            let mut last = Instant::now();
            let tx_progress = tx.clone();
            let ctx_progress = ctx.clone();
            let mut report = move |p: Progress| {
                // Byte counts are throttled; the first and last of each phase always go through.
                let throttle = matches!(p, Progress::Bytes { done, total } if done != total && done != 0);
                if throttle && last.elapsed().as_millis() < 100 {
                    return;
                }
                last = Instant::now();
                let _ = tx_progress.send(JobMsg::Progress(id, p));
                ctx_progress.request_repaint();
            };
            let result = guarded(|| match &job.task {
                Task::Convert { output, opts } => convert(&job.game, output, opts, &cancel, &mut report),
                Task::Extract { output, opts } => {
                    extract(&job.game, output, opts, &cancel, &mut report).map(|p| vec![p])
                }
            });
            let msg = match result {
                Ok(written) => JobMsg::Finished(id, Ok(written)),
                Err(Error::Cancelled) => JobMsg::Cancelled(id),
                Err(e) => JobMsg::Finished(id, Err(e.to_string())),
            };
            let cancelled = matches!(msg, JobMsg::Cancelled(_));
            let _ = tx.send(msg);
            ctx.request_repaint();
            if cancelled {
                break;
            }
        }
        let _ = tx.send(JobMsg::AllDone);
        ctx.request_repaint();
    });
    rx
}

/// Runs `f`, turning a panic into an error value.
fn guarded<T, E: FromPanic>(f: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            Err(E::from_panic(msg))
        }
    }
}

trait FromPanic {
    fn from_panic(msg: String) -> Self;
}

impl FromPanic for String {
    fn from_panic(msg: String) -> Self {
        format!("internal error while reading this image: {msg}")
    }
}

impl FromPanic for Error {
    fn from_panic(msg: String) -> Self {
        Error::Unsupported(format!("internal error: {msg}"))
    }
}
