use crate::CLEANUP_SCORE_THRESHOLD;
use crate::cleanup::{assessment_from_rating, rate_cleanup_candidate};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Clone)]
pub(crate) struct FileEntry {
    pub(crate) path: PathBuf,
    pub(crate) name_lower: String,
    pub(crate) path_lower: String,
    pub(crate) size: u64,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) assessment: Assessment,
    pub(crate) cleanup: CleanupRating,
    pub(crate) deleted: bool,
}

#[derive(Default, Clone, Copy)]
pub(crate) struct ScanStats {
    pub(crate) files: u64,
    pub(crate) bytes: u64,
    pub(crate) skipped: u64,
}

pub(crate) enum ScanMessage {
    Batch(Vec<FileEntry>, ScanStats),
    Finished(ScanStats, bool),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageSearchMode {
    NearVisual,
    Semantic,
}

impl ImageSearchMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NearVisual => "Визуальные копии",
            Self::Semantic => "По содержимому",
        }
    }
}

#[derive(Clone)]
pub(crate) struct ImageSearchRequest {
    pub(crate) path: PathBuf,
    pub(crate) mode: ImageSearchMode,
    pub(crate) limit: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageMatchKind {
    NearVisual,
    Semantic,
}

#[derive(Clone)]
pub(crate) struct ImageSearchHit {
    pub(crate) file_index: usize,
    pub(crate) score: f32,
    pub(crate) match_kind: ImageMatchKind,
}

#[derive(Clone)]
pub(crate) struct PixelImage {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) rgba: Vec<u8>,
}

pub(crate) trait ImageSearchEngine: Send + Sync {
    fn search(&self, request: &ImageSearchRequest) -> Result<Vec<ImageSearchHit>, String>;
    fn image_count(&self) -> usize;
    fn accelerator(&self) -> String;
}

#[derive(Default, Clone, Copy)]
pub(crate) struct ImageIndexStats {
    pub(crate) total: u64,
    pub(crate) processed: u64,
    pub(crate) cached: u64,
    pub(crate) failed: u64,
}

pub(crate) enum ImageIndexMessage {
    Progress(ImageIndexStats, String),
    Ready(Arc<dyn ImageSearchEngine>, ImageIndexStats),
    Failed(String),
    Cancelled,
}

pub(crate) enum ImageSearchMessage {
    Results(Vec<ImageSearchHit>, Option<PixelImage>),
    Thumbnail(usize, PixelImage),
    Finished,
    Failed(String),
    Cancelled,
}

#[derive(Default, Clone, Copy)]
pub(crate) struct AnalysisStats {
    pub(crate) cleanup_files: u64,
    pub(crate) cleanup_bytes: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewMode {
    All,
    Cleanup,
}

impl ViewMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "Все файлы",
            Self::Cleanup => "Умная очистка",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortMode {
    Size,
    Name,
    Modified,
    Assessment,
}

#[derive(Clone, Copy)]
pub(crate) struct SortRule {
    pub(crate) mode: SortMode,
    pub(crate) descending: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Assessment {
    Cleanup,
    Review,
    Protected,
    Ordinary,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CleanupCategory {
    Temporary,
    Cache,
    Logs,
    CrashDump,
    Backup,
    Development,
    Installer,
    Archive,
    LargeOld,
    Protected,
    Ordinary,
}

impl CleanupCategory {
    pub(crate) const FILTERABLE: [Self; 9] = [
        Self::Temporary,
        Self::Cache,
        Self::Logs,
        Self::CrashDump,
        Self::Backup,
        Self::Development,
        Self::Installer,
        Self::Archive,
        Self::LargeOld,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Temporary => "Временные",
            Self::Cache => "Кэш",
            Self::Logs => "Логи",
            Self::CrashDump => "Дампы",
            Self::Backup => "Копии",
            Self::Development => "Разработка",
            Self::Installer => "Установщики",
            Self::Archive => "Архивы",
            Self::LargeOld => "Старые крупные",
            Self::Protected => "Защищённые",
            Self::Ordinary => "Обычные",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CleanupRating {
    pub(crate) score: u8,
    pub(crate) category: CleanupCategory,
    pub(crate) reason: &'static str,
}

impl FileEntry {
    #[cfg(test)]
    pub(crate) fn new(path: PathBuf, size: u64, modified: Option<SystemTime>) -> Self {
        Self::new_at(path, size, modified, SystemTime::now())
    }

    pub(crate) fn new_at(
        path: PathBuf,
        size: u64,
        modified: Option<SystemTime>,
        scan_time: SystemTime,
    ) -> Self {
        let name_lower = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_lowercase();
        let path_lower = path.to_string_lossy().to_lowercase();
        let cleanup = rate_cleanup_candidate(&path_lower, &name_lower, size, modified, scan_time);
        let assessment = assessment_from_rating(cleanup);
        Self {
            path,
            name_lower,
            path_lower,
            size,
            modified,
            assessment,
            cleanup,
            deleted: false,
        }
    }

    pub(crate) fn cleanup_score(&self) -> u8 {
        self.cleanup.score
    }

    pub(crate) fn cleanup_reason(&self) -> String {
        self.cleanup.reason.to_owned()
    }

    pub(crate) fn is_cleanup_candidate(&self) -> bool {
        !self.deleted
            && self.assessment != Assessment::Protected
            && self.cleanup_score() >= CLEANUP_SCORE_THRESHOLD
    }
}
