use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, Color32, RichText};
use xbwrite_core::convert::{output_path, ConvertOptions, Format, Phase, Progress};
use xbwrite_core::extract::ExtractOptions;
use xbwrite_core::scan::{Game, SourceKind};
use xbwrite_core::source::{is_game_folder, Layout, EXTENSIONS};
use xbwrite_core::util::{format_size, percent};

use crate::theme::{self, ThemeChoice, ThemeStyle};
use crate::worker::{spawn_jobs, spawn_scan, Job, JobMsg, ScanMsg, Task};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Settings {
    /// `None` = write next to the source.
    output_dir: Option<PathBuf>,
    /// Write CSO instead of XISO.
    cso: bool,
    split_fat32: bool,
    strip_system_update: bool,
    verify: bool,
    /// Delete the source image once the new one is written.
    replace_source: bool,
    last_dir: Option<PathBuf>,
    theme: ThemeChoice,
    style: ThemeStyle,
    /// Last folder a game was extracted into.
    extract_dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_dir: None,
            cso: false,
            split_fat32: false,
            strip_system_update: true,
            verify: true,
            replace_source: false,
            last_dir: None,
            theme: ThemeChoice::Dark,
            style: ThemeStyle::Duke,
            extract_dir: None,
        }
    }
}

impl Settings {
    fn format(&self) -> Format {
        if self.cso {
            Format::Cso
        } else {
            Format::Xiso
        }
    }

    fn convert_options(&self) -> ConvertOptions {
        ConvertOptions {
            format: self.format(),
            split: self.split_fat32,
            strip_system_update: self.strip_system_update,
            verify: self.verify,
            delete_source: self.replace_source,
        }
    }
}

/// What a running job does with a game.
#[derive(Clone, Copy, PartialEq)]
enum JobKind {
    Convert,
    Extract,
}

impl JobKind {
    fn verb(self) -> &'static str {
        match self {
            Self::Convert => "Converting",
            Self::Extract => "Extracting",
        }
    }

    fn done(self) -> &'static str {
        match self {
            Self::Convert => "Converted",
            Self::Extract => "Extracted",
        }
    }

    fn first_phase(self) -> Phase {
        match self {
            Self::Convert => Phase::Writing,
            Self::Extract => Phase::Extracting,
        }
    }
}

#[derive(Clone, PartialEq)]
enum Status {
    Ready,
    Queued,
    /// `done` / `total` cover every phase of the job (writing, then verifying).
    Working {
        phase: Phase,
        done: u64,
        total: u64,
    },
    Done(JobKind, Vec<PathBuf>),
    Failed(String),
}

struct GameView {
    id: u64,
    game: Game,
    selected: bool,
    status: Status,
    icon: Option<egui::TextureHandle>,
}

impl GameView {
    fn working(&self) -> bool {
        matches!(self.status, Status::Working { .. } | Status::Queued)
    }
}

struct RunningJob {
    kind: JobKind,
    rx: Receiver<JobMsg>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    current: Option<u64>,
    /// Expected bytes of work per game, across all its phases.
    weights: HashMap<u64, u64>,
    /// Work of the finished games.
    bytes_before: u64,
    bytes_total: u64,
    /// Work of the current game's earlier phases, and the size of its current phase.
    phase_base: u64,
    phase_total: u64,
}

/// The "Extract to folder" dialog.
struct ExtractDialog {
    id: u64,
    /// Folder the game folder is created in.
    dir: PathBuf,
}

/// Developer aids driven by environment variables (used to screenshot the UI headlessly).
#[derive(Default)]
struct DebugHooks {
    /// `XBWRITE_SCREENSHOT=out.png`: save a screenshot after ~2 s and exit.
    screenshot: Option<PathBuf>,
    /// `XBWRITE_DEBUG_CONVERT`: start converting the selected games shortly after launch.
    auto_convert: bool,
    /// `XBWRITE_DEBUG_SETTINGS`: open the settings dialog on launch.
    open_settings: bool,
    // `XBWRITE_DEBUG_THEME=light` is applied in `App::new` without touching saved settings.
}

impl DebugHooks {
    fn from_env() -> Self {
        Self {
            screenshot: std::env::var_os("XBWRITE_SCREENSHOT").map(PathBuf::from),
            auto_convert: std::env::var_os("XBWRITE_DEBUG_CONVERT").is_some(),
            open_settings: std::env::var_os("XBWRITE_DEBUG_SETTINGS").is_some(),
        }
    }
}

pub struct App {
    settings: Settings,
    inputs: Vec<PathBuf>,
    games: Vec<GameView>,
    next_id: u64,
    scan_errors: Vec<(PathBuf, String)>,
    scan_rx: Option<Receiver<ScanMsg>>,
    /// Paths added while a scan was running; scanned when it ends.
    pending: Vec<PathBuf>,
    job: Option<RunningJob>,
    /// Messages, and whether each is an error.
    log: Vec<(String, bool)>,
    show_settings: bool,
    extract_dialog: Option<ExtractDialog>,
    show_log: bool,
    logo: Option<(Color32, egui::TextureHandle)>,
    debug: DebugHooks,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings: Settings = cc.storage.and_then(|s| eframe::get_value(s, eframe::APP_KEY)).unwrap_or_default();
        theme::install(&cc.egui_ctx, settings.style, settings.theme);
        if std::env::var("XBWRITE_DEBUG_THEME").as_deref() == Ok("light") {
            cc.egui_ctx.set_theme(egui::ThemePreference::Light);
        }
        let debug = DebugHooks::from_env();
        let mut app = Self {
            settings,
            inputs: Vec::new(),
            games: Vec::new(),
            next_id: 1,
            scan_errors: Vec::new(),
            scan_rx: None,
            pending: Vec::new(),
            job: None,
            log: Vec::new(),
            show_settings: debug.open_settings,
            extract_dialog: None,
            show_log: false,
            logo: None,
            debug,
        };
        // Files passed on the command line / dropped onto the exe.
        let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
        if !args.is_empty() {
            app.add_paths(&cc.egui_ctx, args);
        }
        app
    }

    fn log(&mut self, msg: impl Into<String>, error: bool) {
        self.log.push((msg.into(), error));
        if error {
            self.show_log = true;
        }
    }

    fn busy(&self) -> bool {
        self.scan_rx.is_some() || self.job.is_some()
    }

    fn add_paths(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        let new: Vec<PathBuf> = paths
            .into_iter()
            .map(|p| std::path::absolute(&p).unwrap_or(p))
            .filter(|p| !self.inputs.contains(p))
            .collect();
        if new.is_empty() {
            return;
        }
        self.inputs.extend(new.iter().cloned());
        if self.scan_rx.is_none() {
            self.scan_rx = Some(spawn_scan(ctx.clone(), new));
        } else {
            self.pending.extend(new);
        }
    }

    fn clear(&mut self) {
        self.inputs.clear();
        self.games.clear();
        self.scan_errors.clear();
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.scan_rx {
            let mut msgs = Vec::new();
            let mut done = false;
            loop {
                match rx.try_recv() {
                    Ok(ScanMsg::Game(p, r)) => msgs.push((p, r)),
                    Ok(ScanMsg::Done) | Err(TryRecvError::Disconnected) => {
                        done = true;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
            for (p, r) in msgs {
                match r {
                    Ok(game) => self.add_game(*game),
                    Err(e) => {
                        self.log(format!("✖ {}: {e}", file_name(&p)), true);
                        self.scan_errors.push((p, e));
                    }
                }
            }
            if done {
                self.scan_rx = None;
                if !self.pending.is_empty() {
                    self.scan_rx = Some(spawn_scan(ctx.clone(), std::mem::take(&mut self.pending)));
                }
            }
        }
        self.poll_job();
    }

    fn add_game(&mut self, game: Game) {
        if self.games.iter().any(|g| g.game.source == game.source) {
            return;
        }
        // An XISO that converting wouldn't shrink is listed, but not selected.
        let selected = !(game.already_trimmed(self.settings.strip_system_update) && !self.settings.cso);
        let id = self.next_id;
        self.next_id += 1;
        self.games.push(GameView { id, game, selected, status: Status::Ready, icon: None });
        self.games.sort_by_key(|g| g.game.title().to_lowercase());
    }

    fn poll_job(&mut self) {
        let Some(job) = &mut self.job else { return };
        let kind = job.kind;
        let mut msgs = Vec::new();
        loop {
            match job.rx.try_recv() {
                Ok(m) => msgs.push(m),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    msgs.push(JobMsg::AllDone);
                    break;
                }
            }
        }
        for m in msgs {
            let Some(job) = &mut self.job else { return };
            match m {
                JobMsg::Started(id) => {
                    job.current = Some(id);
                    job.phase_base = 0;
                    job.phase_total = 0;
                    let total = job.weights.get(&id).copied().unwrap_or(1);
                    let name = self.game_title(id);
                    self.log(format!("{} {name}", kind.verb()), false);
                    self.set_status(id, Status::Working { phase: kind.first_phase(), done: 0, total });
                }
                JobMsg::Progress(id, Progress::Phase(phase)) => {
                    job.phase_base += job.phase_total;
                    job.phase_total = 0;
                    let base = job.phase_base;
                    self.update_working(id, Some(phase), base);
                }
                JobMsg::Progress(id, Progress::Bytes { done, total: phase_total }) => {
                    job.phase_total = phase_total;
                    let base = job.phase_base;
                    self.update_working(id, None, base + done);
                }
                JobMsg::Progress(_, Progress::Log(msg)) => self.log(msg, false),
                JobMsg::Finished(id, r) => {
                    job.bytes_before += job.weights.get(&id).copied().unwrap_or(0);
                    match r {
                        Ok(paths) => {
                            for p in &paths {
                                self.log(format!("✔ Wrote {}", p.display()), false);
                            }
                            self.finished(id, kind, &paths);
                            self.set_status(id, Status::Done(kind, paths));
                            // Converted games drop out of the next "Convert all".
                            if kind == JobKind::Convert {
                                if let Some(g) = self.game_mut(id) {
                                    g.selected = false;
                                }
                            }
                        }
                        Err(e) => {
                            let name = self.game_title(id);
                            self.log(format!("✖ {name}: {e}"), true);
                            self.set_status(id, Status::Failed(e));
                        }
                    }
                }
                JobMsg::Cancelled(id) => {
                    let name = self.game_title(id);
                    self.log(format!("Cancelled {name}"), false);
                    self.set_status(id, Status::Ready);
                }
                JobMsg::AllDone => {
                    for g in &mut self.games {
                        if g.working() {
                            g.status = Status::Ready;
                        }
                    }
                    self.job = None;
                    return;
                }
            }
        }
    }

    /// After a game was converted with "replace the original", its source is gone: the card
    /// now stands for the new image, so it can still be extracted or shown.
    fn finished(&mut self, id: u64, kind: JobKind, paths: &[PathBuf]) {
        if kind != JobKind::Convert {
            return;
        }
        let Some(g) = self.game_mut(id) else { return };
        if g.game.is_image() && !g.game.source.exists() {
            if let Some(first) = paths.first() {
                if let Ok(game) = xbwrite_core::scan::scan(first) {
                    g.game = game;
                    g.selected = false;
                }
            }
        }
    }

    fn game(&self, id: u64) -> Option<&GameView> {
        self.games.iter().find(|g| g.id == id)
    }

    fn game_mut(&mut self, id: u64) -> Option<&mut GameView> {
        self.games.iter_mut().find(|g| g.id == id)
    }

    fn game_title(&self, id: u64) -> String {
        self.game(id).map(|g| g.game.title().to_string()).unwrap_or_default()
    }

    fn set_status(&mut self, id: u64, s: Status) {
        if let Some(g) = self.game_mut(id) {
            g.status = s;
        }
    }

    /// Moves a working game's progress to `done` bytes of its total, and to `phase` if given.
    fn update_working(&mut self, id: u64, phase: Option<Phase>, done: u64) {
        let Some(g) = self.game_mut(id) else { return };
        if let Status::Working { phase: current, total, .. } = g.status {
            g.status = Status::Working { phase: phase.unwrap_or(current), done: done.min(total), total };
        }
    }

    fn output_dir_for(&self, g: &Game) -> PathBuf {
        if let Some(d) = &self.settings.output_dir {
            return d.clone();
        }
        g.source.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
    }

    fn output_for(&self, g: &Game) -> PathBuf {
        output_path(g, &self.output_dir_for(g), self.settings.format(), self.settings.replace_source)
    }

    /// Expected work of converting `g`, across writing and verifying.
    fn convert_weight(&self, g: &Game) -> u64 {
        let strip = self.settings.strip_system_update;
        let write = match self.settings.format() {
            Format::Xiso => g.copy_size(strip),
            Format::Cso => g.output_size(strip),
        };
        let verify = if self.settings.verify { g.copy_size(strip) } else { 0 };
        (write + verify).max(1)
    }

    fn start_convert(&mut self, ctx: &egui::Context, only: Option<u64>) {
        let opts = self.settings.convert_options();
        let mut jobs = Vec::new();
        let mut weights = HashMap::new();
        for g in &self.games {
            let wanted = match only {
                Some(id) => g.id == id,
                None => g.selected,
            };
            if !wanted {
                continue;
            }
            let task = Task::Convert { output: self.output_for(&g.game), opts: opts.clone() };
            weights.insert(g.id, self.convert_weight(&g.game));
            jobs.push(Job { id: g.id, game: g.game.clone(), task });
        }
        self.start_jobs(ctx, JobKind::Convert, jobs, weights);
    }

    fn start_jobs(&mut self, ctx: &egui::Context, kind: JobKind, jobs: Vec<Job>, weights: HashMap<u64, u64>) {
        if jobs.is_empty() {
            return;
        }
        for g in &mut self.games {
            if weights.contains_key(&g.id) {
                g.status = Status::Queued;
            }
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = spawn_jobs(ctx.clone(), jobs, cancel.clone());
        self.job = Some(RunningJob {
            kind,
            rx,
            cancel,
            started: Instant::now(),
            current: None,
            bytes_total: weights.values().sum(),
            weights,
            bytes_before: 0,
            phase_base: 0,
            phase_total: 0,
        });
    }

    fn open_extract_dialog(&mut self, id: u64) {
        let Some(g) = self.game(id) else { return };
        let dir = self.settings.extract_dir.clone().unwrap_or_else(|| self.output_dir_for(&g.game));
        self.extract_dialog = Some(ExtractDialog { id, dir });
    }

    fn start_extract(&mut self, ctx: &egui::Context, id: u64, output: PathBuf, overwrite: bool) {
        let Some(g) = self.game(id) else { return };
        let opts = ExtractOptions { strip_system_update: self.settings.strip_system_update, overwrite };
        let weight = g.game.copy_size(self.settings.strip_system_update).max(1);
        let job = Job { id, game: g.game.clone(), task: Task::Extract { output, opts } };
        self.start_jobs(ctx, JobKind::Extract, vec![job], HashMap::from([(id, weight)]));
    }

    // ---------------------------------------------------------------- UI

    fn header(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        let logo = self.logo_texture(ui.ctx(), p.accent);
        ui.horizontal(|ui| {
            ui.add(egui::Image::new((logo.id(), egui::vec2(34.0, 34.0))));
            ui.add_space(2.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new("XbWrite").font(theme::semibold(17.0)).color(p.text));
                ui.label(RichText::new("Xbox disc images → trimmed XISO").size(12.0).color(p.text_dim));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, true, "⚙", "Settings").clicked() {
                    self.show_settings = true;
                }
                let (icon, tip, next) = match self.settings.theme {
                    ThemeChoice::Dark => ("☀", "Switch to light mode", ThemeChoice::Light),
                    ThemeChoice::Light => ("🌙", "Switch to dark mode", ThemeChoice::Dark),
                    ThemeChoice::System => ("◐", "Following system theme. Click for dark mode.", ThemeChoice::Dark),
                };
                if theme::icon_button(ui, true, icon, tip).clicked() {
                    self.settings.theme = next;
                    ui.ctx().set_theme(next.preference());
                }
                ui.add_space(6.0);
                let fmt = self.settings.format().label();
                let mut parts = vec![fmt.to_string()];
                if self.settings.strip_system_update {
                    parts.push("no dashboard update".into());
                }
                if self.settings.verify {
                    parts.push("verified".into());
                }
                status_pill(ui, p.accent, &parts.join(" · ")).on_hover_text("Conversion settings. Change them in ⚙.");
            });
        });
    }

    /// The app icon rendered in the current accent colour (cached per colour).
    fn logo_texture(&mut self, ctx: &egui::Context, accent: Color32) -> egui::TextureHandle {
        if let Some((c, tex)) = &self.logo {
            if *c == accent {
                return tex.clone();
            }
        }
        let rgba = crate::icon::render(128, [accent.r(), accent.g(), accent.b()]);
        let img = egui::ColorImage::from_rgba_unmultiplied([128, 128], &rgba);
        let tex = ctx.load_texture("logo", img, egui::TextureOptions::LINEAR);
        self.logo = Some((accent, tex.clone()));
        tex
    }

    /// A file dialog opening where the user last added games from.
    fn input_dialog(&self) -> rfd::FileDialog {
        let d = rfd::FileDialog::new();
        match &self.settings.last_dir {
            Some(dir) => d.set_directory(dir),
            None => d,
        }
    }

    fn add_files_dialog(&mut self, ctx: &egui::Context) {
        if let Some(files) = self.input_dialog().add_filter("Xbox images", &EXTENSIONS).pick_files() {
            self.settings.last_dir = files.first().and_then(|f| f.parent().map(Path::to_path_buf));
            self.add_paths(ctx, files);
        }
    }

    fn add_folder_dialog(&mut self, ctx: &egui::Context) {
        if let Some(dir) = self.input_dialog().pick_folder() {
            self.settings.last_dir = Some(dir.clone());
            self.add_paths(ctx, vec![dir]);
        }
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        if let Some(job) = &self.job {
            let current = job.current.and_then(|id| self.game(id));
            let (phase, cur_done) = match current.map(|g| &g.status) {
                Some(Status::Working { phase, done, .. }) => (Some(*phase), *done),
                _ => (None, 0),
            };
            let total = job.bytes_total.max(1);
            let done = (job.bytes_before + cur_done).min(total);
            let secs = job.started.elapsed().as_secs_f64().max(0.001);
            let speed = done as f64 / secs;
            let eta = if speed > 0.0 { total.saturating_sub(done) as f64 / speed } else { 0.0 };
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(phase.map(Phase::label).unwrap_or("Starting")).color(p.text_dim));
                ui.label(RichText::new(current.map(|g| g.game.title()).unwrap_or("…")).font(theme::semibold(14.0)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::ghost_button(ui, true, "Cancel").clicked() {
                        job.cancel.store(true, Ordering::Relaxed);
                    }
                    let text = format!(
                        "{} of {}  ·  {}/s  ·  {} left",
                        format_size(done),
                        format_size(total),
                        format_size(speed as u64),
                        fmt_secs(eta)
                    );
                    ui.label(RichText::new(text).color(p.text_dim))
                        .on_hover_text("Counts the reading back for verification as well as the writing.");
                });
            });
            let w = ui.available_width();
            theme::progress_bar(ui, egui::Id::new("overall"), done as f32 / total as f32, w);
            ui.add_space(10.0);
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("Output").color(p.text_dim));
            let label = match &self.settings.output_dir {
                Some(d) => short_path(d, 36),
                None => "Same folder as source".into(),
            };
            let hover = match &self.settings.output_dir {
                Some(d) => d.display().to_string(),
                None => "Each image is written next to its source. Click to pick a folder.".into(),
            };
            if theme::secondary_button(ui, true, format!("{label}  ⏷")).on_hover_text(hover).clicked() {
                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                    self.settings.output_dir = Some(d);
                }
            }
            if self.settings.output_dir.is_some() && theme::icon_button(ui, true, "✕", "Use source folder").clicked()
            {
                self.settings.output_dir = None;
            }
            ui.add_space(10.0);
            theme::segmented(ui, &mut self.settings.cso, &[(false, "XISO"), (true, "CSO")]);
            ui.add_space(10.0);
            theme::toggle(
                ui,
                &mut self.settings.strip_system_update,
                "Drop dashboard update",
                "Leave out the $SystemUpdate folder. Only an unmodified console with an old\n\
                 dashboard ever installs it; emulators and modded consoles never do.",
            );
            ui.add_space(6.0);
            theme::toggle(
                ui,
                &mut self.settings.split_fat32,
                "Split for FAT32",
                "Cut images over 4 GB into .1.iso, .2.iso, … parts",
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let n = self.games.iter().filter(|g| g.selected).count();
                let can = !self.busy() && n > 0;
                let text = match n {
                    0 => "Convert".to_string(),
                    1 => "Convert 1 game".to_string(),
                    n => format!("Convert {n} games"),
                };
                if theme::primary_button(ui, can, &format!("   {text}   ")).clicked() {
                    self.start_convert(ui.ctx(), None);
                }
                let errors = self.log.iter().filter(|l| l.1).count();
                let icon = if errors > 0 { format!("☰ {errors}") } else { "☰".into() };
                let tip = if self.show_log { "Hide activity log" } else { "Show activity log" };
                if theme::icon_button(ui, true, &icon, tip).clicked() {
                    self.show_log = !self.show_log;
                }
            });
        });
    }

    fn log_panel(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Activity").font(theme::semibold(13.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::ghost_button(ui, true, "Clear").clicked() {
                    self.log.clear();
                }
            });
        });
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            if self.log.is_empty() {
                ui.label(RichText::new("Nothing yet.").color(p.text_faint));
            }
            for (msg, err) in &self.log {
                let c = if *err { p.err } else { p.text_dim };
                ui.label(RichText::new(msg).monospace().color(c));
            }
        });
    }

    fn drop_zone(&mut self, ui: &mut egui::Ui, hovering: bool) {
        let p = theme::palette(ui);
        let avail = ui.available_rect_before_wrap();
        let size = egui::vec2(avail.width().min(640.0), 320.0);
        let rect = egui::Rect::from_center_size(avail.center() - egui::vec2(0.0, 24.0), size);
        let zone = ui.allocate_rect(rect, egui::Sense::click());
        let hot = hovering || zone.hovered();
        let t = ui.ctx().animate_bool_with_time(egui::Id::new("dz-hot"), hot, 0.15);
        let painter = ui.painter();
        painter.rect_filled(rect, 20.0, theme::lerp_color(p.card, p.accent.gamma_multiply(0.10), t));
        theme::dashed_rect(
            painter,
            rect.shrink(1.0),
            egui::Stroke::new(1.5, theme::lerp_color(p.border_strong, p.accent, t)),
        );
        if zone.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.add_files_dialog(ui.ctx());
        }

        let inner = rect.shrink2(egui::vec2(40.0, 34.0));
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Center)),
            |ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(64.0, 64.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 32.0, p.accent.gamma_multiply(0.16));
                ui.painter().text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    "⬇",
                    egui::FontId::proportional(28.0),
                    p.accent,
                );
                ui.add_space(6.0);
                ui.label(RichText::new("Drop your Xbox games here").font(theme::semibold(22.0)).color(p.text));
                ui.label(
                    RichText::new(
                        "Each game is rebuilt from its files alone: no video partition, no filler, \
                     just what the console reads.",
                    )
                    .color(p.text_dim),
                );
                ui.add_space(12.0);
                let bw = 136.0;
                ui.allocate_ui_with_layout(
                    egui::vec2(bw * 2.0 + 10.0, 36.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        if theme::primary_button_sized(ui, true, "Add files", egui::vec2(bw, 36.0)).clicked() {
                            self.add_files_dialog(ui.ctx());
                        }
                        if ui
                            .add_sized([bw, 36.0], egui::Button::new("Add folder").corner_radius(p.button_radius))
                            .clicked()
                        {
                            self.add_folder_dialog(ui.ctx());
                        }
                    },
                );
                ui.add_space(10.0);
                ui.label(
                    RichText::new("Redump ISO  ·  XISO  ·  CSO  ·  extracted game folder")
                        .size(12.0)
                        .color(p.text_faint),
                );
            },
        );

        if !self.scan_errors.is_empty() {
            let area = egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.bottom() + 16.0),
                egui::vec2(rect.width(), 140.0),
            );
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.label(
                        RichText::new(format!("Couldn't read {} item(s)", self.scan_errors.len()))
                            .font(theme::semibold(13.0))
                            .color(p.err),
                    );
                    for (path, e) in self.scan_errors.iter().take(4) {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{}: {e}", file_name(path))).size(12.5).color(p.text_dim),
                            )
                            .truncate(),
                        )
                        .on_hover_text(format!("{}\n{e}", path.display()));
                    }
                    if self.scan_errors.len() > 4 {
                        ui.label(
                            RichText::new(format!("…and {} more (see the activity log)", self.scan_errors.len() - 4))
                                .size(12.5)
                                .color(p.text_faint),
                        );
                    }
                },
            );
        }
    }

    fn game_list(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        let busy = self.busy();
        let strip = self.settings.strip_system_update;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Games").font(theme::semibold(20.0)));
            theme::pill(ui, self.games.len().to_string(), p.text_dim);
            let selected: Vec<&GameView> = self.games.iter().filter(|g| g.selected).collect();
            let before: u64 = selected.iter().map(|g| g.game.source_size).sum();
            let after: u64 = selected.iter().map(|g| g.game.output_size(strip)).sum();
            if before > after && self.settings.format() == Format::Xiso {
                theme::pill(ui, format!("Saves {}", format_size(before - after)), p.ok).on_hover_text(format!(
                    "{} → {} for the selected games",
                    format_size(before),
                    format_size(after)
                ));
            }
            if self.scan_rx.is_some() {
                ui.add_space(4.0);
                ui.spinner();
                ui.label(RichText::new("Reading images…").color(p.text_dim));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::ghost_button(ui, !busy, "Clear all").clicked() {
                    self.clear();
                }
                if theme::secondary_button(ui, !busy, "Add folder").clicked() {
                    self.add_folder_dialog(ui.ctx());
                }
                if theme::secondary_button(ui, !busy, "Add files").clicked() {
                    self.add_files_dialog(ui.ctx());
                }
            });
        });
        ui.add_space(10.0);

        let outputs: Vec<PathBuf> = self.games.iter().map(|g| self.output_for(&g.game)).collect();
        let ctx = CardCtx { busy: self.job.is_some(), strip, format: self.settings.format() };
        let mut convert_one = None;
        let mut extract_one = None;
        let mut remove = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (i, g) in self.games.iter_mut().enumerate() {
                match game_card(ui, g, &ctx, &outputs[i]) {
                    CardAction::None => {}
                    CardAction::Convert => convert_one = Some(g.id),
                    CardAction::Extract => extract_one = Some(g.id),
                    CardAction::Remove => remove = Some(i),
                }
                ui.add_space(10.0);
            }
            for (path, e) in &self.scan_errors {
                ui.label(RichText::new(format!("✖ {}: {e}", file_name(path))).color(p.err));
            }
        });
        if let Some(id) = convert_one {
            self.start_convert(ui.ctx(), Some(id));
        }
        if let Some(id) = extract_one {
            self.open_extract_dialog(id);
        }
        if let Some(i) = remove {
            let g = self.games.remove(i);
            self.inputs.retain(|p| *p != g.game.source);
        }
    }

    fn settings_modal(&mut self, ctx: &egui::Context) {
        let p = theme::palette_ctx(ctx);
        let mut close = false;
        let max_h = (ctx.content_rect().height() - 120.0).max(200.0);
        let modal = egui::Modal::new(egui::Id::new("settings")).frame(modal_frame(&p)).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.set_max_width(520.0);
            egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                close |= modal_header(ui, "Settings");

                section(ui, "APPEARANCE");
                let mut choice = self.settings.theme;
                if theme::segmented(
                    ui,
                    &mut choice,
                    &[(ThemeChoice::Dark, "Dark"), (ThemeChoice::Light, "Light"), (ThemeChoice::System, "System")],
                ) {
                    self.settings.theme = choice;
                    ui.ctx().set_theme(choice.preference());
                }
                ui.add_space(4.0);
                let mut style = self.settings.style;
                if theme::segmented(ui, &mut style, &[(ThemeStyle::Duke, "Duke"), (ThemeStyle::Violet, "Violet")]) {
                    self.settings.style = style;
                    theme::apply_style(ui.ctx(), style);
                }

                section(ui, "OUTPUT FORMAT");
                theme::segmented(ui, &mut self.settings.cso, &[(false, "XISO"), (true, "CSO")]);
                ui.add_space(4.0);
                let note = if self.settings.cso {
                    "LZ4-compressed, often a third smaller again. Read by Cerbios, Stellar and XBMC4Gamers \
                     on the console, but not by xemu."
                } else {
                    "A plain XISO. Works everywhere: xemu, Cerbios, Stellar, XBMC4Gamers, and for burning."
                };
                ui.add(egui::Label::new(RichText::new(note).size(12.0).color(p.text_dim)).wrap());

                section(ui, "CONVERTING");
                setting_toggle(
                    ui,
                    &mut self.settings.strip_system_update,
                    "Drop the dashboard update",
                    "Leave out $SystemUpdate. Only an unmodified console on an older dashboard installs it.",
                );
                setting_toggle(
                    ui,
                    &mut self.settings.split_fat32,
                    "Split for FAT32",
                    "Cut images over 4 GB into .1.iso, .2.iso, … parts for FAT32 drives.",
                );
                setting_toggle(
                    ui,
                    &mut self.settings.verify,
                    "Verify after writing",
                    "Read the new image back and compare every file with the source before keeping it.",
                );
                setting_toggle(
                    ui,
                    &mut self.settings.replace_source,
                    "Replace the original image",
                    "Delete the source image once the new one is written (and verified, if that's on). \
                     Extracted game folders are never deleted.",
                );
                if self.settings.replace_source && !self.settings.verify {
                    ui.label(
                        RichText::new("⚠  Originals are deleted without checking the new images first.")
                            .size(12.5)
                            .color(p.warn),
                    );
                }
            });
            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_button(ui, true, "   Done   ").clicked() {
                    close = true;
                }
            });
        });
        if close || modal.should_close() {
            self.show_settings = false;
        }
    }

    fn extract_modal(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.extract_dialog.take() else { return };
        // The game was removed while the dialog was open: close it.
        // Borrows only `games`: the dialog below edits `settings`.
        let Some(g) = self.games.iter().find(|g| g.id == dlg.id) else { return };
        let p = theme::palette_ctx(ctx);
        let can_start = !self.busy();
        let mut close = false;
        let mut start = false;
        let target = dlg.dir.join(&g.game.name);
        let exists = target.exists();
        let replaceable = exists && is_game_folder(&target);
        let modal = egui::Modal::new(egui::Id::new("extract")).frame(modal_frame(&p)).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.set_max_width(460.0);
            close |= modal_header(ui, "Extract to folder");
            ui.add_space(8.0);
            ui.label(RichText::new(g.game.title()).font(theme::semibold(15.0)));
            ui.label(
                RichText::new("Writes every file of the game to a folder, for modding or for loaders that run games from folders.")
                    .size(12.0)
                    .color(p.text_dim),
            );

            section(ui, "DESTINATION");
            let label = format!("{}  ⏷", short_path(&dlg.dir, 40));
            if theme::secondary_button(ui, true, label).on_hover_text(dlg.dir.display().to_string()).clicked() {
                if let Some(d) = rfd::FileDialog::new().set_directory(&dlg.dir).pick_folder() {
                    dlg.dir = d;
                }
            }
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(format!("→  {}", g.game.name)).size(12.5).color(p.text_dim)).truncate())
                .on_hover_text(target.display().to_string());

            section(ui, "CONTENT");
            setting_toggle(
                ui,
                &mut self.settings.strip_system_update,
                "Drop the dashboard update",
                "Leave out the $SystemUpdate folder.",
            );

            if exists {
                ui.add_space(4.0);
                let (msg, c) = if replaceable {
                    ("An earlier extraction is already there. Extracting replaces it.", p.warn)
                } else {
                    ("A folder with this name exists and isn't an extracted game. Pick another destination.", p.err)
                };
                ui.label(RichText::new(msg).color(c));
            }

            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = if exists { "   Replace   " } else { "   Extract   " };
                if theme::primary_button(ui, can_start && (!exists || replaceable), text).clicked() {
                    start = true;
                }
                if theme::ghost_button(ui, true, "Cancel").clicked() {
                    close = true;
                }
            });
        });
        if start {
            self.settings.extract_dir = Some(dlg.dir.clone());
            self.start_extract(ctx, dlg.id, target, replaceable);
        } else if !(close || modal.should_close()) {
            self.extract_dialog = Some(dlg);
        }
    }
}

/// Frame shared by the modal dialogs.
fn modal_frame(p: &theme::Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.panel)
        .stroke(egui::Stroke::new(1.0, p.border))
        .corner_radius(16)
        .inner_margin(24)
        .shadow(egui::epaint::Shadow { offset: [0, 16], blur: 48, spread: 0, color: p.shadow })
}

/// A modal dialog's title with a close button. Returns true when the button is clicked.
fn modal_header(ui: &mut egui::Ui, title: &str) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(theme::semibold(20.0)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            close = theme::icon_button(ui, true, "✕", "Close").clicked();
        });
    });
    close
}

fn section(ui: &mut egui::Ui, title: &str) {
    let p = theme::palette(ui);
    ui.add_space(14.0);
    ui.label(RichText::new(title).font(theme::semibold(11.5)).color(p.text_faint));
    ui.add_space(2.0);
}

fn setting_toggle(ui: &mut egui::Ui, value: &mut bool, label: &str, hint: &str) {
    let p = theme::palette(ui);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        theme::toggle(ui, value, label, "");
        ui.horizontal(|ui| {
            ui.add_space(42.0);
            ui.add(egui::Label::new(RichText::new(hint).size(12.0).color(p.text_dim)).wrap());
        });
    });
    ui.add_space(6.0);
}

fn status_pill(ui: &mut egui::Ui, color: Color32, text: &str) -> egui::Response {
    let dark = ui.visuals().dark_mode;
    // Own row so it sizes to its content inside right-aligned layouts; the dot is painted
    // in the left padding so it stays on the left whatever the layout direction.
    let resp = ui
        .horizontal(|ui| {
            egui::Frame::new()
                .fill(color.gamma_multiply(if dark { 0.14 } else { 0.10 }))
                .corner_radius(20)
                .inner_margin(egui::Margin { left: 23, right: 11, top: 4, bottom: 4 })
                .show(ui, |ui| {
                    ui.label(RichText::new(text).font(theme::semibold(12.5)).color(color));
                })
                .response
        })
        .inner;
    ui.painter().circle_filled(egui::pos2(resp.rect.left() + 13.0, resp.rect.center().y), 4.0, color);
    resp
}

/// The last component of `p`, for showing in the UI.
fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn short_path(p: &Path, max: usize) -> String {
    let s = p.display().to_string();
    if s.chars().count() <= max {
        return s;
    }
    let tail: String = s.chars().rev().take(max - 1).collect::<Vec<_>>().into_iter().rev().collect();
    format!("…{tail}")
}

enum CardAction {
    None,
    Convert,
    Extract,
    Remove,
}

/// Settings every card needs.
struct CardCtx {
    busy: bool,
    strip: bool,
    format: Format,
}

fn kind_color(p: &theme::Palette, kind: SourceKind) -> Color32 {
    match kind {
        SourceKind::Image { cso: true, .. } => p.alt,
        SourceKind::Image { layout: Layout::Redump(_), .. } => p.info,
        SourceKind::Image { layout: Layout::Xiso, .. } => p.ok,
        SourceKind::Folder => p.warn,
    }
}

fn kind_hint(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Image { cso: true, .. } => "A compressed CSO image",
        SourceKind::Image { layout: Layout::Redump(_), .. } => {
            "A full dump of the disc, with the DVD video partition and the filler between files"
        }
        SourceKind::Image { layout: Layout::Xiso, .. } => "Already an XISO: the file system starts at the beginning",
        SourceKind::Folder => "An extracted game folder",
    }
}

/// The "Extract…" button on a game card. Returns true when clicked.
fn extract_button(ui: &mut egui::Ui, busy: bool, g: &Game) -> bool {
    if !g.is_image() {
        return false;
    }
    theme::secondary_button(ui, !busy, "Extract…").on_hover_text("Extract the game's files to a folder").clicked()
}

fn game_card(ui: &mut egui::Ui, g: &mut GameView, c: &CardCtx, output: &Path) -> CardAction {
    let p = theme::palette(ui);
    let mut action = CardAction::None;
    let id = egui::Id::new(("card", g.id));
    let hovered = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let t = ui.ctx().animate_bool_with_time(id.with("h"), hovered, 0.12);
    let selected_t = ui.ctx().animate_bool_with_time(id.with("s"), g.selected, 0.12);
    let border = theme::lerp_color(p.border, p.border_strong, t);
    let frame = egui::Frame::new()
        .fill(p.card)
        .stroke(egui::Stroke::new(1.0, border))
        .corner_radius(14)
        .inner_margin(16)
        .shadow(egui::epaint::Shadow { offset: [0, 2], blur: 12, spread: 0, color: p.shadow.gamma_multiply(0.6) });

    if g.icon.is_none() {
        if let Some(icon) = &g.game.icon {
            let img = egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
            g.icon = Some(ui.ctx().load_texture(format!("title-{}", g.id), img, egui::TextureOptions::LINEAR));
        }
    }

    let resp = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            check_circle(ui, &mut g.selected, !c.busy, 22.0)
                .on_hover_text("Include when converting all selected games");
            ui.add_space(2.0);
            let size = egui::vec2(76.0, 76.0);
            let alpha = 0.45 + 0.55 * selected_t;
            match &g.icon {
                Some(tex) => {
                    ui.add(
                        egui::Image::new((tex.id(), size)).corner_radius(12).tint(Color32::WHITE.gamma_multiply(alpha)),
                    );
                }
                None => {
                    let (r, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.painter().rect_filled(r, 12.0, p.control);
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        "💿",
                        egui::FontId::proportional(30.0),
                        p.text_faint,
                    );
                }
            }
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 5.0;
                ui.add_space(2.0);
                ui.add(
                    egui::Label::new(RichText::new(g.game.title()).font(theme::semibold(18.0)).color(p.text))
                        .truncate(),
                );
                let mut sub = Vec::new();
                if let Some(x) = &g.game.xbe {
                    sub.push(x.title_id_label());
                    let regions = x.region_codes();
                    if !regions.is_empty() {
                        sub.push(regions.join("/"));
                    }
                    if x.disc_number > 0 {
                        sub.push(format!("Disc {}", x.disc_number + 1));
                    }
                    sub.push(format!("{} files", g.game.file_count));
                } else {
                    sub.push(format!("{} files  ·  no default.xbe found", g.game.file_count));
                }
                let sub_resp = ui.label(RichText::new(sub.join("  ·  ")).size(12.5).color(p.text_dim));
                if let Some(x) = &g.game.xbe {
                    sub_resp.on_hover_text(format!("Title ID {:08X}, version {}", x.title_id, x.version));
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    theme::pill(ui, g.game.kind_label(), kind_color(&p, g.game.kind))
                        .on_hover_text(kind_hint(g.game.kind));
                    if g.game.system_update_size > 0 {
                        let (text, color) = if c.strip {
                            (format!("Update −{}", format_size(g.game.system_update_size)), p.text_dim)
                        } else {
                            (format!("Update {}", format_size(g.game.system_update_size)), p.text_faint)
                        };
                        let tip = if c.strip {
                            "The $SystemUpdate folder will be left out"
                        } else {
                            "The $SystemUpdate folder will be kept"
                        };
                        theme::pill(ui, text, color).on_hover_text(tip);
                    }
                    if g.game.already_trimmed(c.strip) {
                        theme::pill(ui, "Already trimmed", p.ok)
                            .on_hover_text("Converting this XISO wouldn't make it any smaller");
                    }
                    ui.add_space(4.0);
                    let before = g.game.source_size;
                    let after = g.game.output_size(c.strip);
                    ui.label(RichText::new(format_size(before)).color(p.text_dim));
                    ui.label(RichText::new("→").color(p.text_faint));
                    if c.format == Format::Cso {
                        ui.label(RichText::new(format!("CSO, under {}", format_size(after))).color(p.text_dim))
                            .on_hover_text("Compressed size depends on the data. This is the uncompressed XISO size.");
                    } else {
                        ui.label(RichText::new(format_size(after)).color(p.text));
                        if before > after {
                            ui.label(
                                RichText::new(format!("saves {}%", percent(before - after, before)))
                                    .font(theme::semibold(13.0))
                                    .color(p.ok),
                            );
                        }
                    }
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, !c.busy, "✕", "Remove from list").clicked() {
                    action = CardAction::Remove;
                }
                match &g.status {
                    Status::Done(kind, paths) => {
                        if let Some(path) = paths.first() {
                            if theme::secondary_button(ui, true, "Show in folder")
                                .on_hover_text(path.display().to_string())
                                .clicked()
                            {
                                reveal(path);
                            }
                        }
                        if extract_button(ui, c.busy, &g.game) {
                            action = CardAction::Extract;
                        }
                        status_pill(ui, p.ok, kind.done());
                    }
                    Status::Working { phase, done, total } => {
                        let pct = *done as f32 / (*total).max(1) as f32 * 100.0;
                        status_pill(ui, p.accent, &format!("{} {pct:.0}%", phase.label()));
                    }
                    Status::Queued => {
                        status_pill(ui, p.text_dim, "Queued");
                    }
                    Status::Failed(e) => {
                        if theme::secondary_button(ui, !c.busy, "Retry").clicked() {
                            action = CardAction::Convert;
                        }
                        if extract_button(ui, c.busy, &g.game) {
                            action = CardAction::Extract;
                        }
                        status_pill(ui, p.err, "Failed").on_hover_text(e);
                    }
                    Status::Ready => {
                        if theme::secondary_button(ui, !c.busy, "Convert").clicked() {
                            action = CardAction::Convert;
                        }
                        if extract_button(ui, c.busy, &g.game) {
                            action = CardAction::Extract;
                        }
                    }
                }
            });
        });

        if let Status::Working { done, total, .. } = g.status {
            ui.add_space(10.0);
            let w = ui.available_width();
            theme::progress_bar(ui, id.with("bar"), done as f32 / total.max(1) as f32, w);
        }
        if let Status::Failed(e) = &g.status {
            ui.add_space(6.0);
            ui.label(RichText::new(format!("✖ {e}")).size(12.5).color(p.err));
        }

        ui.add_space(10.0);
        let rect = ui.available_rect_before_wrap();
        ui.painter().hline(rect.x_range(), rect.top(), egui::Stroke::new(1.0, p.border));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let src = file_name(&g.game.source);
            let parts =
                if g.game.parts.len() > 1 { format!("  (+{} parts)", g.game.parts.len() - 1) } else { String::new() };
            ui.add(egui::Label::new(RichText::new(format!("{src}{parts}")).size(12.5).color(p.text_faint)).truncate())
                .on_hover_text(g.game.source.display().to_string());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let name = file_name(output);
                let just_written = matches!(&g.status, Status::Done(_, paths) if paths.iter().any(|p| p == output));
                let replaces_other = output.exists() && !just_written && !g.game.parts.contains(&output.to_path_buf());
                let (text, color, tip) = if replaces_other {
                    (
                        format!("→  {name}  (replaces existing)"),
                        p.warn,
                        format!("{}\nA file with this name exists and will be replaced.", output.display()),
                    )
                } else {
                    (format!("→  {name}"), p.text_dim, output.display().to_string())
                };
                ui.add(egui::Label::new(RichText::new(text).size(12.5).color(color)).truncate()).on_hover_text(tip);
            });
        });
    });
    let hovered_now = ui.rect_contains_pointer(resp.response.rect);
    if hovered_now != hovered {
        ui.ctx().data_mut(|d| d.insert_temp(id, hovered_now));
    }
    action
}

/// Round checkbox.
fn check_circle(ui: &mut egui::Ui, on: &mut bool, enabled: bool, size: f32) -> egui::Response {
    let p = theme::palette(ui);
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(size, size), sense);
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let painter = ui.painter();
    let c = rect.center();
    let r = size / 2.0 - 1.0;
    let ring = if resp.hovered() { p.text_dim } else { p.border_strong };
    painter.circle_stroke(c, r, egui::Stroke::new(1.5, theme::lerp_color(ring, p.accent, t)));
    if t > 0.0 {
        painter.circle_filled(c, r * t, p.accent);
        let s = size / 22.0;
        let pts = vec![c + egui::vec2(-5.0, 0.0) * s, c + egui::vec2(-1.5, 3.5) * s, c + egui::vec2(5.0, -4.0) * s];
        painter.add(egui::Shape::line(pts, egui::Stroke::new(2.0 * s, Color32::WHITE.gamma_multiply(t))));
    }
    if !enabled {
        return resp;
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn fmt_secs(s: f64) -> String {
    let s = s.round() as u64;
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// Opens the system file manager with `p` selected.
fn reveal(p: &Path) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg("/select,").arg(p).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(dir) = p.parent() {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.poll(ctx);
        if let Some(path) = &self.debug.screenshot {
            debug_screenshot(ctx, path);
        }
        if self.debug.auto_convert && !self.games.is_empty() && self.scan_rx.is_none() && !self.busy() {
            self.debug.auto_convert = false;
            self.start_convert(ctx, None);
        }

        // Drag & drop.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            if self.job.is_some() {
                self.log("Can't add games while converting. Wait for it to finish.", true);
            } else {
                self.add_paths(ctx, dropped);
            }
        }
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let p = theme::palette(ui);

        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(18, 12)))
            .show(ui, |ui| self.header(ui));
        egui::Panel::bottom("bottom")
            .frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(20, 14)))
            .show(ui, |ui| self.bottom_bar(ui));
        if self.show_log {
            egui::Panel::bottom("log")
                .resizable(true)
                .default_size(150.0)
                .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(20, 10)))
                .show(ui, |ui| self.log_panel(ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin { left: 20, right: 20, top: 18, bottom: 8 }))
            .show(ui, |ui| {
                if self.games.is_empty() && self.scan_rx.is_none() {
                    self.drop_zone(ui, hovering);
                } else {
                    self.game_list(ui);
                }
            });

        if self.show_settings {
            self.settings_modal(ctx);
        }
        if self.extract_dialog.is_some() {
            self.extract_modal(ctx);
        }

        if hovering && !self.games.is_empty() {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
            let rect = ctx.content_rect();
            painter.rect_filled(rect, 0.0, p.bg.gamma_multiply(0.85));
            let inner = rect.shrink(18.0);
            painter.rect_filled(inner, 18.0, p.accent.gamma_multiply(0.08));
            theme::dashed_rect(&painter, inner, egui::Stroke::new(2.0, p.accent));
            painter.text(
                inner.center() - egui::vec2(0.0, 14.0),
                egui::Align2::CENTER_CENTER,
                "Drop to add",
                theme::semibold(28.0),
                p.text,
            );
            painter.text(
                inner.center() + egui::vec2(0.0, 20.0),
                egui::Align2::CENTER_CENTER,
                "Images and game folders, or folders full of them",
                egui::FontId::proportional(14.0),
                p.text_dim,
            );
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.debug.screenshot.is_some() {
            return; // debug runs mustn't change the user's saved settings
        }
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }

    /// Cancels a running job and gives it a moment to remove its partial output, which would
    /// otherwise be left on disk.
    fn on_exit(&mut self) {
        let Some(job) = self.job.take() else { return };
        job.cancel.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match job.rx.recv_timeout(left) {
                Ok(JobMsg::AllDone) | Err(_) => break,
                Ok(_) => {}
            }
        }
    }
}

/// Saves a screenshot of the window to `path` after ~2 s and exits (see [`DebugHooks`]).
fn debug_screenshot(ctx: &egui::Context, path: &Path) {
    if ctx.cumulative_frame_nr() == 120 {
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
    }
    let shot = ctx.input(|i| {
        i.raw.events.iter().find_map(|e| match e {
            egui::Event::Screenshot { image, .. } => Some(image.clone()),
            _ => None,
        })
    });
    if let Some(image) = shot {
        let [w, h] = image.size;
        let raw: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
        if let Some(img) = image::RgbaImage::from_raw(w as u32, h as u32, raw) {
            let _ = img.save(path);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    ctx.request_repaint();
}
