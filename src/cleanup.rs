use crate::CLEANUP_SCORE_THRESHOLD;
use crate::model::{Assessment, CleanupCategory, CleanupRating};
use std::path::Path;
use std::time::SystemTime;

pub(crate) fn rate_cleanup_candidate(
    path_lower: &str,
    name_lower: &str,
    size: u64,
    modified: Option<SystemTime>,
    scan_time: SystemTime,
) -> CleanupRating {
    let extension = Path::new(name_lower)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default();

    if contains_directory(path_lower, "\\windows\\", "/windows/")
        || contains_directory(path_lower, "\\program files\\", "/program files/")
        || matches!(name_lower, "pagefile.sys" | "hiberfil.sys" | "swapfile.sys")
        || matches!(extension, "sys" | "dll" | "drv")
    {
        return CleanupRating {
            score: 0,
            category: CleanupCategory::Protected,
            reason: "Системная область или исполняемый компонент",
        };
    }

    let age_days = modified
        .and_then(|time| scan_time.duration_since(time).ok())
        .map_or(0, |age| age.as_secs() / 86_400);
    let age_bonus = match age_days {
        0..=30 => 0,
        31..=180 => 6,
        181..=365 => 12,
        _ => 18,
    };
    let size_bonus = match size {
        0..=10_485_760 => 0,
        10_485_761..=104_857_600 => 4,
        104_857_601..=1_073_741_824 => 8,
        _ => 14,
    };
    let rating = if contains_directory(path_lower, "\\temp\\", "/temp/") || extension == "tmp" {
        (
            72,
            CleanupCategory::Temporary,
            "Временное расположение или расширение .tmp",
        )
    } else if contains_directory(path_lower, "\\cache\\", "/cache/")
        || contains_directory(path_lower, "\\caches\\", "/caches/")
        || contains_directory(path_lower, "\\code cache\\", "/code cache/")
    {
        (
            68,
            CleanupCategory::Cache,
            "Кэш можно восстановить при следующем запуске программы",
        )
    } else if matches!(extension, "dmp" | "mdmp") {
        (
            76,
            CleanupCategory::CrashDump,
            "Диагностический дамп завершившегося сбоя",
        )
    } else if extension == "log" {
        (
            52,
            CleanupCategory::Logs,
            "Журнал работы программы; полезность снижается со временем",
        )
    } else if matches!(extension, "bak" | "old" | "orig") {
        (
            45,
            CleanupCategory::Backup,
            "Резервная или прежняя версия файла",
        )
    } else if contains_directory(path_lower, "\\target\\", "/target/")
        || contains_directory(path_lower, "\\node_modules\\", "/node_modules/")
        || contains_directory(path_lower, "\\.next\\", "/.next/")
        || contains_directory(path_lower, "\\dist\\", "/dist/")
        || contains_directory(path_lower, "\\build\\", "/build/")
    {
        (
            62,
            CleanupCategory::Development,
            "Генерируемый каталог разработки или сборки",
        )
    } else if matches!(extension, "msi" | "msix" | "exe") {
        (
            36,
            CleanupCategory::Installer,
            "Установщик стоит проверить после завершения установки",
        )
    } else if matches!(extension, "iso" | "zip" | "7z" | "rar" | "tar" | "gz") {
        (
            34,
            CleanupCategory::Archive,
            "Архив может дублировать уже распакованные данные",
        )
    } else if size >= 1024 * 1024 * 1024 && age_days >= 180 {
        (
            42,
            CleanupCategory::LargeOld,
            "Крупный файл не изменялся больше шести месяцев",
        )
    } else {
        return CleanupRating {
            score: 8,
            category: CleanupCategory::Ordinary,
            reason: "Нет надёжных признаков ненужного файла",
        };
    };
    CleanupRating {
        score: (rating.0 + age_bonus + size_bonus).min(100),
        category: rating.1,
        reason: rating.2,
    }
}

fn contains_directory(path_lower: &str, windows: &str, unix: &str) -> bool {
    path_lower.contains(windows) || path_lower.contains(unix)
}

pub(crate) fn assessment_from_rating(rating: CleanupRating) -> Assessment {
    if rating.category == CleanupCategory::Protected {
        Assessment::Protected
    } else if rating.score >= CLEANUP_SCORE_THRESHOLD {
        Assessment::Cleanup
    } else if rating.score >= 30 {
        Assessment::Review
    } else {
        Assessment::Ordinary
    }
}
