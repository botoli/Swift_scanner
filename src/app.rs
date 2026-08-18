use crate::filtering::{compare_file_entries, default_sort_descending, file_matches};
use crate::games::MiniGames;
use crate::index::FileIndex;
use crate::model::*;
use crate::scanner::{scan_directory, scan_engine_label, scan_target_capacity};
use crate::ui::*;
use crate::visual_index::{build_image_index, is_supported_image, run_image_search};
use eframe::egui::{self, Color32, RichText, ScrollArea};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 780.0])
            .with_min_inner_size([1040.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SwiftScan — поиск файлов",
        options,
        Box::new(|cc| Ok(Box::new(ScannerApp::new(cc)))),
    )
}

struct ScannerApp {
    root: PathBuf,
    files: Arc<Vec<FileEntry>>,
    visible: Vec<usize>,
    scan_preview_all: Vec<usize>,
    scan_preview_cleanup: Vec<usize>,
    file_index: Option<FileIndex>,
    index_receiver: Option<Receiver<FileIndex>>,
    query: String,
    min_mb: String,
    max_mb: String,
    view: ViewMode,
    sort: SortRule,
    loaded_rows: usize,
    has_more_rows: bool,
    list_revision: u64,
    selected_rows: HashSet<usize>,
    selection_anchor: Option<usize>,
    scan_receiver: Option<Receiver<ScanMessage>>,
    scan_cancel: Option<Arc<AtomicBool>>,
    scanning: bool,
    stats: ScanStats,
    scan_total_bytes: Option<u64>,
    analysis: AnalysisStats,
    started_at: Option<Instant>,
    elapsed: Duration,
    notice: Option<(String, bool, Instant)>,
    pending_bulk_delete: Option<Vec<usize>>,
    image_index_receiver: Option<Receiver<ImageIndexMessage>>,
    image_index_cancel: Option<Arc<AtomicBool>>,
    image_engine: Option<Arc<dyn ImageSearchEngine>>,
    image_indexing: bool,
    image_index_started: Option<Instant>,
    image_index_stats: ImageIndexStats,
    image_accelerator: String,
    image_search_receiver: Option<Receiver<ImageSearchMessage>>,
    image_search_cancel: Option<Arc<AtomicBool>>,
    image_searching: bool,
    image_query_path: Option<PathBuf>,
    image_search_mode: ImageSearchMode,
    image_hits: Vec<ImageSearchHit>,
    image_query_texture: Option<egui::TextureHandle>,
    image_result_textures: HashMap<usize, egui::TextureHandle>,
    image_search_open: bool,
    games: MiniGames,
    games_open: bool,
}

impl ScannerApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("C:\\"));
        let scan_total_bytes = scan_target_capacity(&root);
        Self {
            root,
            files: Arc::new(Vec::new()),
            visible: Vec::new(),
            scan_preview_all: Vec::new(),
            scan_preview_cleanup: Vec::new(),
            file_index: None,
            index_receiver: None,
            query: String::new(),
            min_mb: String::new(),
            max_mb: String::new(),
            view: ViewMode::All,
            sort: SortRule {
                mode: SortMode::Size,
                descending: true,
            },
            loaded_rows: crate::PAGE_SIZE,
            has_more_rows: false,
            list_revision: 0,
            selected_rows: HashSet::new(),
            selection_anchor: None,
            scan_receiver: None,
            scan_cancel: None,
            scanning: false,
            stats: ScanStats::default(),
            scan_total_bytes,
            analysis: AnalysisStats::default(),
            started_at: None,
            elapsed: Duration::ZERO,
            notice: None,
            pending_bulk_delete: None,
            image_index_receiver: None,
            image_index_cancel: None,
            image_engine: None,
            image_indexing: false,
            image_index_started: None,
            image_index_stats: ImageIndexStats::default(),
            image_accelerator: "Ожидание сканирования".to_owned(),
            image_search_receiver: None,
            image_search_cancel: None,
            image_searching: false,
            image_query_path: None,
            image_search_mode: ImageSearchMode::Semantic,
            image_hits: Vec::new(),
            image_query_texture: None,
            image_result_textures: HashMap::new(),
            image_search_open: false,
            games: MiniGames::new(),
            games_open: false,
        }
    }

    fn choose_folder(&mut self) {
        if let Some(folder) = rfd::FileDialog::new()
            .set_directory(&self.root)
            .pick_folder()
        {
            self.root = folder;
            self.scan_total_bytes = scan_target_capacity(&self.root);
        }
    }

    fn start_scan(&mut self) {
        self.cancel_jobs();
        self.files = Arc::new(Vec::new());
        self.visible.clear();
        self.scan_preview_all.clear();
        self.scan_preview_cleanup.clear();
        self.selected_rows.clear();
        self.selection_anchor = None;
        self.pending_bulk_delete = None;
        self.file_index = None;
        self.index_receiver = None;
        self.image_engine = None;
        self.image_index_receiver = None;
        self.image_indexing = false;
        self.image_index_started = None;
        self.image_index_stats = ImageIndexStats::default();
        self.image_accelerator = "Ожидание индекса файлов".to_owned();
        self.image_hits.clear();
        self.image_query_texture = None;
        self.image_result_textures.clear();
        self.stats = ScanStats::default();
        self.analysis = AnalysisStats::default();
        self.loaded_rows = crate::PAGE_SIZE;
        self.has_more_rows = false;
        self.elapsed = Duration::ZERO;
        self.notice = None;
        self.scan_total_bytes = scan_target_capacity(&self.root);
        self.started_at = Some(Instant::now());
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let root = self.root.clone();
        thread::spawn(move || scan_directory(root, sender, worker_cancelled));
        self.scan_receiver = Some(receiver);
        self.scan_cancel = Some(cancelled);
        self.scanning = true;
    }

    fn cancel_jobs(&mut self) {
        if let Some(cancel) = &self.scan_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(cancel) = &self.image_index_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(cancel) = &self.image_search_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.scanning = false;
        self.image_indexing = false;
        self.image_searching = false;
    }

    fn poll_scan(&mut self) {
        let Some(receiver) = self.scan_receiver.take() else {
            return;
        };
        let mut finished = false;
        let mut preview_changed = false;
        while let Ok(message) = receiver.try_recv() {
            match message {
                ScanMessage::Batch(mut batch, stats) => {
                    self.stats = stats;
                    for file in &batch {
                        if file.is_cleanup_candidate() {
                            self.analysis.cleanup_files += 1;
                            self.analysis.cleanup_bytes =
                                self.analysis.cleanup_bytes.saturating_add(file.size);
                        }
                    }
                    let first_index = self.files.len();
                    Arc::get_mut(&mut self.files)
                        .expect("scan owns the file collection")
                        .append(&mut batch);
                    self.update_scan_preview(first_index, self.files.len());
                    preview_changed = true;
                }
                ScanMessage::Finished(stats, cancelled) => {
                    self.stats = stats;
                    self.scanning = false;
                    self.elapsed = self
                        .started_at
                        .take()
                        .map_or(self.elapsed, |start| start.elapsed());
                    finished = true;
                    self.refresh_visible();
                    if !cancelled {
                        self.notice = Some((
                            format!(
                                "Сканирование завершено: {} файлов, {}",
                                format_count(self.stats.files),
                                format_bytes(self.stats.bytes)
                            ),
                            false,
                            Instant::now(),
                        ));
                        self.start_indexing();
                    }
                }
            }
        }
        if !finished {
            self.scan_receiver = Some(receiver);
        } else {
            self.scan_cancel = None;
        }
        if preview_changed && !finished {
            self.load_visible_page();
        }
    }

    fn update_scan_preview(&mut self, first_index: usize, end_index: usize) {
        self.scan_preview_all.extend(first_index..end_index);
        self.scan_preview_all
            .sort_unstable_by_key(|index| std::cmp::Reverse(self.files[*index].size));
        self.scan_preview_all.truncate(crate::PAGE_SIZE);

        self.scan_preview_cleanup.extend(
            (first_index..end_index).filter(|index| self.files[*index].is_cleanup_candidate()),
        );
        self.scan_preview_cleanup
            .sort_unstable_by_key(|index| std::cmp::Reverse(self.files[*index].size));
        self.scan_preview_cleanup.truncate(crate::PAGE_SIZE);
    }

    fn start_indexing(&mut self) {
        let files = Arc::clone(&self.files);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(FileIndex::build(&files));
        });
        self.index_receiver = Some(receiver);
    }

    fn poll_index(&mut self) {
        let Some(receiver) = self.index_receiver.take() else {
            return;
        };
        match receiver.try_recv() {
            Ok(index) => {
                self.file_index = Some(index);
                self.refresh_visible();
                self.start_image_indexing();
            }
            Err(mpsc::TryRecvError::Empty) => self.index_receiver = Some(receiver),
            Err(mpsc::TryRecvError::Disconnected) => {}
        }
    }

    /// Starts the incremental visual index after the regular file index is ready.
    ///
    /// It receives the existing scan result and root path, and returns progress
    /// plus a shared search engine through `ImageIndexMessage`. No filesystem
    /// traversal or image decoding occurs on the UI thread.
    fn start_image_indexing(&mut self) {
        if let Some(cancel) = &self.image_index_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        let files = Arc::clone(&self.files);
        let root = self.root.clone();
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        thread::spawn(move || build_image_index(files, root, sender, worker_cancelled));
        self.image_index_receiver = Some(receiver);
        self.image_index_cancel = Some(cancelled);
        self.image_indexing = true;
        self.image_index_started = Some(Instant::now());
        self.image_engine = None;
        self.image_index_stats = ImageIndexStats::default();
        self.image_accelerator = "Загрузка MobileCLIP2-S0".to_owned();
    }

    fn poll_image_index(&mut self) {
        let Some(receiver) = self.image_index_receiver.take() else {
            return;
        };
        let mut keep_receiver = true;
        while let Ok(message) = receiver.try_recv() {
            match message {
                ImageIndexMessage::Progress(stats, accelerator) => {
                    self.image_index_stats = stats;
                    self.image_accelerator = accelerator;
                }
                ImageIndexMessage::Ready(engine, stats) => {
                    self.image_index_stats = stats;
                    self.image_accelerator = engine.accelerator();
                    self.image_engine = Some(engine);
                    self.image_indexing = false;
                    self.image_index_started = None;
                    self.image_index_cancel = None;
                    keep_receiver = false;
                    if self.image_query_path.is_some() {
                        self.start_image_search();
                    }
                }
                ImageIndexMessage::Failed(error) => {
                    self.image_indexing = false;
                    self.image_index_started = None;
                    self.image_index_cancel = None;
                    self.image_accelerator = "Индекс недоступен".to_owned();
                    self.notice = Some((error, true, Instant::now()));
                    keep_receiver = false;
                }
                ImageIndexMessage::Cancelled => {
                    self.image_indexing = false;
                    self.image_index_started = None;
                    self.image_index_cancel = None;
                    keep_receiver = false;
                }
            }
        }
        if keep_receiver {
            self.image_index_receiver = Some(receiver);
        }
    }

    fn choose_reference_image(&mut self) {
        let mut dialog =
            rfd::FileDialog::new().add_filter("Изображения", &["jpg", "jpeg", "png", "webp"]);
        if self.root.is_dir() {
            dialog = dialog.set_directory(&self.root);
        }
        if let Some(path) = dialog.pick_file() {
            self.image_query_path = Some(path);
            self.start_image_search();
        }
    }

    /// Accepts an image dropped onto the visual-search screen.
    ///
    /// `eframe::App::update` calls this once per frame with the egui context.
    /// It reads only already-delivered drop metadata, stores a supported local
    /// path, and starts the same asynchronous query used by the file dialog.
    fn handle_dropped_reference(&mut self, ctx: &egui::Context) {
        if !self.image_search_open {
            return;
        }
        let dropped_path = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .rev()
                .filter_map(|file| file.path.clone())
                .find(|path| is_supported_image(path))
        });
        if let Some(path) = dropped_path {
            self.image_query_path = Some(path);
            self.start_image_search();
        }
    }

    /// Starts a ranked image query and lazy thumbnail stream.
    ///
    /// It is called after choosing a reference image, changing the search mode,
    /// or completing a new visual index. It takes no arguments because the
    /// current query state belongs to `ScannerApp`; results arrive asynchronously.
    fn start_image_search(&mut self) {
        if let Some(cancel) = &self.image_search_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.image_search_receiver = None;
        self.image_search_cancel = None;
        self.image_searching = false;
        self.image_hits.clear();
        self.image_query_texture = None;
        self.image_result_textures.clear();

        let (Some(engine), Some(path)) = (&self.image_engine, &self.image_query_path) else {
            return;
        };
        let request = ImageSearchRequest {
            path: path.clone(),
            mode: self.image_search_mode,
            limit: 100,
        };
        let engine = Arc::clone(engine);
        let files = Arc::clone(&self.files);
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        thread::spawn(move || run_image_search(engine, files, request, sender, worker_cancelled));
        self.image_search_receiver = Some(receiver);
        self.image_search_cancel = Some(cancelled);
        self.image_searching = true;
    }

    fn poll_image_search(&mut self, ctx: &egui::Context) {
        let Some(receiver) = self.image_search_receiver.take() else {
            return;
        };
        let mut keep_receiver = true;
        while let Ok(message) = receiver.try_recv() {
            match message {
                ImageSearchMessage::Results(hits, preview) => {
                    self.image_hits = hits;
                    if let Some(engine) = &self.image_engine {
                        self.image_accelerator = engine.accelerator();
                    }
                    if let Some(preview) = preview {
                        self.image_query_texture =
                            Some(Self::make_texture(ctx, "visual-query", preview));
                    }
                }
                ImageSearchMessage::Thumbnail(file_index, image) => {
                    let texture =
                        Self::make_texture(ctx, &format!("visual-result-{file_index}"), image);
                    self.image_result_textures.insert(file_index, texture);
                }
                ImageSearchMessage::Finished => {
                    self.image_searching = false;
                    self.image_search_cancel = None;
                    keep_receiver = false;
                }
                ImageSearchMessage::Failed(error) => {
                    self.image_searching = false;
                    self.image_search_cancel = None;
                    self.notice = Some((error, true, Instant::now()));
                    keep_receiver = false;
                }
                ImageSearchMessage::Cancelled => {
                    self.image_searching = false;
                    self.image_search_cancel = None;
                    keep_receiver = false;
                }
            }
        }
        if keep_receiver {
            self.image_search_receiver = Some(receiver);
        }
    }

    fn make_texture(ctx: &egui::Context, name: &str, image: PixelImage) -> egui::TextureHandle {
        ctx.load_texture(
            name,
            egui::ColorImage::from_rgba_unmultiplied([image.width, image.height], &image.rgba),
            egui::TextureOptions::LINEAR,
        )
    }

    fn size_bounds(&self) -> (u64, u64) {
        let min = parse_megabytes(&self.min_mb).unwrap_or(0);
        let max = parse_megabytes(&self.max_mb).unwrap_or(u64::MAX);
        (min.min(max), max.max(min))
    }

    fn refresh_visible(&mut self) {
        self.loaded_rows = crate::PAGE_SIZE;
        self.list_revision = self.list_revision.wrapping_add(1);
        self.load_visible_page();
    }

    fn load_visible_page(&mut self) {
        let bounds = self.size_bounds();
        let (visible, has_more) = if let Some(index) = &self.file_index {
            index.query_page(
                &self.files,
                &self.query,
                bounds,
                self.view,
                self.sort,
                self.loaded_rows,
            )
        } else {
            let source = match self.view {
                ViewMode::All => &self.scan_preview_all,
                ViewMode::Cleanup => &self.scan_preview_cleanup,
            };
            let normalized = self.query.trim().to_lowercase();
            let mut visible = source
                .iter()
                .copied()
                .filter(|index| file_matches(&self.files[*index], &normalized, bounds, self.view))
                .collect::<Vec<_>>();
            visible.sort_unstable_by(|left, right| {
                compare_file_entries(&self.files[*left], &self.files[*right], &[self.sort])
            });
            visible.truncate(self.loaded_rows);
            (visible, false)
        };
        self.visible = visible;
        self.has_more_rows = has_more;
    }

    fn set_sort(&mut self, mode: SortMode) {
        if self.sort.mode == mode {
            self.sort.descending = !self.sort.descending;
        } else {
            self.sort = SortRule {
                mode,
                descending: default_sort_descending(mode),
            };
        }
        self.refresh_visible();
    }

    fn select_row(&mut self, index: usize, extend: bool) {
        if extend
            && let Some(anchor) = self.selection_anchor
            && let (Some(anchor_position), Some(clicked_position)) = (
                self.visible.iter().position(|row| *row == anchor),
                self.visible.iter().position(|row| *row == index),
            )
        {
            let start = anchor_position.min(clicked_position);
            let end = anchor_position.max(clicked_position);
            self.selected_rows.clear();
            self.selected_rows
                .extend(self.visible[start..=end].iter().copied());
            return;
        }

        self.selected_rows.clear();
        self.selected_rows.insert(index);
        self.selection_anchor = Some(index);
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .exact_width(252.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(13, 19, 27))
                    .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("SPACE/INDEX")
                            .size(10.0)
                            .strong()
                            .color(crate::BLUE),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("LOCAL 0.3").size(9.0).color(crate::MUTED));
                    });
                });
                ui.add_space(22.0);
                ui.label(
                    RichText::new("ОЧИСТКА ДИСКА")
                        .size(21.0)
                        .strong()
                        .color(crate::INK),
                );
                ui.add_space(9.0);
                ui.label(
                    RichText::new("Поиск крупных файлов и мусора\nбез автоматического удаления.")
                        .size(12.0)
                        .color(crate::MUTED),
                );
                ui.add_space(22.0);
                ui.separator();
                ui.add_space(16.0);
                ui.label(
                    RichText::new("ОБЛАСТЬ СКАНИРОВАНИЯ")
                        .size(9.0)
                        .strong()
                        .color(crate::MUTED),
                );
                ui.add_space(8.0);
                egui::Frame::new()
                    .fill(crate::SURFACE)
                    .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_min_width(194.0);
                        ui.label(
                            RichText::new(short_path(&self.root))
                                .size(11.0)
                                .color(crate::INK),
                        );
                        ui.add_space(9.0);
                        if ui
                            .button(RichText::new("ИЗМЕНИТЬ ПУТЬ  →").size(10.0))
                            .clicked()
                        {
                            self.choose_folder();
                        }
                    });
                ui.add_space(20.0);
                ui.label(
                    RichText::new("РАЗМЕР ФАЙЛОВ")
                        .size(9.0)
                        .strong()
                        .color(crate::MUTED),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(self.min_mb.is_empty(), RichText::new("Все").size(11.0))
                        .clicked()
                    {
                        self.min_mb.clear();
                        self.refresh_visible();
                    }
                    if ui
                        .selectable_label(
                            self.min_mb == "100",
                            RichText::new("от 100 МБ").size(11.0),
                        )
                        .clicked()
                    {
                        self.min_mb = "100".to_owned();
                        self.refresh_visible();
                    }
                    if ui
                        .selectable_label(
                            self.min_mb == "1024",
                            RichText::new("от 1 ГБ").size(11.0),
                        )
                        .clicked()
                    {
                        self.min_mb = "1024".to_owned();
                        self.refresh_visible();
                    }
                });
                ui.add_space(12.0);
                ui.label(
                    RichText::new("ТОЧНЫЙ ДИАПАЗОН / МБ")
                        .size(9.0)
                        .color(crate::MUTED),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let min_changed = ui
                        .add_sized(
                            [93.0, 29.0],
                            egui::TextEdit::singleline(&mut self.min_mb).hint_text("От"),
                        )
                        .changed();
                    ui.label(RichText::new("—").color(crate::MUTED));
                    let max_changed = ui
                        .add_sized(
                            [93.0, 29.0],
                            egui::TextEdit::singleline(&mut self.max_mb).hint_text("До"),
                        )
                        .changed();
                    if min_changed || max_changed {
                        self.refresh_visible();
                    }
                });
                ui.add_space(20.0);
                ui.label(
                    RichText::new("ПРОСМОТР")
                        .size(9.0)
                        .strong()
                        .color(crate::MUTED),
                );
                ui.add_space(8.0);
                self.mode_tab(
                    ui,
                    ViewMode::All,
                    "Все файлы".to_owned(),
                    format!("{} объектов", format_count(self.stats.files)),
                );
                ui.add_space(6.0);
                self.mode_tab(
                    ui,
                    ViewMode::Cleanup,
                    "Умная очистка".to_owned(),
                    format!(
                        "{} · {}",
                        format_count(self.analysis.cleanup_files),
                        format_bytes(self.analysis.cleanup_bytes)
                    ),
                );
                ui.add_space(6.0);
                self.image_search_tab(ui);

                ui.add_space(14.0);
                ui.label(
                    RichText::new("РАЗВЛЕЧЕНИЯ")
                        .size(8.0)
                        .strong()
                        .color(crate::MUTED),
                );
                ui.add_space(5.0);
                let games_button =
                    egui::Button::new(RichText::new("МИНИ-ИГРЫ  /  12").size(11.0).strong().color(
                        if self.games_open {
                            Color32::WHITE
                        } else {
                            crate::INK
                        },
                    ))
                    .fill(if self.games_open {
                        Color32::from_rgb(27, 49, 76)
                    } else {
                        crate::SURFACE
                    })
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        if self.games_open {
                            crate::BLUE
                        } else {
                            crate::LINE
                        },
                    ))
                    .min_size(egui::Vec2::new(ui.available_width(), 38.0));
                if ui.add(games_button).clicked() {
                    self.games_open = true;
                    self.image_search_open = false;
                }

                let spacer = (ui.available_height() - 64.0).max(14.0);
                ui.add_space(spacer);
                let label = if self.scanning {
                    "Остановить сканирование"
                } else {
                    "Сканировать папку"
                };
                let button = egui::Button::new(
                    RichText::new(label)
                        .size(13.0)
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(crate::BLUE)
                .min_size(egui::Vec2::new(ui.available_width(), 50.0));
                if ui.add(button).clicked() {
                    if self.scanning {
                        self.cancel_jobs();
                    } else {
                        self.start_scan();
                    }
                }
            });
    }

    fn stat_cell(ui: &mut egui::Ui, label: &str, value: String, color: Color32) {
        ui.vertical(|ui| {
            ui.label(RichText::new(label).size(8.0).strong().color(color));
            ui.label(RichText::new(value).size(14.0).color(crate::INK));
        });
    }

    fn mode_tab(&mut self, ui: &mut egui::Ui, mode: ViewMode, primary: String, secondary: String) {
        let selected = self.view == mode && !self.games_open;
        let frame = egui::Frame::new()
            .fill(if selected {
                Color32::from_rgb(27, 49, 76)
            } else {
                crate::SURFACE
            })
            .stroke(egui::Stroke::new(
                1.0_f32,
                if selected { crate::BLUE } else { crate::LINE },
            ))
            .inner_margin(egui::Margin::symmetric(12, 10));
        let response = frame
            .show(ui, |ui| {
                ui.set_min_width(192.0);
                ui.horizontal(|ui| {
                    let (bar_rect, _) =
                        ui.allocate_exact_size(egui::Vec2::new(3.0, 32.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        bar_rect,
                        1.0,
                        if selected { crate::BLUE } else { crate::LINE },
                    );
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(primary)
                                .size(12.0)
                                .strong()
                                .color(if selected { crate::INK } else { crate::MUTED }),
                        );
                        ui.label(RichText::new(secondary).size(10.0).color(crate::MUTED));
                    });
                });
            })
            .response;
        if response.interact(egui::Sense::click()).clicked() {
            self.view = mode;
            self.games_open = false;
            self.image_search_open = false;
            self.refresh_visible();
        }
    }

    fn image_search_tab(&mut self, ui: &mut egui::Ui) {
        let selected = self.image_search_open && !self.games_open;
        let indexed = self.image_engine.as_ref().map_or(
            self.image_index_stats.cached + self.image_index_stats.processed,
            |engine| engine.image_count() as u64,
        );
        let secondary = if self.image_indexing {
            format!(
                "индексация {} / {}",
                format_count(
                    self.image_index_stats.processed
                        + self.image_index_stats.cached
                        + self.image_index_stats.failed
                ),
                format_count(self.image_index_stats.total)
            )
        } else {
            format!("{} изображений", format_count(indexed))
        };
        let frame = egui::Frame::new()
            .fill(if selected {
                Color32::from_rgb(27, 49, 76)
            } else {
                crate::SURFACE
            })
            .stroke(egui::Stroke::new(
                1.0_f32,
                if selected { crate::BLUE } else { crate::LINE },
            ))
            .inner_margin(egui::Margin::symmetric(12, 10));
        let response = frame
            .show(ui, |ui| {
                ui.set_min_width(192.0);
                ui.horizontal(|ui| {
                    let (lens_rect, _) =
                        ui.allocate_exact_size(egui::Vec2::new(30.0, 30.0), egui::Sense::hover());
                    let painter = ui.painter();
                    painter.circle_stroke(
                        lens_rect.center() - egui::vec2(2.0, 2.0),
                        8.0,
                        egui::Stroke::new(
                            1.5_f32,
                            if selected { crate::BLUE } else { crate::MUTED },
                        ),
                    );
                    painter.line_segment(
                        [
                            lens_rect.center() + egui::vec2(4.0, 4.0),
                            lens_rect.center() + egui::vec2(10.0, 10.0),
                        ],
                        egui::Stroke::new(
                            1.5_f32,
                            if selected { crate::BLUE } else { crate::MUTED },
                        ),
                    );
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Похожие фото")
                                .size(12.0)
                                .strong()
                                .color(if selected { crate::INK } else { crate::MUTED }),
                        );
                        ui.label(RichText::new(secondary).size(10.0).color(crate::MUTED));
                    });
                });
            })
            .response;
        if response.interact(egui::Sense::click()).clicked() {
            self.image_search_open = true;
            self.games_open = false;
        }
    }

    fn header_cell(&mut self, ui: &mut egui::Ui, label: &str, width: f32, sort: Option<SortMode>) {
        let active = sort.is_some_and(|mode| mode == self.sort.mode);
        let response = ui.add_sized(
            [width, 28.0],
            egui::Button::new(RichText::new(label).size(8.0).color(crate::MUTED)).frame(false),
        );
        if active {
            let center = response.rect.right_center() - egui::vec2(8.0, 0.0);
            let points = if self.sort.descending {
                vec![
                    center + egui::vec2(-3.5, -2.0),
                    center + egui::vec2(3.5, -2.0),
                    center + egui::vec2(0.0, 2.5),
                ]
            } else {
                vec![
                    center + egui::vec2(0.0, -2.5),
                    center + egui::vec2(3.5, 2.0),
                    center + egui::vec2(-3.5, 2.0),
                ]
            };
            ui.painter().add(egui::Shape::convex_polygon(
                points,
                crate::BLUE,
                egui::Stroke::NONE,
            ));
        }
        if let Some(mode) = sort {
            if response.clicked() {
                self.set_sort(mode);
            }
        }
    }

    fn row_context_menu(&mut self, ui: &mut egui::Ui, index: usize) {
        let Some(file) = self.files.get(index) else {
            return;
        };
        let path = file.path.clone();
        let can_delete = self.file_index.is_some() && !file.deleted;

        if ui.button("Открыть файл").clicked() {
            self.notice = Some(match open_path(&path) {
                Ok(()) => ("Файл открыт".to_owned(), false, Instant::now()),
                Err(error) => (error, true, Instant::now()),
            });
            ui.close_menu();
        }
        if ui.button("Показать в папке").clicked() {
            self.notice = Some(match reveal_in_file_manager(&path) {
                Ok(()) => ("Папка открыта".to_owned(), false, Instant::now()),
                Err(error) => (error, true, Instant::now()),
            });
            ui.close_menu();
        }
        if ui
            .add_enabled(can_delete, egui::Button::new("Переместить в корзину"))
            .on_disabled_hover_text("Доступно после подготовки списка")
            .clicked()
        {
            self.pending_bulk_delete = Some(vec![index]);
            ui.close_menu();
        }
    }

    fn central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(crate::CANVAS)
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new("FILE INDEX").size(17.0).color(crate::INK));
                        let status = if self.scanning {
                            "СКАНИРОВАНИЕ"
                        } else {
                            "ГОТОВО"
                        };
                        ui.label(
                            RichText::new(format!("{status} · {}", scan_engine_label()))
                                .size(8.0)
                                .color(crate::MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let changed = ui
                            .add_sized(
                                [240.0, 30.0],
                                egui::TextEdit::singleline(&mut self.query)
                                    .hint_text("Поиск по имени или пути…"),
                            )
                            .changed();
                        if changed {
                            self.refresh_visible();
                        }
                    });
                });
                ui.add_space(14.0);

                let speed = if self.elapsed.as_secs_f64() > 0.0 {
                    self.stats.files as f64 / self.elapsed.as_secs_f64()
                } else {
                    0.0
                };
                egui::Frame::new()
                    .fill(crate::SURFACE)
                    .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.columns(5, |columns| {
                            Self::stat_cell(
                                &mut columns[0],
                                "ФАЙЛОВ",
                                format_count(self.stats.files),
                                crate::BLUE,
                            );
                            Self::stat_cell(
                                &mut columns[1],
                                "ОБЪЁМ",
                                format!(
                                    "{} / {}",
                                    format_bytes(self.stats.bytes),
                                    self.scan_total_bytes
                                        .map(format_bytes)
                                        .unwrap_or_else(|| "—".to_owned())
                                ),
                                Color32::from_rgb(244, 174, 91),
                            );
                            Self::stat_cell(
                                &mut columns[2],
                                "ВРЕМЯ",
                                format_duration(self.elapsed),
                                crate::MUTED,
                            );
                            Self::stat_cell(
                                &mut columns[3],
                                "СКОРОСТЬ",
                                format!("{}/с", format_count(speed as u64)),
                                Color32::from_rgb(91, 201, 151),
                            );
                            Self::stat_cell(
                                &mut columns[4],
                                "МОЖНО ОСВОБОДИТЬ",
                                format_bytes(self.analysis.cleanup_bytes),
                                Color32::from_rgb(91, 201, 151),
                            );
                        });
                    });
                ui.add_space(12.0);

                let unfiltered = self.query.trim().is_empty()
                    && self.min_mb.trim().is_empty()
                    && self.max_mb.trim().is_empty();
                let result_count = if unfiltered {
                    format_count(match self.view {
                        ViewMode::All => self.stats.files,
                        ViewMode::Cleanup => self.analysis.cleanup_files,
                    })
                } else if self.has_more_rows {
                    format!("{}+", format_count(self.visible.len() as u64))
                } else {
                    format_count(self.visible.len() as u64)
                };
                let can_delete = self.file_index.is_some();
                egui::Frame::new()
                    .fill(crate::SURFACE)
                    .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
                    .inner_margin(egui::Margin::symmetric(10, 7))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} / {} ОБЪЕКТОВ / ПОКАЗАНО {}",
                                    self.view.label().to_uppercase(),
                                    result_count,
                                    self.visible.len()
                                ))
                                .size(9.0)
                                .color(crate::INK),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new("СТОЛБЦЫ  8/8").size(8.0).color(crate::MUTED),
                                    );
                                    if !self.selected_rows.is_empty() {
                                        let selected = self
                                            .selected_rows
                                            .iter()
                                            .copied()
                                            .filter(|index| {
                                                self.files
                                                    .get(*index)
                                                    .is_some_and(|file| !file.deleted)
                                            })
                                            .collect::<Vec<_>>();
                                        if ui
                                            .add_enabled(
                                                can_delete && !selected.is_empty(),
                                                egui::Button::new(format!(
                                                    "В КОРЗИНУ {}",
                                                    selected.len()
                                                )),
                                            )
                                            .on_disabled_hover_text(
                                                "Доступно после подготовки списка",
                                            )
                                            .clicked()
                                        {
                                            self.pending_bulk_delete = Some(selected);
                                        }
                                        ui.label(
                                            RichText::new(format!(
                                                "ВЫДЕЛЕНО  {}",
                                                self.selected_rows.len()
                                            ))
                                            .size(8.0)
                                            .strong()
                                            .color(crate::BLUE),
                                        );
                                        if ui.small_button("Снять").clicked() {
                                            self.selected_rows.clear();
                                            self.selection_anchor = None;
                                        }
                                    }
                                    if ui.small_button("Выбрать все").clicked() {
                                        self.selected_rows = self.visible.iter().copied().collect();
                                        self.selection_anchor = None;
                                    }
                                    ui.label(
                                        RichText::new("Клик: сортировка / повторный клик: порядок")
                                            .size(8.0)
                                            .color(crate::MUTED),
                                    );
                                },
                            );
                        });
                    });

                let row_indices = self
                    .visible
                    .iter()
                    .take(self.loaded_rows)
                    .copied()
                    .collect::<Vec<_>>();
                let table_width = ui.available_width();
                let fixed_width = 560.0;
                let flexible_width = (table_width - fixed_width).max(168.0);
                let name_width = flexible_width * 0.40;
                let path_width = flexible_width - name_width;
                let table_height = ui.available_height();
                let scroll_output = ScrollArea::vertical()
                    .id_salt(("file-table", self.list_revision))
                    .auto_shrink([false, false])
                    .max_height(table_height)
                    .show(ui, |ui| {
                        ui.set_min_width(table_width);
                        ui.set_min_height(table_height);
                        ui.horizontal(|ui| {
                            self.header_cell(ui, "ДАТА", 96.0, Some(SortMode::Modified));
                            self.header_cell(ui, "ТИП", 48.0, None);
                            self.header_cell(ui, "ФАЙЛ", name_width, Some(SortMode::Name));
                            self.header_cell(ui, "ПУТЬ", path_width, None);
                            self.header_cell(ui, "ОЦЕНКА", 78.0, Some(SortMode::Assessment));
                            self.header_cell(ui, "ПОЧЕМУ", 136.0, None);
                            self.header_cell(ui, "РАЗМЕР", 78.0, Some(SortMode::Size));
                            self.header_cell(ui, "ДЕЙСТВИЯ", 56.0, None);
                        });
                        ui.separator();
                        for index in row_indices {
                            let selected = self.selected_rows.contains(&index);
                            let file = &self.files[index];
                            let path = file.path.clone();
                            let name = path
                                .file_name()
                                .and_then(|value| value.to_str())
                                .unwrap_or("—")
                                .to_owned();
                            let parent = path
                                .parent()
                                .map(short_path)
                                .unwrap_or_else(|| "—".to_owned());
                            let extension = format!("[{}]", file_extension(&path));
                            let modified = format_modified(file.modified);
                            let score = file.cleanup_score();
                            let reason = file.cleanup_reason();
                            let size = format_bytes(file.size);
                            let (_, score_color) = assessment_label(score);
                            let row_rect = egui::Frame::new()
                                .fill(if selected {
                                    Color32::from_rgb(25, 43, 66)
                                } else {
                                    crate::SURFACE
                                })
                                .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
                                .inner_margin(egui::Margin::symmetric(4, 3))
                                .show(ui, |ui| {
                                    ui.set_min_height(31.0);
                                    ui.horizontal(|ui| {
                                        ui.add_sized(
                                            [96.0, 24.0],
                                            egui::Label::new(
                                                RichText::new(modified)
                                                    .monospace()
                                                    .size(9.0)
                                                    .color(crate::MUTED),
                                            ),
                                        );
                                        ui.add_sized(
                                            [48.0, 24.0],
                                            egui::Label::new(
                                                RichText::new(extension)
                                                    .monospace()
                                                    .size(9.0)
                                                    .color(crate::BLUE),
                                            ),
                                        );
                                        ui.add_sized(
                                            [name_width, 24.0],
                                            egui::Label::new(
                                                RichText::new(name).size(10.0).color(crate::INK),
                                            )
                                            .truncate(),
                                        );
                                        ui.add_sized(
                                            [path_width, 24.0],
                                            egui::Label::new(
                                                RichText::new(parent)
                                                    .monospace()
                                                    .size(9.0)
                                                    .color(crate::MUTED),
                                            )
                                            .truncate(),
                                        );
                                        ui.add_sized(
                                            [78.0, 24.0],
                                            egui::Label::new(
                                                RichText::new(format!("{score} / 100"))
                                                    .strong()
                                                    .size(9.0)
                                                    .color(score_color),
                                            ),
                                        );
                                        ui.add_sized(
                                            [136.0, 24.0],
                                            egui::Label::new(
                                                RichText::new(reason).size(9.0).color(score_color),
                                            )
                                            .truncate(),
                                        );
                                        ui.add_sized(
                                            [78.0, 24.0],
                                            egui::Label::new(
                                                RichText::new(size)
                                                    .monospace()
                                                    .strong()
                                                    .size(9.0)
                                                    .color(crate::INK),
                                            ),
                                        );
                                        ui.allocate_ui_with_layout(
                                            egui::Vec2::new(56.0, 24.0),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                ui.menu_button("•••", |ui| {
                                                    self.row_context_menu(ui, index);
                                                });
                                            },
                                        );
                                    });
                                })
                                .response
                                .rect;
                            let selection_rect = egui::Rect::from_min_max(
                                row_rect.min,
                                egui::pos2(
                                    (row_rect.max.x - 64.0).max(row_rect.min.x),
                                    row_rect.max.y,
                                ),
                            );
                            let selection_response = ui.interact(
                                selection_rect,
                                ui.id().with(("file-row", index)),
                                egui::Sense::click(),
                            );
                            let hovered = ui.input(|input| {
                                input
                                    .pointer
                                    .hover_pos()
                                    .is_some_and(|position| row_rect.contains(position))
                            });
                            if hovered && !selected {
                                ui.painter().rect_filled(
                                    row_rect,
                                    0.0,
                                    Color32::from_rgba_unmultiplied(92, 159, 255, 24),
                                );
                            }
                            if selection_response.clicked() {
                                let extend = ui.input(|input| input.modifiers.shift);
                                self.select_row(index, extend);
                            }
                        }
                    });
                let reached_bottom = scroll_output.state.offset.y > 0.0
                    && scroll_output.state.offset.y + scroll_output.inner_rect.height()
                        >= scroll_output.content_size.y - 24.0;
                if reached_bottom && self.has_more_rows {
                    self.loaded_rows = self.loaded_rows.saturating_add(crate::PAGE_SIZE);
                    self.load_visible_page();
                    ctx.request_repaint();
                }
            });
    }

    fn image_search_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(crate::CANVAS)
                    .inner_margin(egui::Margin::same(18)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new("VISUAL SEARCH").size(17.0).color(crate::INK));
                        ui.label(
                            RichText::new(format!(
                                "ЛОКАЛЬНО · MOBILECLIP2-S0 · {}",
                                self.image_accelerator.to_uppercase()
                            ))
                            .size(8.0)
                            .color(crate::MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("ВЕРНУТЬСЯ К ФАЙЛАМ").clicked() {
                            self.image_search_open = false;
                        }
                    });
                });
                ui.add_space(14.0);

                if self.image_indexing {
                    let complete = self.image_index_stats.processed
                        + self.image_index_stats.cached
                        + self.image_index_stats.failed;
                    let fraction = if self.image_index_stats.total == 0 {
                        0.0
                    } else {
                        complete as f32 / self.image_index_stats.total as f32
                    };
                    let elapsed = self
                        .image_index_started
                        .map_or(Duration::ZERO, |started| started.elapsed());
                    let speed = if elapsed.as_secs_f64() > 0.0 {
                        complete as f64 / elapsed.as_secs_f64()
                    } else {
                        0.0
                    };
                    let eta = if speed > 0.0 {
                        Duration::from_secs_f64(
                            self.image_index_stats.total.saturating_sub(complete) as f64 / speed,
                        )
                    } else {
                        Duration::ZERO
                    };
                    egui::Frame::new()
                        .fill(crate::SURFACE)
                        .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
                        .inner_margin(egui::Margin::symmetric(14, 11))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("ВИЗУАЛЬНЫЙ ИНДЕКС")
                                        .size(9.0)
                                        .strong()
                                        .color(crate::BLUE),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(format!(
                                                "{} / {} · {}/с · ETA {} · КЭШ {} · ОШИБКИ {}",
                                                format_count(complete),
                                                format_count(self.image_index_stats.total),
                                                format_count(speed.round() as u64),
                                                format_duration(eta),
                                                format_count(self.image_index_stats.cached),
                                                format_count(self.image_index_stats.failed)
                                            ))
                                            .size(9.0)
                                            .color(crate::MUTED),
                                        );
                                    },
                                );
                            });
                            ui.add(
                                egui::ProgressBar::new(fraction)
                                    .fill(crate::BLUE)
                                    .desired_height(5.0),
                            );
                        });
                    ui.add_space(12.0);
                }

                let mut requested_mode = None;
                egui::Frame::new()
                    .fill(crate::SURFACE)
                    .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let preview_size = egui::vec2(196.0, 132.0);
                            if let Some(texture) = self.image_query_texture.clone() {
                                ui.add(
                                    egui::Image::new((texture.id(), texture.size_vec2()))
                                        .fit_to_exact_size(preview_size)
                                        .corner_radius(3.0),
                                );
                            } else {
                                let (rect, _) =
                                    ui.allocate_exact_size(preview_size, egui::Sense::hover());
                                ui.painter().rect_filled(rect, 3.0, crate::SURFACE_ALT);
                                ui.painter().rect_stroke(
                                    rect,
                                    3.0,
                                    egui::Stroke::new(1.0_f32, crate::LINE),
                                    egui::StrokeKind::Inside,
                                );
                                ui.painter().text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "ПЕРЕТАЩИТЕ ВЗГЛЯД\nНА ЭТАЛОН",
                                    egui::FontId::monospace(10.0),
                                    crate::MUTED,
                                );
                            }
                            ui.add_space(14.0);
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("ЭТАЛОННОЕ ИЗОБРАЖЕНИЕ")
                                        .size(9.0)
                                        .strong()
                                        .color(crate::BLUE),
                                );
                                ui.add_space(5.0);
                                let reference = self
                                    .image_query_path
                                    .as_deref()
                                    .map(short_path)
                                    .unwrap_or_else(|| {
                                        "Выберите JPEG, PNG или WebP для сравнения".to_owned()
                                    });
                                ui.add_sized(
                                    [ui.available_width().min(560.0), 36.0],
                                    egui::Label::new(
                                        RichText::new(reference)
                                            .size(11.0)
                                            .color(crate::INK),
                                    )
                                    .wrap(),
                                );
                                ui.add_space(7.0);
                                if ui
                                    .add_enabled(
                                        self.image_engine.is_some(),
                                        egui::Button::new("ВЫБРАТЬ ФОТО  →")
                                            .fill(crate::BLUE)
                                            .min_size(egui::vec2(168.0, 34.0)),
                                    )
                                    .on_disabled_hover_text(
                                        "Сначала завершите сканирование и визуальную индексацию",
                                    )
                                    .clicked()
                                {
                                    self.choose_reference_image();
                                }
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    for mode in [
                                        ImageSearchMode::Semantic,
                                        ImageSearchMode::NearVisual,
                                    ] {
                                        let selected = self.image_search_mode == mode;
                                        let button = egui::Button::new(
                                            RichText::new(mode.label()).size(10.0).color(
                                                if selected {
                                                    Color32::WHITE
                                                } else {
                                                    crate::MUTED
                                                },
                                            ),
                                        )
                                        .fill(if selected {
                                            Color32::from_rgb(27, 49, 76)
                                        } else {
                                            crate::SURFACE_ALT
                                        })
                                        .stroke(egui::Stroke::new(
                                            1.0_f32,
                                            if selected { crate::BLUE } else { crate::LINE },
                                        ));
                                        if ui.add(button).clicked() && !selected {
                                            requested_mode = Some(mode);
                                        }
                                    }
                                });
                            });
                        });
                    });
                if let Some(mode) = requested_mode {
                    self.image_search_mode = mode;
                    self.start_image_search();
                }

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "РЕЗУЛЬТАТЫ / {}",
                            format_count(self.image_hits.len() as u64)
                        ))
                        .size(9.0)
                        .strong()
                        .color(crate::INK),
                    );
                    if self.image_searching {
                        ui.spinner();
                        ui.label(
                            RichText::new("Модель сравнивает изображение…")
                                .size(9.0)
                                .color(crate::MUTED),
                        );
                    }
                });
                ui.add_space(8.0);

                if self.image_engine.is_none() && !self.image_indexing {
                    self.image_empty_state(
                        ui,
                        "СНАЧАЛА СОЗДАЙТЕ ИНДЕКС",
                        "Выберите папку и запустите сканирование. После обычного индекса SwiftScan подготовит визуальный кэш.",
                    );
                } else if self.image_query_path.is_none() {
                    self.image_empty_state(
                        ui,
                        "ВЫБЕРИТЕ ФОТОГРАФИЮ",
                        "Эталон обрабатывается один раз, затем поиск идёт по готовому локальному индексу.",
                    );
                } else if self.image_hits.is_empty() && !self.image_searching {
                    self.image_empty_state(
                        ui,
                        "ПОХОЖИХ ФОТО НЕ НАЙДЕНО",
                        "Попробуйте режим «По содержимому» или выберите другой эталон.",
                    );
                } else {
                    let hits = self.image_hits.clone();
                    ScrollArea::vertical()
                        .id_salt("visual-results")
                        .show(ui, |ui| {
                            for row in hits.chunks(3) {
                                ui.columns(3, |columns| {
                                    for (column, hit) in row.iter().enumerate() {
                                        self.image_result_card(&mut columns[column], hit);
                                    }
                                });
                                ui.add_space(8.0);
                            }
                        });
                }
            });
    }

    fn image_empty_state(&self, ui: &mut egui::Ui, title: &str, description: &str) {
        egui::Frame::new()
            .fill(crate::SURFACE)
            .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
            .inner_margin(egui::Margin::same(26))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.add_space(18.0);
                    ui.label(RichText::new(title).size(11.0).strong().color(crate::MUTED));
                    ui.add_space(6.0);
                    ui.label(RichText::new(description).size(11.0).color(crate::MUTED));
                    ui.add_space(18.0);
                });
            });
    }

    fn image_result_card(&mut self, ui: &mut egui::Ui, hit: &ImageSearchHit) {
        let Some(file) = self.files.get(hit.file_index) else {
            return;
        };
        let path = file.path.clone();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("—")
            .to_owned();
        let texture = self.image_result_textures.get(&hit.file_index).cloned();
        let score = (hit.score * 100.0).round().clamp(0.0, 100.0) as u8;
        let kind = match hit.match_kind {
            ImageMatchKind::NearVisual => "ВИЗУАЛЬНАЯ КОПИЯ",
            ImageMatchKind::Semantic => "ПО СОДЕРЖИМОМУ",
        };
        egui::Frame::new()
            .fill(crate::SURFACE)
            .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
            .corner_radius(3.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                let image_size = egui::vec2(ui.available_width(), 124.0);
                if let Some(texture) = texture {
                    ui.add(
                        egui::Image::new((texture.id(), texture.size_vec2()))
                            .fit_to_exact_size(image_size)
                            .corner_radius(2.0),
                    );
                } else {
                    let (rect, _) = ui.allocate_exact_size(image_size, egui::Sense::hover());
                    ui.painter().rect_filled(rect, 2.0, crate::SURFACE_ALT);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "ЗАГРУЗКА",
                        egui::FontId::monospace(9.0),
                        crate::MUTED,
                    );
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(kind).size(8.0).strong().color(crate::BLUE));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{score}%"))
                                .size(11.0)
                                .strong()
                                .color(crate::INK),
                        );
                    });
                });
                let (bar, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 4.0),
                    egui::Sense::hover(),
                );
                ui.painter().rect_filled(bar, 2.0, crate::SURFACE_ALT);
                let filled = egui::Rect::from_min_size(
                    bar.min,
                    egui::vec2(bar.width() * hit.score.clamp(0.0, 1.0), bar.height()),
                );
                ui.painter().rect_filled(filled, 2.0, crate::BLUE);
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(RichText::new(name).size(11.0).color(crate::INK)).truncate(),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(short_path(&path))
                            .monospace()
                            .size(8.0)
                            .color(crate::MUTED),
                    )
                    .truncate(),
                );
                ui.add_space(5.0);
                ui.horizontal(|ui| {
                    if ui.small_button("ОТКРЫТЬ").clicked() {
                        self.notice = Some(match open_path(&path) {
                            Ok(()) => ("Файл открыт".to_owned(), false, Instant::now()),
                            Err(error) => (error, true, Instant::now()),
                        });
                    }
                    if ui.small_button("В ПАПКЕ").clicked() {
                        self.notice = Some(match reveal_in_file_manager(&path) {
                            Ok(()) => ("Папка открыта".to_owned(), false, Instant::now()),
                            Err(error) => (error, true, Instant::now()),
                        });
                    }
                });
            });
    }

    fn games_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(crate::CANVAS)
                    .inner_margin(egui::Margin::same(18)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new("SCAN BREAK").size(17.0).color(crate::INK));
                        let status = if self.scanning {
                            format!(
                                "СКАНИРОВАНИЕ ПРОДОЛЖАЕТСЯ · {} ФАЙЛОВ",
                                format_count(self.stats.files)
                            )
                        } else {
                            "ИГРЫ ДОСТУПНЫ В ЛЮБОЕ ВРЕМЯ".to_owned()
                        };
                        ui.label(RichText::new(status).size(8.0).color(crate::MUTED));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("ВЕРНУТЬСЯ К ФАЙЛАМ").clicked() {
                            self.games_open = false;
                        }
                    });
                });
                ui.add_space(16.0);
                ui.separator();
                ui.add_space(16.0);
                self.games.ui(ui, ctx);
            });
    }

    fn show_notice(&mut self, ctx: &egui::Context) {
        let Some((message, is_error, created)) = &self.notice else {
            return;
        };
        if created.elapsed() >= Duration::from_secs(4) {
            self.notice = None;
            return;
        }

        let message = message.clone();
        let is_error = *is_error;
        let accent = if is_error {
            Color32::from_rgb(240, 101, 108)
        } else {
            Color32::from_rgb(91, 201, 151)
        };
        let title = if is_error {
            "ОШИБКА"
        } else if message.starts_with("Файл перемещён") {
            "УДАЛЕНО"
        } else {
            "ГОТОВО"
        };
        egui::Area::new("scan-notice".into())
            .anchor(egui::Align2::RIGHT_TOP, [-18.0, 8.0])
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(crate::SURFACE_ALT)
                    .stroke(egui::Stroke::new(1.0_f32, accent))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_max_width(360.0);
                        ui.horizontal(|ui| {
                            let (bar_rect, _) = ui.allocate_exact_size(
                                egui::Vec2::new(3.0, 28.0),
                                egui::Sense::hover(),
                            );
                            ui.painter().rect_filled(bar_rect, 1.0, accent);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(title).size(8.0).strong().color(accent));
                                ui.label(RichText::new(message).size(11.0).color(crate::INK));
                            });
                        });
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn delete_confirmation(&mut self, ctx: &egui::Context) {
        let Some(indices) = self.pending_bulk_delete.clone() else {
            return;
        };
        let entries = indices
            .into_iter()
            .filter_map(|index| {
                let file = self.files.get(index)?;
                (!file.deleted).then(|| {
                    let name = file
                        .path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("файл")
                        .to_owned();
                    (
                        index,
                        file.path.clone(),
                        name,
                        file.size,
                        file.is_cleanup_candidate(),
                    )
                })
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            self.pending_bulk_delete = None;
            return;
        }
        let total_size = entries
            .iter()
            .fold(0_u64, |total, entry| total.saturating_add(entry.3));
        let count = entries.len();
        egui::Window::new(format!("Переместить {count} файлов в корзину?"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("Общий размер: {}", format_bytes(total_size)));
                ui.add_space(6.0);
                for (_, _, name, _, _) in entries.iter().take(5) {
                    ui.label(format!("• {name}"));
                }
                if count > 5 {
                    ui.label(format!("…и ещё {}", count - 5));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Отмена").clicked() {
                        self.pending_bulk_delete = None;
                    }
                    if ui.button("В корзину").clicked() {
                        let mut deleted_count = 0_usize;
                        let mut failed = 0_usize;
                        let mut freed = 0_u64;
                        for (index, path, _, size, cleanup_candidate) in &entries {
                            match trash::delete(path) {
                                Ok(()) => {
                                    if let Some(file) =
                                        Arc::make_mut(&mut self.files).get_mut(*index)
                                    {
                                        file.deleted = true;
                                    }
                                    if *cleanup_candidate {
                                        self.analysis.cleanup_files =
                                            self.analysis.cleanup_files.saturating_sub(1);
                                        self.analysis.cleanup_bytes =
                                            self.analysis.cleanup_bytes.saturating_sub(*size);
                                    }
                                    deleted_count += 1;
                                    freed = freed.saturating_add(*size);
                                }
                                Err(_) => failed += 1,
                            }
                        }
                        self.selected_rows.clear();
                        self.selection_anchor = None;
                        let message = if failed > 0 {
                            format!("Перемещено {deleted_count}, не удалось: {failed}")
                        } else if deleted_count == 1 {
                            format!("Файл перемещён в корзину ({})", format_bytes(freed))
                        } else {
                            format!(
                                "Файлов перемещено в корзину: {deleted_count} ({})",
                                format_bytes(freed)
                            )
                        };
                        self.notice = Some((message, failed > 0, Instant::now()));
                        self.pending_bulk_delete = None;
                        self.refresh_visible();
                    }
                });
            });
    }
}

impl eframe::App for ScannerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_dropped_reference(ctx);
        self.poll_scan();
        self.poll_index();
        self.poll_image_index();
        self.poll_image_search(ctx);
        if self.scanning {
            if let Some(started) = self.started_at {
                self.elapsed = started.elapsed();
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.image_indexing || self.image_searching {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        self.sidebar(ctx);
        if self.games_open {
            self.games_panel(ctx);
        } else if self.image_search_open {
            self.image_search_panel(ctx);
        } else {
            self.central_panel(ctx);
        }
        self.delete_confirmation(ctx);
        self.show_notice(ctx);
    }
}
