//! RocketZip desktop window.
//!
//!   RocketZip [archive]                      open and browse an archive
//!   RocketZip --add FILES...                 "Add to archive" dialog, pre-filled
//!   RocketZip --extract-here ARCHIVES...     extract next to each archive
//!   RocketZip --extract-to ARCHIVES...       extract into a folder named after each
//!
//! Files can also be dropped onto the window: an archive opens, anything else
//! goes into a new archive.
#![cfg_attr(windows, windows_subsystem = "windows")]

use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use rocketzip::formats::{detect_format, is_stream, strip_archive_ext, writable_formats};
use rocketzip::ultra::UltraStats;
use rocketzip::*;

type Apply = Box<dyn FnOnce(&mut App) + Send>;

#[derive(Default, Clone)]
struct Prog {
    name: String,
    done: u64,
    total: u64,
}

const DICTS: [(&str, Option<u32>); 5] = [
    ("Auto (up to 64 MB)", None),
    ("128 MB  (~1.4 GB RAM)", Some(128)),
    ("256 MB  (~2.8 GB RAM)", Some(256)),
    ("512 MB  (~5.5 GB RAM)", Some(512)),
    ("1024 MB (~11 GB RAM)", Some(1024)),
];
const DOCS: [(&str, &str); 3] = [
    ("exact - byte-identical (recommended)", "exact"),
    ("full - smallest, may rebuild documents", "full"),
    ("off", "off"),
];
const BEST: &str = "best (try all, keep smallest)";

struct CreateDlg {
    sources: Vec<PathBuf>,
    out: String,
    fmt: String,
    level: u32,
    method: String,
    password: String,
    dict: usize,
    docs: usize,
    dedup: bool,
    max: bool,
    selected: Option<usize>,
}

impl CreateDlg {
    fn new() -> Self {
        CreateDlg { sources: vec![], out: String::new(), fmt: "zfx".into(), level: 9, method: "deflate".into(),
                    password: String::new(), dict: 0, docs: 0, dedup: true, max: false, selected: None }
    }

    fn add(&mut self, p: PathBuf) {
        if !self.sources.contains(&p) {
            if self.out.is_empty() {
                let ext = if self.fmt == BEST { String::new() } else { format!(".{}", self.fmt) };
                let parent = p.parent().map(Path::to_path_buf).unwrap_or_default();
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or("archive".into());
                self.out = parent.join(format!("{name}{ext}")).to_string_lossy().into_owned();
            }
            self.sources.push(p);
        }
    }

    fn sync_ext(&mut self) {
        if !self.out.is_empty() && self.fmt != BEST {
            self.out = format!("{}.{}", strip_archive_ext(&self.out), self.fmt);
        }
    }

    fn ultra(&self) -> bool {
        self.fmt == "zfx" || self.fmt == BEST
    }
    fn pw_ok(&self) -> bool {
        self.fmt == "7z" || self.fmt == "zip" || self.fmt == BEST
    }
}

#[derive(Clone)]
enum PwFor {
    Open(PathBuf),
    Queue(Vec<PathBuf>, String),
    Extract(PathBuf, PathBuf),
}

struct App {
    archive: Option<PathBuf>,
    fmt: String,
    password: Option<String>,
    entries: Vec<Entry>,
    filter: String,
    status: String,
    busy: Option<(String, Instant)>,
    prog: Arc<Mutex<Prog>>,
    tx: Sender<Apply>,
    rx: Receiver<Apply>,
    create: Option<CreateDlg>,
    report: Option<UltraStats>,
    message: Option<(String, String, Option<PathBuf>)>,
    pw_prompt: Option<(PwFor, String)>,
    close_when_done: bool,
    quit: bool,
}

impl App {
    fn new(mode: &str, paths: Vec<PathBuf>) -> App {
        let (tx, rx) = channel();
        let mut app = App {
            archive: None, fmt: String::new(), password: None, entries: vec![], filter: String::new(),
            status: "Open an archive, click Add, or drop files here".into(), busy: None,
            prog: Arc::new(Mutex::new(Prog::default())), tx, rx, create: None, report: None, message: None,
            pw_prompt: None, close_when_done: false, quit: false,
        };
        match mode {
            "open" => app.load(paths[0].clone(), None),
            "add" => {
                let mut d = CreateDlg::new();
                for p in &paths {
                    d.add(p.clone());
                }
                if paths.len() > 1
                    && let Some(parent) = dunce::canonicalize(&paths[0]).ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
                        let name = parent.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or("archive".into());
                        d.out = parent.join(format!("{name}.{}", d.fmt)).to_string_lossy().into_owned();
                    }
                app.create = Some(d);
            }
            "extract-here" | "extract-to" => {
                app.close_when_done = true;
                app.extract_queue(paths, mode.to_string(), None);
            }
            _ => {}
        }
        app
    }

    fn run_bg(&mut self, label: &str, job: impl FnOnce(&ProgressFn) -> Apply + Send + 'static) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some((label.to_string(), Instant::now()));
        self.status = format!("{label}...");
        *self.prog.lock().unwrap() = Prog::default();
        let (tx, prog) = (self.tx.clone(), self.prog.clone());
        std::thread::spawn(move || {
            let cb = move |name: &str, done: u64, total: u64| {
                if let Ok(mut p) = prog.lock() {
                    *p = Prog { name: name.to_string(), done, total };
                }
            };
            // A crash inside a job must not leave the window stuck on "busy".
            let apply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&cb)))
                .unwrap_or_else(|_| Box::new(|app: &mut App| {
                    app.finish();
                    app.close_when_done = false;
                    app.error("Internal error while processing this archive. It may be damaged or \
                               crafted; no files outside the chosen folder were touched.".into());
                }));
            let _ = tx.send(apply);
        });
    }

    fn finish(&mut self) -> f64 {
        
        self.busy.take().map(|(_, t)| t.elapsed().as_secs_f64()).unwrap_or(0.0)
    }

    fn error(&mut self, msg: String) {
        self.status = "Failed".into();
        self.message = Some(("RocketZip - error".into(), msg, None));
    }

    fn load(&mut self, path: PathBuf, password: Option<String>) {
        match list_archive(&path, password.as_deref()) {
            Ok(mut entries) => {
                entries.sort_by_key(|a| a.name.to_lowercase());
                let files: Vec<&Entry> = entries.iter().filter(|e| !e.is_dir).collect();
                let total: u64 = files.iter().filter_map(|e| e.size).sum();
                let size = std::fs::metadata(&path).map(|m| m.len()).ok();
                self.status = format!("{} files, {} unpacked, archive {}", files.len(), human(Some(total)), human(size));
                self.fmt = detect_format(&path).unwrap_or_default();
                self.entries = entries;
                self.archive = Some(path);
                self.password = password;
            }
            Err(e) if e.0.to_lowercase().contains("password") && password.is_none() => {
                self.pw_prompt = Some((PwFor::Open(path), String::new()));
            }
            Err(e) => self.error(format!("Cannot open {}:\n{e}", path.display())),
        }
    }

    fn open_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("Archives", &["zfx", "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "xz", "txz", "zst",
                                      "docx", "xlsx", "pptx", "odt", "epub", "jar", "apk"])
            .add_filter("All files", &["*"])
            .pick_file()
        {
            self.load(p, None);
        }
    }

    fn extract(&mut self) {
        let Some(arc) = self.archive.clone() else {
            self.open_dialog();
            return;
        };
        let start = arc.parent().map(Path::to_path_buf).unwrap_or_default();
        let Some(mut dest) = rfd::FileDialog::new().set_directory(&start).pick_folder() else { return };
        if Some(dest.as_path()) == arc.parent() && !is_stream(&self.fmt) {
            dest = default_dest(&arc, &self.fmt); // don't spill files loose into the parent folder
        }
        self.extract_to(arc, dest, self.password.clone());
    }

    fn extract_to(&mut self, arc: PathBuf, dest: PathBuf, pw: Option<String>) {
        self.run_bg("Extracting", move |cb| {
            let r = extract_archive(&arc, Some(&dest), pw.as_deref(), true, Some(cb));
            Box::new(move |app: &mut App| {
                let secs = app.finish();
                match r {
                    Ok(out) => {
                        app.status = format!("Extracted in {secs:.1}s");
                        app.message = Some(("Extracted".into(), format!("Extracted to:\n{}", out.display()), Some(out)));
                    }
                    Err(e) if e.0.to_lowercase().contains("password") => {
                        app.pw_prompt = Some((PwFor::Extract(arc, dest), String::new()));
                    }
                    Err(e) => app.error(e.0),
                }
            })
        });
    }

    fn extract_queue(&mut self, mut paths: Vec<PathBuf>, mode: String, pw: Option<String>) {
        if paths.is_empty() {
            self.status = "All archives extracted".into();
            if self.close_when_done {
                self.quit = true;
            }
            return;
        }
        let arc = paths.remove(0);
        let fmt = match detect_format(&arc) {
            Ok(f) => f,
            Err(e) => {
                self.error(e.0);
                return self.extract_queue(paths, mode, None);
            }
        };
        let dest = if mode == "extract-here" || is_stream(&fmt) {
            arc.parent().map(Path::to_path_buf).unwrap_or_default()
        } else {
            default_dest(&arc, &fmt)
        };
        self.archive = Some(arc.clone());
        self.fmt = fmt.clone();
        self.entries.clear();
        self.status = format!("{} -> {}", arc.display(), dest.display());
        self.run_bg(&format!("Extracting {}", arc.file_name().unwrap_or_default().to_string_lossy()), move |cb| {
            let r = extract_archive(&arc, Some(&dest), pw.as_deref(), true, Some(cb));
            Box::new(move |app: &mut App| {
                app.finish();
                match r {
                    Ok(_) => app.extract_queue(paths, mode, None),
                    Err(e) if e.0.to_lowercase().contains("password") => {
                        let mut all = vec![arc];
                        all.extend(paths);
                        app.pw_prompt = Some((PwFor::Queue(all, mode), String::new()));
                    }
                    Err(e) => {
                        app.close_when_done = false;
                        app.error(format!("{}: {e}", arc.display()));
                    }
                }
            })
        });
    }

    fn test(&mut self) {
        let Some(arc) = self.archive.clone() else {
            self.message = Some(("RocketZip".into(), "Open an archive first.".into(), None));
            return;
        };
        let pw = self.password.clone();
        self.run_bg("Testing", move |cb| {
            let r = test_archive(&arc, pw.as_deref(), Some(cb));
            Box::new(move |app: &mut App| {
                let secs = app.finish();
                app.status = format!("Tested in {secs:.1}s");
                match r {
                    Ok((true, m)) => app.message = Some(("Test result".into(), format!("Everything is Ok\n{m}"), None)),
                    Ok((false, m)) => app.error(format!("Test FAILED\n{m}")),
                    Err(e) => app.error(e.0),
                }
            })
        });
    }

    fn start_create(&mut self, d: &CreateDlg) {
        let srcs = d.sources.clone();
        let out = PathBuf::from(d.out.trim());
        let o = CreateOptions {
            fmt: if d.fmt == BEST { None } else { Some(d.fmt.clone()) },
            level: d.level,
            method: Some(d.method.clone()),
            password: if d.pw_ok() && !d.password.is_empty() { Some(d.password.clone()) } else { None },
            store_compressed: true,
            dict_mb: DICTS[d.dict].1,
            docs: DOCS[d.docs].1.into(),
            dedup: d.dedup,
            engine: if d.max { "max".into() } else { "lzma".into() },
        };
        let best = d.fmt == BEST;
        self.run_bg("Compressing", move |cb| {
            let r = if best {
                create_best(&out, &srcs, &o, Some(cb)).map(|(p, _)| (p, None))
            } else {
                create_archive(&out, &srcs, &o, Some(cb)).map(|st| (out.clone(), st))
            };
            let pw = o.password.clone();
            Box::new(move |app: &mut App| {
                let secs = app.finish();
                match r {
                    Ok((p, st)) => {
                        app.load(p, pw);
                        app.status = format!("{}  -  done in {secs:.1}s", app.status);
                        app.report = st;
                    }
                    Err(e) => app.error(e.0),
                }
            })
        });
    }

    fn formats_info(&mut self) {
        let msg = format!(
            "Create: {}\n\nOpen / extract: {}, rar,\nplus docx/xlsx/pptx/odt/epub/jar/apk\n\n\
             Ultra (.zfx) is RocketZip's maximum-compression format.\nMax engine: built in.",
            writable_formats().join(", "), writable_formats().join(", "));
        self.message = Some(("Formats".into(), msg, None));
    }

    fn about(&mut self) {
        let dir = std::env::current_exe().ok()
            .and_then(|p| p.parent().map(|d| d.display().to_string()))
            .unwrap_or_default();
        let msg = format!(
            "RocketZip {VERSION}\n\nFree and open source under the MIT license. \
             Copyright (c) 2026 The RocketZip authors.\n\n\
             RocketZip never connects to the internet and collects no data.\n\n\
             Built with open-source components (zlib, zlib-ng, xz/liblzma, zstd, bzip2, \
             UnRAR, egui and others). Their licenses are in THIRD-PARTY-NOTICES.txt in:\n{dir}");
        self.message = Some(("About RocketZip".into(), msg, None));
    }

    // ---------------------------------------------------------------- UI
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let idle = self.busy.is_none();
            ui.add_enabled_ui(idle, |ui| {
                if ui.button("📂 Open").clicked() {
                    self.open_dialog();
                }
                if ui.button("➕ Add / Create").clicked() {
                    self.create = Some(CreateDlg::new());
                }
                if ui.button("📤 Extract").clicked() {
                    self.extract();
                }
                if ui.button("✔ Test").clicked() {
                    self.test();
                }
                if ui.button("ℹ Formats").clicked() {
                    self.formats_info();
                }
                if ui.button("About").clicked() {
                    self.about();
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("🔍 filter").desired_width(180.0));
            });
        });
        let label = match &self.archive {
            Some(a) => format!("{}   [{}]", a.display(), self.fmt),
            None => "No archive open".into(),
        };
        ui.label(egui::RichText::new(label).weak());
    }

    fn table(&mut self, ui: &mut egui::Ui) {
        use egui_extras::{Column, TableBuilder};
        let f = self.filter.to_lowercase();
        let rows: Vec<&Entry> = self.entries.iter().filter(|e| f.is_empty() || e.name.to_lowercase().contains(&f)).collect();
        if rows.is_empty() {
            ui.centered_and_justified(|ui| {
                let msg = if let Some((label, _)) = &self.busy { format!("{label}...") }
                    else if self.archive.is_some() { "No matching files".into() }
                    else { "Drop an archive here to open it,\nor files to compress them".into() };
                ui.label(egui::RichText::new(msg)
                         .size(16.0).weak());
            });
            return;
        }
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .column(Column::remainder().at_least(250.0).clip(true))
            .column(Column::auto().at_least(80.0))
            .column(Column::auto().at_least(80.0))
            .column(Column::auto().at_least(130.0))
            .header(22.0, |mut h| {
                for t in ["Name", "Size", "Packed", "Modified"] {
                    h.col(|ui| { ui.strong(t); });
                }
            })
            .body(|body| {
                body.rows(20.0, rows.len(), |mut row| {
                    let e = rows[row.index()];
                    row.col(|ui| { ui.label(format!("{} {}", if e.is_dir { "📁" } else { "📄" }, e.name)); });
                    row.col(|ui| { if !e.is_dir { ui.label(human(e.size)); } });
                    row.col(|ui| { if !e.is_dir && e.packed.is_some() { ui.label(human(e.packed)); } });
                    row.col(|ui| {
                        if let Some(t) = e.mtime.and_then(|m| chrono::DateTime::from_timestamp(m as i64, 0)) {
                            ui.label(t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string());
                        }
                    });
                });
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        if let Some((label, t)) = &self.busy {
            let p = self.prog.lock().map(|p| p.clone()).unwrap_or_default();
            let frac = if p.total > 0 { (p.done as f32 / p.total as f32).min(1.0) } else { 0.0 };
            let text = if p.total > 0 {
                format!("{label}  {:.0}%  {} / {}  {}", frac * 100.0, human(Some(p.done)), human(Some(p.total)), p.name)
            } else {
                format!("{label}  {}  {}", human(Some(p.done)), p.name)
            };
            ui.add(egui::ProgressBar::new(frac).text(text).animate(p.total == 0));
            ui.label(egui::RichText::new(format!("{:.0}s", t.elapsed().as_secs_f64())).weak());
        } else {
            ui.label(&self.status);
        }
    }

    fn create_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.create.take() else { return };
        let mut open = true;
        let mut start = false;
        egui::Window::new("Add to archive").open(&mut open).resizable(true).default_width(560.0).show(ctx, |ui| {
            ui.label("Files and folders to add:");
            egui::Frame::group(ui.style()).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(120.0).auto_shrink([false, true]).show(ui, |ui| {
                    for (i, s) in d.sources.iter().enumerate() {
                        if ui.selectable_label(d.selected == Some(i), s.display().to_string()).clicked() {
                            d.selected = Some(i);
                        }
                    }
                    if d.sources.is_empty() {
                        ui.weak("(none yet - add files or folders, or drop them on the window)");
                    }
                });
            });
            ui.horizontal(|ui| {
                if ui.button("Add files…").clicked() {
                    for p in rfd::FileDialog::new().pick_files().unwrap_or_default() {
                        d.add(p);
                    }
                }
                if ui.button("Add folder…").clicked()
                    && let Some(p) = rfd::FileDialog::new().pick_folder() {
                        d.add(p);
                    }
                if ui.add_enabled(d.selected.is_some(), egui::Button::new("Remove")).clicked()
                    && let Some(i) = d.selected.take() {
                        d.sources.remove(i);
                    }
            });
            ui.separator();
            egui::Grid::new("opts").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Archive:");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.out).desired_width(380.0));
                    if ui.button("…").clicked()
                        && let Some(p) = rfd::FileDialog::new().save_file() {
                            d.out = p.to_string_lossy().into_owned();
                            d.sync_ext();
                        }
                });
                ui.end_row();

                ui.label("Format:");
                let before = d.fmt.clone();
                egui::ComboBox::from_id_salt("fmt").selected_text(&d.fmt).show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.fmt, BEST.to_string(), BEST);
                    for f in writable_formats() {
                        let label = if f == "zfx" { "zfx  (Ultra - smallest)".to_string() } else { f.to_string() };
                        ui.selectable_value(&mut d.fmt, f.to_string(), label);
                    }
                });
                if d.fmt != before {
                    d.sync_ext();
                }
                ui.end_row();

                ui.label("Level:");
                let names = ["Store", "Fastest", "Fast", "Fast", "Normal", "Normal", "Normal", "Maximum", "Maximum", "Ultra"];
                let lname = names[d.level as usize];
                ui.add(egui::Slider::new(&mut d.level, 0..=9).text(lname));
                ui.end_row();

                ui.label("ZIP method:");
                ui.add_enabled_ui(d.fmt == "zip", |ui| {
                    egui::ComboBox::from_id_salt("method").selected_text(&d.method).show_ui(ui, |ui| {
                        for m in ["deflate", "zstd", "xz", "bzip2", "store"] {
                            ui.selectable_value(&mut d.method, m.to_string(), m);
                        }
                    });
                });
                ui.end_row();

                ui.label("Password:");
                ui.add_enabled(d.pw_ok(), egui::TextEdit::singleline(&mut d.password).password(true).desired_width(220.0));
                ui.end_row();
            });
            ui.add_space(6.0);
            ui.add_enabled_ui(d.ultra() || d.fmt == "tar.xz" || d.fmt == "7z", |ui| {
                egui::CollapsingHeader::new("Ultra compression (.zfx)").default_open(true).show(ui, |ui| {
                    egui::Grid::new("ultra").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        ui.label("Dictionary:");
                        egui::ComboBox::from_id_salt("dict").selected_text(DICTS[d.dict].0).show_ui(ui, |ui| {
                            for (i, (l, _)) in DICTS.iter().enumerate() {
                                ui.selectable_value(&mut d.dict, i, *l);
                            }
                        });
                        ui.end_row();
                        ui.label("Documents:");
                        ui.add_enabled_ui(d.ultra(), |ui| {
                            egui::ComboBox::from_id_salt("docs").selected_text(DOCS[d.docs].0).show_ui(ui, |ui| {
                                for (i, (l, _)) in DOCS.iter().enumerate() {
                                    ui.selectable_value(&mut d.docs, i, *l);
                                }
                            });
                        });
                        ui.end_row();
                    });
                    ui.add_enabled(d.ultra(), egui::Checkbox::new(&mut d.dedup, "Store identical files once"));
                    ui.add_enabled(d.ultra(), egui::Checkbox::new(
                        &mut d.max, "Max engine - context mixing (smallest, much slower)"));
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                let ok = !d.sources.is_empty() && !d.out.trim().is_empty();
                if ui.add_enabled(ok, egui::Button::new("  OK  ")).clicked() {
                    start = true;
                }
                if !ok {
                    ui.weak("Add files and choose an archive name.");
                }
            });
        });
        if start {
            self.start_create(&d);
        } else if open {
            self.create = Some(d);
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some((title, body, folder)) = self.message.clone() {
            let mut close = false;
            egui::Window::new(title).collapsible(false).resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                    ui.label(&body);
                    ui.horizontal(|ui| {
                        if let Some(f) = &folder
                            && ui.button("Open folder").clicked() {
                                open_folder(f);
                                close = true;
                            }
                        if ui.button("OK").clicked() {
                            close = true;
                        }
                    });
                });
            if close {
                self.message = None;
            }
        }
        if let Some((what, mut text)) = self.pw_prompt.take() {
            let mut done = None;
            egui::Window::new("Password").collapsible(false).resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                    ui.label("This archive is encrypted. Password:");
                    let r = ui.add(egui::TextEdit::singleline(&mut text).password(true));
                    r.request_focus();
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            done = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            done = Some(false);
                        }
                    });
                });
            match done {
                Some(true) => match what {
                    PwFor::Open(p) => self.load(p, Some(text)),
                    PwFor::Queue(paths, mode) => self.extract_queue(paths, mode, Some(text)),
                    PwFor::Extract(a, d) => {
                        self.password = Some(text.clone());
                        self.extract_to(a, d, Some(text));
                    }
                },
                Some(false) => {
                    if let PwFor::Queue(mut paths, mode) = what {
                        paths.remove(0);
                        self.extract_queue(paths, mode, None);
                    }
                }
                None => self.pw_prompt = Some((what, text)),
            }
        }
        if let Some(st) = self.report.clone() {
            let mut close = false;
            egui::Window::new("Ultra compression result").collapsible(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                let ratio = if st.input > 0 { st.output as f64 * 100.0 / st.input as f64 } else { 0.0 };
                ui.heading(format!("{}  →  {}   ({ratio:.1}% of original)", human(Some(st.input)), human(Some(st.output))));
                if st.docs > 0 {
                    ui.label(format!("Documents recompressed: {} ({} byte-exact)", st.docs, st.docs_exact));
                }
                if st.duplicates > 0 {
                    ui.label(format!("Duplicates stored once: {} (saved {})", st.duplicates, human(Some(st.dup_bytes))));
                }
                egui::Grid::new("groups").striped(true).num_columns(5).show(ui, |ui| {
                    for h in ["Group", "Filter", "Files", "In", "Out"] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for g in &st.groups {
                        ui.label(&g.kind);
                        ui.label(&g.filter);
                        ui.label(g.files.to_string());
                        ui.label(human(Some(g.input)));
                        ui.label(format!("{} ({:.1}%)", human(Some(g.output)),
                                         if g.input > 0 { g.output as f64 * 100.0 / g.input as f64 } else { 0.0 }));
                        ui.end_row();
                    }
                });
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
            if close {
                self.report = None;
            }
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if dropped.is_empty() || self.busy.is_some() {
            return;
        }
        if let Some(d) = self.create.as_mut() {
            for p in dropped {
                d.add(p);
            }
        } else if dropped.len() == 1 && dropped[0].is_file() && detect_format(&dropped[0]).is_ok() {
            self.load(dropped[0].clone(), None);
        } else {
            let mut d = CreateDlg::new();
            for p in dropped {
                d.add(p);
            }
            self.create = Some(d);
        }
    }
}

fn open_folder(p: &Path) {
    let cmd = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(cmd).arg(p).spawn();
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        while let Ok(apply) = self.rx.try_recv() {
            apply(self);
        }
        if self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.handle_drops(&ctx);
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            self.toolbar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(3.0);
            self.status_bar(ui);
            ui.add_space(3.0);
        });
        egui::CentralPanel::default().show(ui, |ui| self.table(ui));
        self.create_window(&ctx);
        self.dialogs(&ctx);
        if self.busy.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

fn parse_launch(args: &[String]) -> (String, Vec<PathBuf>) {
    let args: Vec<&String> = args.iter().filter(|a| !a.trim().is_empty()).collect();
    match args.first().map(|s| s.as_str()) {
        None => ("none".into(), vec![]),
        Some(m @ ("--add" | "--extract-here" | "--extract-to")) => {
            let paths: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();
            if paths.is_empty() { ("none".into(), vec![]) } else { (m[2..].to_string(), paths) }
        }
        Some(p) => ("open".into(), vec![PathBuf::from(p)]),
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mode, paths) = parse_launch(&args);
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/rocketzip.png")).ok();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([960.0, 600.0])
        .with_min_inner_size([640.0, 400.0])
        .with_drag_and_drop(true)
        .with_title(format!("RocketZip {}", VERSION));
    if let Some(i) = icon {
        viewport = viewport.with_icon(std::sync::Arc::new(i));
    }
    let opts = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("RocketZip", opts, Box::new(move |_cc| Ok(Box::new(App::new(&mode, paths)))))
}
