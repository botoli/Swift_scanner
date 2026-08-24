use crate::filtering::{
    compare_file_entries, default_sort_descending, file_matches, file_matches_category,
};
use crate::games::MiniGames;
use crate::index::FileIndex;
use crate::model::*;
use crate::scanner::{scan_directory, scan_engine_label, scan_target_capacity};
use crate::ui::*;
use eframe::egui::{self, Color32, RichText, ScrollArea};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

fn last_root_config_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("SwiftScan").join("last_root.txt"))
}

pub(crate) fn read_saved_root(config_path: &Path) -> Option<PathBuf> {
    let root = PathBuf::from(std::fs::read_to_string(config_path).ok()?.trim());
    root.is_dir().then_some(root)
}

fn system_drive_root() -> PathBuf {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
    PathBuf::from(format!("{}\\", drive.trim_end_matches(['\\', '/'])))
}

fn default_scan_root() -> PathBuf {
    last_root_config_path()
        .as_deref()
        .and_then(read_saved_root)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .filter(|path| path.is_dir())
        .or_else(|| {
            let root = system_drive_root();
            root.is_dir().then_some(root)
        })
        .unwrap_or_else(|| PathBuf::from(r"C:\"))
}

fn save_scan_root(root: &Path) -> std::io::Result<()> {
    let Some(config_path) = last_root_config_path() else {
        return Ok(());
    };
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config_path, root.to_string_lossy().as_bytes())
}

pub(crate) fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 780.0])
            .with_min_inner_size([920.0, 620.0]),
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
    file_scope: FileScopeFilter,
    category_filter: Option<CleanupCategory>,
    category_stats: HashMap<CleanupCategory, (u64, u64)>,
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
    search_focus: bool,
    games: MiniGames,
    games_open: bool,
}

impl ScannerApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);
        let root = default_scan_root();
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
            file_scope: FileScopeFilter::All,
            category_filter: None,
            category_stats: HashMap::new(),
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
            search_focus: false,
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
            self.start_scan();
        }
    }

    fn start_scan(&mut self) {
        if !self.root.is_dir() {
            self.notice = Some((
                "Папка недоступна или не существует".to_owned(),
                true,
                Instant::now(),
            ));
            return;
        }
        self.cancel_jobs();
        let _ = save_scan_root(&self.root);
        self.files = Arc::new(Vec::new());
        self.visible.clear();
        self.scan_preview_all.clear();
        self.scan_preview_cleanup.clear();
        self.selected_rows.clear();
        self.selection_anchor = None;
        self.pending_bulk_delete = None;
        self.search_focus = false;
        self.category_filter = None;
        self.category_stats.clear();
        self.file_index = None;
        self.index_receiver = None;
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
        self.scanning = false;
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
                            let stats = self
                                .category_stats
                                .entry(file.cleanup.category)
                                .or_default();
                            stats.0 = stats.0.saturating_add(1);
                            stats.1 = stats.1.saturating_add(file.size);
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
            }
            Err(mpsc::TryRecvError::Empty) => self.index_receiver = Some(receiver),
            Err(mpsc::TryRecvError::Disconnected) => {}
        }
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
                self.file_scope,
                self.category_filter,
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
                .filter(|index| {
                    file_matches(
                        &self.files[*index],
                        &normalized,
                        bounds,
                        self.view,
                        self.file_scope,
                    )
                })
                .filter(|index| file_matches_category(&self.files[*index], self.category_filter))
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
        let viewport_width = ctx.screen_rect().width();
        let sidebar_width = if viewport_width < 1120.0 {
            224.0
        } else {
            268.0
        };
        egui::SidePanel::left("sidebar")
            .exact_width(sidebar_width)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(13, 21, 31))
                    .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
                    .inner_margin(egui::Margin::same(18)),
            )
            .show(ctx, |ui| {
                ScrollArea::vertical()
                    .id_salt("sidebar-content")
                    .auto_shrink([false, false])
                    .max_height(ui.available_height())
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("SWIFTSCAN")
                                    .size(13.0)
                                    .strong()
                                    .color(crate::BLUE),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(RichText::new("LOCAL").size(9.0).color(crate::MUTED));
                                },
                            );
                        });
                        ui.add_space(19.0);
                        ui.horizontal(|ui| {
                            let (icon_rect, _) = ui
                                .allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                            ui.painter().circle_stroke(
                                icon_rect.center(),
                                8.0,
                                egui::Stroke::new(1.5_f32, crate::MUTED),
                            );
                            ui.painter()
                                .circle_filled(icon_rect.center(), 2.5, crate::MUTED);
                            ui.label(
                                RichText::new("Очистка диска")
                                    .size(18.0)
                                    .strong()
                                    .color(crate::INK),
                            );
                        });
                        ui.add_space(7.0);
                        ui.horizontal(|ui| {
                            ui.add_space(32.0);
                            ui.label(
                                RichText::new(
                                    "Поиск крупных файлов и мусора\nбез автоматического удаления.",
                                )
                                .size(11.0)
                                .color(crate::MUTED),
                            );
                        });
                        ui.add_space(16.0);
                        ui.separator();
                        ui.add_space(14.0);
                        Self::section_label(ui, "Область сканирования");
                        ui.add_space(8.0);
                        egui::Frame::new()
                            .fill(crate::SURFACE_ALT)
                            .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
                            .corner_radius(5.0)
                            .inner_margin(egui::Margin::symmetric(11, 8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(short_path(&self.root))
                                                .size(11.0)
                                                .color(crate::INK),
                                        )
                                        .truncate(),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.small_button("□").clicked() {
                                                self.choose_folder();
                                            }
                                        },
                                    );
                                });
                            });
                        ui.add_space(7.0);
                        if ui
                            .add_sized(
                                [ui.available_width(), 32.0],
                                egui::Button::new(
                                    RichText::new("Изменить путь   □")
                                        .size(11.0)
                                        .color(crate::INK),
                                )
                                .fill(crate::SURFACE_ALT),
                            )
                            .clicked()
                        {
                            self.choose_folder();
                        }
                        ui.add_space(7.0);
                        let scan_label = if self.scanning {
                            "ОСТАНОВИТЬ"
                        } else if self.stats.files > 0 {
                            "СКАНИРОВАТЬ СНОВА"
                        } else {
                            "СКАНИРОВАТЬ"
                        };
                        if ui
                            .add_sized(
                                [ui.available_width(), 38.0],
                                egui::Button::new(
                                    RichText::new(format!("⌕  {scan_label}"))
                                        .size(11.0)
                                        .strong()
                                        .color(Color32::WHITE),
                                )
                                .fill(crate::BLUE),
                            )
                            .clicked()
                        {
                            if self.scanning {
                                self.cancel_jobs();
                            } else {
                                self.start_scan();
                            }
                        }
                        ui.add_space(19.0);
                        Self::section_label(ui, "Быстрые фильтры");
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if Self::sidebar_chip(ui, "Все", self.min_mb.is_empty()).clicked() {
                                self.min_mb.clear();
                                self.refresh_visible();
                            }
                            if Self::sidebar_chip(ui, "> 100 МБ", self.min_mb == "100").clicked()
                            {
                                self.min_mb = "100".to_owned();
                                self.refresh_visible();
                            }
                            if Self::sidebar_chip(ui, "> 1 ГБ", self.min_mb == "1024").clicked() {
                                self.min_mb = "1024".to_owned();
                                self.refresh_visible();
                            }
                        });
                        ui.add_space(17.0);
                        Self::section_label(ui, "Размер файлов");
                        ui.add_space(7.0);
                        ui.horizontal(|ui| {
                            let field_width = ((ui.available_width() - 8.0) / 2.0).max(62.0);
                            let min_changed = ui
                                .add_sized(
                                    [field_width, 32.0],
                                    egui::TextEdit::singleline(&mut self.min_mb)
                                        .hint_text("от 100 МБ"),
                                )
                                .changed();
                            let max_changed = ui
                                .add_sized(
                                    [field_width, 32.0],
                                    egui::TextEdit::singleline(&mut self.max_mb)
                                        .hint_text("до 1 ГБ"),
                                )
                                .changed();
                            if min_changed || max_changed {
                                self.refresh_visible();
                            }
                        });
                        ui.add_space(17.0);
                        Self::section_label(ui, "Типы файлов");
                        ui.add_space(7.0);
                        ui.horizontal(|ui| {
                            for (label, scope) in [
                                ("Все", FileScopeFilter::All),
                                ("Система", FileScopeFilter::System),
                                ("Польз.", FileScopeFilter::User),
                            ] {
                                if Self::sidebar_chip(ui, label, self.file_scope == scope).clicked()
                                {
                                    self.file_scope = scope;
                                    self.selected_rows.clear();
                                    self.selection_anchor = None;
                                    self.refresh_visible();
                                }
                            }
                        });
                        ui.add_space(16.0);
                        ui.separator();
                        ui.add_space(13.0);
                        Self::section_label(ui, "Просмотр");
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
                        if self.view == ViewMode::Cleanup && !self.games_open {
                            self.category_chips(ui);
                        }

                        ui.add_space(13.0);
                        ui.separator();
                        ui.add_space(13.0);
                        Self::section_label(ui, "Разделение по типу");
                        ui.add_space(5.0);
                        let mut requested_game = None;
                        ui.columns(3, |columns| {
                            if columns[0]
                                .add_sized(
                                    [columns[0].available_width(), 68.0],
                                    egui::Button::new("⊞\nКрестики\nнолики"),
                                )
                                .clicked()
                            {
                                requested_game = Some(0);
                            }
                            if columns[1]
                                .add_sized(
                                    [columns[1].available_width(), 68.0],
                                    egui::Button::new("S\nЗмейка"),
                                )
                                .clicked()
                            {
                                requested_game = Some(1);
                            }
                            if columns[2]
                                .add_sized(
                                    [columns[2].available_width(), 68.0],
                                    egui::Button::new("D\nДино"),
                                )
                                .clicked()
                            {
                                requested_game = Some(2);
                            }
                        });
                        if let Some(game) = requested_game {
                            self.games_open = true;
                            match game {
                                0 => self.games.open_tic_tac_toe(),
                                1 => self.games.open_snake(),
                                _ => self.games.open_dinosaur(),
                            }
                        }
                    });
            });
    }

    fn section_label(ui: &mut egui::Ui, label: &str) {
        ui.label(RichText::new(label).size(10.0).color(crate::MUTED));
    }

    fn sidebar_chip(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
        ui.add(
            egui::Button::new(RichText::new(label).size(10.0).color(if selected {
                crate::INK
            } else {
                crate::MUTED
            }))
            .fill(if selected {
                Color32::from_rgb(25, 54, 91)
            } else {
                crate::SURFACE_ALT
            })
            .stroke(egui::Stroke::new(
                1.0_f32,
                if selected { crate::BLUE } else { crate::LINE },
            )),
        )
    }

    fn stat_cell(ui: &mut egui::Ui, label: &str, value: String, color: Color32) {
        ui.horizontal(|ui| {
            let (icon_rect, _) =
                ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(icon_rect.center(), 17.0, color.gamma_multiply(0.13));
            ui.painter()
                .circle_stroke(icon_rect.center(), 10.0, egui::Stroke::new(1.4_f32, color));
            ui.painter().circle_filled(icon_rect.center(), 2.5, color);
            ui.vertical(|ui| {
                ui.label(RichText::new(label).size(9.0).color(crate::MUTED));
                ui.label(RichText::new(value).size(15.0).color(crate::INK));
            });
        });
    }

    fn mode_tab(&mut self, ui: &mut egui::Ui, mode: ViewMode, primary: String, secondary: String) {
        let selected = self.view == mode && !self.games_open;
        let frame = egui::Frame::new()
            .fill(if selected {
                Color32::from_rgb(23, 45, 72)
            } else {
                Color32::TRANSPARENT
            })
            .stroke(egui::Stroke::NONE)
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(9, 8));
        let response = frame
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (bar_rect, _) =
                        ui.allocate_exact_size(egui::Vec2::new(3.0, 32.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        bar_rect,
                        1.0,
                        if selected {
                            crate::BLUE
                        } else {
                            Color32::TRANSPARENT
                        },
                    );
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(primary)
                                .size(11.0)
                                .strong()
                                .color(if selected { crate::INK } else { crate::MUTED }),
                        );
                        ui.label(RichText::new(secondary).size(9.0).color(if selected {
                            crate::BLUE
                        } else {
                            crate::MUTED
                        }));
                    });
                });
            })
            .response;
        if response.interact(egui::Sense::click()).clicked() {
            self.view = mode;
            if mode == ViewMode::All {
                self.category_filter = None;
            }
            self.games_open = false;
            self.refresh_visible();
        }
    }

    fn category_chips(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.label(
            RichText::new("КАТЕГОРИИ")
                .size(8.0)
                .strong()
                .color(crate::MUTED),
        );
        ui.add_space(5.0);
        let all_selected = self.category_filter.is_none();
        let all_button =
            egui::Button::new(RichText::new("Все категории").size(10.0)).fill(if all_selected {
                Color32::from_rgb(27, 49, 76)
            } else {
                crate::SURFACE
            });
        if ui
            .add_sized([ui.available_width(), 28.0], all_button)
            .clicked()
            && !all_selected
        {
            self.category_filter = None;
            self.refresh_visible();
        }

        ui.add_space(4.0);
        let categories = CleanupCategory::FILTERABLE
            .into_iter()
            .filter_map(|category| {
                self.category_stats
                    .get(&category)
                    .copied()
                    .filter(|stats| stats.0 > 0)
                    .map(|stats| (category, stats))
            })
            .collect::<Vec<_>>();
        egui::Grid::new("cleanup-category-grid")
            .num_columns(2)
            .spacing([4.0, 4.0])
            .show(ui, |ui| {
                let chip_width = ((ui.available_width() - 4.0) / 2.0).max(78.0);
                for (position, (category, (count, bytes))) in categories.iter().enumerate() {
                    let selected = self.category_filter == Some(*category);
                    let text = format!(
                        "{}\n{} · {}",
                        category.label(),
                        format_count(*count),
                        format_bytes(*bytes)
                    );
                    let button =
                        egui::Button::new(RichText::new(text).size(9.0)).fill(if selected {
                            Color32::from_rgb(27, 49, 76)
                        } else {
                            crate::SURFACE
                        });
                    if ui.add_sized([chip_width, 44.0], button).clicked() && !selected {
                        self.category_filter = Some(*category);
                        self.view = ViewMode::Cleanup;
                        self.refresh_visible();
                    }
                    if position % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
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

    fn handle_keyboard_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let (select_all, delete, escape, focus_search) = ctx.input(|input| {
            (
                input.modifiers.command && input.key_pressed(egui::Key::A),
                input.key_pressed(egui::Key::Delete),
                input.key_pressed(egui::Key::Escape),
                input.modifiers.command && input.key_pressed(egui::Key::F),
            )
        });

        if select_all {
            self.selected_rows = self.visible.iter().copied().collect();
            self.selection_anchor = None;
        }
        if delete && self.file_index.is_some() {
            let selected = self
                .selected_rows
                .iter()
                .copied()
                .filter(|index| self.files.get(*index).is_some_and(|file| !file.deleted))
                .collect::<Vec<_>>();
            if !selected.is_empty() {
                self.pending_bulk_delete = Some(selected);
            }
        }
        if escape {
            self.query.clear();
            self.min_mb.clear();
            self.max_mb.clear();
            self.file_scope = FileScopeFilter::All;
            self.category_filter = None;
            self.selected_rows.clear();
            self.selection_anchor = None;
            self.refresh_visible();
        }
        if focus_search {
            self.search_focus = true;
        }
    }

    fn central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(crate::CANVAS)
                    .inner_margin(egui::Margin::symmetric(18, 14)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(self.view.label())
                                .size(19.0)
                                .strong()
                                .color(crate::INK),
                        );
                        let status = if self.scanning {
                            "Сканирование файлов"
                        } else {
                            "Сканирование завершено"
                        };
                        ui.label(
                            RichText::new(format!("{status} · {}", scan_engine_label()))
                                .size(10.0)
                                .color(crate::MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let search_width = ui.available_width().clamp(180.0, 340.0);
                        let response = ui.add_sized(
                            [search_width, 36.0],
                            egui::TextEdit::singleline(&mut self.query)
                                .hint_text("Поиск по имени или пути…"),
                        );
                        if self.search_focus {
                            response.request_focus();
                            self.search_focus = false;
                        }
                        if response.changed() {
                            self.refresh_visible();
                        }
                    });
                });
                ui.add_space(16.0);

                let speed = if self.elapsed.as_secs_f64() > 0.0 {
                    self.stats.files as f64 / self.elapsed.as_secs_f64()
                } else {
                    0.0
                };
                egui::Frame::new()
                    .fill(crate::SURFACE)
                    .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
                    .corner_radius(7.0)
                    .inner_margin(egui::Margin::symmetric(16, 13))
                    .show(ui, |ui| {
                        if ui.available_width() >= 850.0 {
                            ui.columns(5, |columns| {
                                Self::stat_cell(
                                    &mut columns[0],
                                    "Найдено файлов",
                                    format_count(self.stats.files),
                                    crate::BLUE,
                                );
                                Self::stat_cell(
                                    &mut columns[1],
                                    "Занято на диске",
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
                                    "Сканирование",
                                    format_duration(self.elapsed),
                                    crate::MUTED,
                                );
                                Self::stat_cell(
                                    &mut columns[3],
                                    "Скорость",
                                    format!("{}/с", format_count(speed as u64)),
                                    Color32::from_rgb(91, 201, 151),
                                );
                                Self::stat_cell(
                                    &mut columns[4],
                                    "Можно освободить",
                                    format_bytes(self.analysis.cleanup_bytes),
                                    Color32::from_rgb(91, 201, 151),
                                );
                            });
                        } else {
                            ui.columns(3, |columns| {
                                Self::stat_cell(
                                    &mut columns[0],
                                    "Файлов",
                                    format_count(self.stats.files),
                                    crate::BLUE,
                                );
                                Self::stat_cell(
                                    &mut columns[1],
                                    "Сканирование",
                                    format_duration(self.elapsed),
                                    crate::MUTED,
                                );
                                Self::stat_cell(
                                    &mut columns[2],
                                    "Можно освободить",
                                    format_bytes(self.analysis.cleanup_bytes),
                                    Color32::from_rgb(91, 201, 151),
                                );
                            });
                            ui.add_space(8.0);
                            ui.columns(2, |columns| {
                                Self::stat_cell(
                                    &mut columns[0],
                                    "Объём",
                                    format_bytes(self.stats.bytes),
                                    Color32::from_rgb(244, 174, 91),
                                );
                                Self::stat_cell(
                                    &mut columns[1],
                                    "Скорость",
                                    format!("{}/с", format_count(speed as u64)),
                                    Color32::from_rgb(91, 201, 151),
                                );
                            });
                        }
                    });
                ui.add_space(12.0);

                if self.files.is_empty() && !self.scanning {
                    ui.add_space(42.0);
                    ui.vertical_centered(|ui| {
                        egui::Frame::new()
                            .fill(crate::SURFACE)
                            .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
                            .inner_margin(egui::Margin::symmetric(28, 24))
                            .show(ui, |ui| {
                                ui.set_max_width(460.0);
                                ui.label(
                                    RichText::new("ВЫБЕРИТЕ ОБЛАСТЬ ДЛЯ ПОИСКА")
                                        .size(12.0)
                                        .strong()
                                        .color(crate::INK),
                                );
                                ui.add_space(7.0);
                                ui.label(
                                    RichText::new(
                                        "SwiftScan найдёт крупные файлы и безопасные кандидаты на очистку.",
                                    )
                                    .size(11.0)
                                    .color(crate::MUTED),
                                );
                                ui.add_space(14.0);
                                if ui
                                    .add(
                                        egui::Button::new("ВЫБРАТЬ ПАПКУ  →")
                                            .fill(crate::BLUE)
                                            .min_size(egui::vec2(190.0, 38.0)),
                                    )
                                    .clicked()
                                {
                                    self.choose_folder();
                                }
                            });
                    });
                    return;
                }

                let unfiltered = self.query.trim().is_empty()
                    && self.min_mb.trim().is_empty()
                    && self.max_mb.trim().is_empty()
                    && self.file_scope == FileScopeFilter::All;
                let result_count = if unfiltered {
                    format_count(match (self.view, self.category_filter) {
                        (ViewMode::All, _) => self.stats.files,
                        (ViewMode::Cleanup, Some(category)) => self
                            .category_stats
                            .get(&category)
                            .map_or(0, |stats| stats.0),
                        (ViewMode::Cleanup, None) => self.analysis.cleanup_files,
                    })
                } else if self.has_more_rows {
                    format!("{}+", format_count(self.visible.len() as u64))
                } else {
                    format_count(self.visible.len() as u64)
                };
                let can_delete = self.file_index.is_some();
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
                let selected_bytes = selected.iter().fold(0_u64, |total, index| {
                    total.saturating_add(self.files[*index].size)
                });
                let table_outer_width = ui.available_width();
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::Frame::new()
                    .fill(if selected.is_empty() {
                        crate::SURFACE
                    } else {
                        Color32::from_rgb(20, 36, 55)
                    })
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        if selected.is_empty() {
                            crate::LINE
                        } else {
                            crate::BLUE
                        },
                    ))
                    .corner_radius(egui::CornerRadius {
                        nw: 7,
                        ne: 7,
                        sw: 0,
                        se: 0,
                    })
                    .inner_margin(egui::Margin::symmetric(16, 11))
                    .show(ui, |ui| {
                        ui.set_min_width((table_outer_width - 32.0).max(0.0));
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                if selected.is_empty() {
                                    ui.label(
                                        RichText::new(format!(
                                            "{} · {} объектов",
                                            self.view.label(),
                                            result_count
                                        ))
                                        .size(12.0)
                                        .strong()
                                        .color(crate::INK),
                                    );
                                    ui.label(
                                        RichText::new(format!(
                                            "Показано {}. Используйте фильтры для уточнения результатов.",
                                            self.visible.len()
                                        ))
                                        .size(9.0)
                                        .color(crate::MUTED),
                                    );
                                } else {
                                    ui.label(
                                        RichText::new(format!(
                                            "ВЫБРАНО {} · {}",
                                            selected.len(),
                                            format_bytes(selected_bytes)
                                        ))
                                        .size(11.0)
                                        .strong()
                                        .color(crate::INK),
                                    );
                                    ui.label(
                                        RichText::new("Файлы будут перемещены в корзину")
                                            .size(8.0)
                                            .color(crate::MUTED),
                                    );
                                }
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if !selected.is_empty() {
                                        if ui
                                            .add_enabled(
                                                can_delete && !selected.is_empty(),
                                                egui::Button::new("В КОРЗИНУ")
                                                    .fill(crate::BLUE)
                                                    .min_size(egui::vec2(132.0, 34.0)),
                                            )
                                            .on_disabled_hover_text(
                                                "Доступно после подготовки списка",
                                            )
                                            .clicked()
                                        {
                                            self.pending_bulk_delete = Some(selected.clone());
                                        }
                                        if ui.small_button("Снять").clicked() {
                                            self.selected_rows.clear();
                                            self.selection_anchor = None;
                                        }
                                    } else if ui.button("Выбрать все").clicked() {
                                        self.selected_rows = self.visible.iter().copied().collect();
                                        self.selection_anchor = None;
                                    }
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
                let table_height = ui.available_height();
                let scroll_output = egui::Frame::new()
                    .fill(crate::SURFACE)
                    .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
                    .corner_radius(egui::CornerRadius {
                        nw: 0,
                        ne: 0,
                        sw: 7,
                        se: 7,
                    })
                    .show(ui, |ui| {
                        ui.set_min_width(table_outer_width);
                        ScrollArea::vertical()
                            .id_salt(("file-table", self.list_revision))
                            .auto_shrink([false, false])
                            .max_width(table_outer_width)
                            .max_height(table_height)
                            .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                        let table_width = (table_outer_width - 14.0).max(0.0);
                        let show_modified = table_width >= 760.0;
                        let show_status = table_width >= 900.0;
                        let category_width = 110.0;
                        let modified_width = 132.0;
                        let size_width = 90.0;
                        let status_width = 96.0;
                        let menu_width = 44.0;
                        let fixed_width = 32.0
                            + category_width
                            + size_width
                            + menu_width
                            + if show_modified { modified_width } else { 0.0 }
                            + if show_status { status_width } else { 0.0 };
                        let file_width = (table_width - fixed_width).max(190.0);
                        ui.set_width(table_width);
                        ui.set_min_width(table_width);
                        ui.set_min_height(table_height);
                        ui.horizontal(|ui| {
                            let visible_rows = self
                                .visible
                                .iter()
                                .copied()
                                .filter(|index| {
                                    self.files
                                        .get(*index)
                                        .is_some_and(|file| !file.deleted)
                                })
                                .collect::<Vec<_>>();
                            let mut all_selected = !visible_rows.is_empty()
                                && visible_rows
                                    .iter()
                                    .all(|index| self.selected_rows.contains(index));
                            let select_all_response = ui
                                .allocate_ui_with_layout(
                                    egui::vec2(32.0, 34.0),
                                    egui::Layout::centered_and_justified(
                                        egui::Direction::LeftToRight,
                                    ),
                                    |ui| ui.checkbox(&mut all_selected, ""),
                                )
                                .inner;
                            if select_all_response.changed() {
                                if all_selected {
                                    self.selected_rows.extend(visible_rows);
                                } else {
                                    for index in visible_rows {
                                        self.selected_rows.remove(&index);
                                    }
                                    self.selection_anchor = None;
                                }
                            }
                            self.header_cell(ui, "ФАЙЛ И ПУТЬ", file_width, Some(SortMode::Name));
                            self.header_cell(ui, "КАТЕГОРИЯ", category_width, None);
                            if show_modified {
                                self.header_cell(
                                    ui,
                                    "ИЗМЕНЁН",
                                    modified_width,
                                    Some(SortMode::Modified),
                                );
                            }
                            self.header_cell(ui, "РАЗМЕР", size_width, Some(SortMode::Size));
                            if show_status {
                                self.header_cell(
                                    ui,
                                    "СТАТУС",
                                    status_width,
                                    Some(SortMode::Assessment),
                                );
                            }
                            self.header_cell(ui, "", menu_width, None);
                        });
                        ui.separator();
                        for index in row_indices {
                            let selected = self.selected_rows.contains(&index);
                            let file = &self.files[index];
                            let path = file.path.clone();
                            let full_path = path.to_string_lossy().into_owned();
                            let name = path
                                .file_name()
                                .and_then(|value| value.to_str())
                                .unwrap_or("—")
                                .to_owned();
                            let parent = path
                                .parent()
                                .map(short_path)
                                .unwrap_or_else(|| "—".to_owned());
                            let extension = file_extension(&path);
                            let modified = format_modified(file.modified);
                            let age = format_file_age(file.modified);
                            let score = file.cleanup_score();
                            let reason = file.cleanup_reason();
                            let category = file.cleanup.category.label();
                            let size = format_bytes(file.size);
                            let (status, score_color) = assessment_label(score);
                            let row_rect = egui::Frame::new()
                                .fill(if selected {
                                    Color32::from_rgb(25, 43, 66)
                                } else {
                                    crate::SURFACE
                                })
                                .stroke(egui::Stroke::new(1.0_f32, crate::LINE))
                                .inner_margin(egui::Margin::ZERO)
                                .show(ui, |ui| {
                                    ui.set_min_width(table_width);
                                    ui.set_min_height(54.0);
                                    ui.horizontal(|ui| {
                                        let mut checked = selected;
                                        let checkbox = ui
                                            .allocate_ui_with_layout(
                                                egui::vec2(32.0, 54.0),
                                                egui::Layout::centered_and_justified(
                                                    egui::Direction::LeftToRight,
                                                ),
                                                |ui| {
                                                    ui.set_min_size(egui::vec2(32.0, 54.0));
                                                    ui.checkbox(&mut checked, "")
                                                },
                                            )
                                            .inner;
                                        if checkbox.changed() {
                                            if checked {
                                                self.selected_rows.insert(index);
                                                self.selection_anchor = Some(index);
                                            } else {
                                                self.selected_rows.remove(&index);
                                                if self.selection_anchor == Some(index) {
                                                    self.selection_anchor = None;
                                                }
                                            }
                                        }
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(file_width, 54.0),
                                            egui::Layout::top_down(egui::Align::Min),
                                            |ui| {
                                                ui.set_min_size(egui::vec2(file_width, 54.0));
                                                ui.spacing_mut().item_spacing =
                                                    egui::vec2(7.0, 1.0);
                                                ui.add_space(6.0);
                                                ui.horizontal(|ui| {
                                                    egui::Frame::new()
                                                        .fill(Color32::from_rgb(25, 58, 101))
                                                        .stroke(egui::Stroke::new(
                                                            1.0_f32,
                                                            crate::BLUE,
                                                        ))
                                                        .corner_radius(4.0)
                                                        .inner_margin(
                                                            egui::Margin::symmetric(5, 4),
                                                        )
                                                        .show(ui, |ui| {
                                                            ui.label(
                                                                RichText::new(&extension)
                                                                    .monospace()
                                                                    .strong()
                                                                    .size(8.0)
                                                                    .color(Color32::from_rgb(
                                                                        151, 193, 255,
                                                                    )),
                                                            );
                                                        });
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(&name)
                                                                .size(11.0)
                                                                .strong()
                                                                .color(crate::INK),
                                                        )
                                                        .truncate(),
                                                    );
                                                });
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(&parent)
                                                            .monospace()
                                                            .size(9.0)
                                                            .color(crate::MUTED),
                                                    )
                                                    .truncate(),
                                                );
                                            },
                                        )
                                        .response
                                        .on_hover_text(&full_path);
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(category_width, 54.0),
                                            egui::Layout::centered_and_justified(
                                                egui::Direction::LeftToRight,
                                            ),
                                            |ui| {
                                                ui.set_min_size(egui::vec2(category_width, 54.0));
                                                egui::Frame::new()
                                                    .fill(score_color.gamma_multiply(0.12))
                                                    .stroke(egui::Stroke::new(1.0_f32, score_color))
                                                    .corner_radius(3.0)
                                                    .inner_margin(egui::Margin::symmetric(7, 4))
                                                    .show(ui, |ui| {
                                                        ui.label(
                                                            RichText::new(category)
                                                                .size(8.0)
                                                                .color(score_color),
                                                        );
                                                    });
                                            },
                                        );
                                        if show_modified {
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(modified_width, 54.0),
                                                egui::Layout::top_down(egui::Align::Min),
                                                |ui| {
                                                    ui.set_min_size(egui::vec2(
                                                        modified_width,
                                                        54.0,
                                                    ));
                                                    ui.label(
                                                        RichText::new(&modified)
                                                            .monospace()
                                                            .size(8.0)
                                                            .color(crate::INK),
                                                    );
                                                    ui.label(
                                                        RichText::new(&age)
                                                            .size(8.0)
                                                            .color(crate::MUTED),
                                                    );
                                                },
                                            );
                                        }
                                        ui.add_sized(
                                            [size_width, 54.0],
                                            egui::Label::new(
                                                RichText::new(size)
                                                    .monospace()
                                                    .strong()
                                                    .size(9.0)
                                                    .color(crate::INK),
                                            ),
                                        );
                                        if show_status {
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(status_width, 54.0),
                                                egui::Layout::centered_and_justified(
                                                    egui::Direction::LeftToRight,
                                                ),
                                                |ui| {
                                                    ui.set_min_size(egui::vec2(
                                                        status_width,
                                                        54.0,
                                                    ));
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "{status}\n{score} / 100"
                                                        ))
                                                        .size(8.0)
                                                        .color(score_color),
                                                    );
                                                },
                                            );
                                        }
                                        ui.allocate_ui_with_layout(
                                            egui::Vec2::new(menu_width, 54.0),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                ui.set_min_size(egui::vec2(menu_width, 54.0));
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
                                egui::pos2(row_rect.min.x + 34.0, row_rect.min.y),
                                egui::pos2(
                                    (row_rect.max.x - 52.0).max(row_rect.min.x + 34.0),
                                    row_rect.max.y,
                                ),
                            );
                            let selection_response = ui.interact(
                                selection_rect,
                                ui.id().with(("file-row", index)),
                                egui::Sense::click(),
                            )
                            .on_hover_ui(|ui| {
                                ui.label(
                                    RichText::new(&name).strong().color(crate::INK),
                                );
                                ui.label(
                                    RichText::new(&full_path)
                                        .monospace()
                                        .size(9.0)
                                        .color(crate::MUTED),
                                );
                                ui.separator();
                                ui.label(format!("Категория: {category}"));
                                ui.label(format!("Почему найден: {reason}"));
                                ui.label(format!("Изменён: {modified} · {age}"));
                                ui.label(
                                    RichText::new(format!("{status} · {score} / 100"))
                                        .color(score_color),
                                );
                            });
                            selection_response.context_menu(|ui| {
                                self.row_context_menu(ui, index);
                            });
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
                            if selection_response.double_clicked() {
                                self.notice = Some(match open_path(&path) {
                                    Ok(()) => ("Файл открыт".to_owned(), false, Instant::now()),
                                    Err(error) => (error, true, Instant::now()),
                                });
                            }
                        }
                            })
                    })
                    .inner;
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
                        file.cleanup.category,
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
                for (_, _, name, _, _, _) in entries.iter().take(5) {
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
                        for (index, path, _, size, cleanup_candidate, category) in &entries {
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
                                        if let Some(stats) = self.category_stats.get_mut(category) {
                                            stats.0 = stats.0.saturating_sub(1);
                                            stats.1 = stats.1.saturating_sub(*size);
                                        }
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
        self.handle_keyboard_shortcuts(ctx);
        self.poll_scan();
        self.poll_index();
        if self.scanning {
            if let Some(started) = self.started_at {
                self.elapsed = started.elapsed();
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.sidebar(ctx);
        if self.games_open {
            self.games_panel(ctx);
        } else {
            self.central_panel(ctx);
        }
        self.delete_confirmation(ctx);
        self.show_notice(ctx);
    }
}
