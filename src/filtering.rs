use crate::model::{CleanupCategory, FileEntry, SortMode, SortRule, ViewMode};

pub(crate) fn file_matches(
    file: &FileEntry,
    query: &str,
    bounds: (u64, u64),
    view: ViewMode,
) -> bool {
    !file.deleted
        && file.size >= bounds.0
        && file.size <= bounds.1
        && match view {
            ViewMode::All => true,
            ViewMode::Cleanup => file.is_cleanup_candidate(),
        }
        && (query.is_empty()
            || file.name_lower.contains(query)
            || file.path.to_string_lossy().to_lowercase().contains(query))
}

pub(crate) fn file_matches_category(file: &FileEntry, category: Option<CleanupCategory>) -> bool {
    match category {
        None => true,
        Some(category) => file.cleanup.category == category && file.is_cleanup_candidate(),
    }
}

pub(crate) fn default_sort_descending(mode: SortMode) -> bool {
    matches!(
        mode,
        SortMode::Size | SortMode::Modified | SortMode::Assessment
    )
}

pub(crate) fn compare_file_entries(
    left: &FileEntry,
    right: &FileEntry,
    rules: &[SortRule],
) -> std::cmp::Ordering {
    for rule in rules {
        let order = match rule.mode {
            SortMode::Size => left.size.cmp(&right.size),
            SortMode::Name => left.name_lower.cmp(&right.name_lower),
            SortMode::Modified => left.modified.cmp(&right.modified),
            SortMode::Assessment => left.cleanup_score().cmp(&right.cleanup_score()),
        };
        if order != std::cmp::Ordering::Equal {
            return if rule.descending {
                order.reverse()
            } else {
                order
            };
        }
    }
    left.path.cmp(&right.path)
}
