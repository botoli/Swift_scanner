use crate::filtering::file_matches;
use crate::model::{FileEntry, SortMode, SortRule, ViewMode};
use std::collections::{HashMap, HashSet};

/// Immutable in-memory index used by every search and sort request after a scan.
pub(crate) struct FileIndex {
    trigrams: HashMap<u32, Vec<usize>>,
    by_size: Vec<usize>,
    by_name: Vec<usize>,
    by_modified: Vec<usize>,
    by_assessment: Vec<usize>,
    cleanup_by_size: Vec<usize>,
    cleanup_by_name: Vec<usize>,
    cleanup_by_modified: Vec<usize>,
    cleanup_by_assessment: Vec<usize>,
}

impl FileIndex {
    /// Builds search postings and stable sort orders once, away from keystrokes.
    pub(crate) fn build(files: &[FileEntry]) -> Self {
        let mut trigrams: HashMap<u32, Vec<usize>> = HashMap::new();
        for (index, file) in files.iter().enumerate() {
            let mut unique = HashSet::new();
            for key in trigram_keys(file.path_lower.as_bytes()) {
                if unique.insert(key) {
                    trigrams.entry(key).or_default().push(index);
                }
            }
        }
        let base = (0..files.len()).collect::<Vec<_>>();
        let mut by_size = base.clone();
        by_size.sort_unstable_by_key(|index| std::cmp::Reverse(files[*index].size));
        let mut by_name = base.clone();
        by_name
            .sort_unstable_by(|left, right| files[*left].name_lower.cmp(&files[*right].name_lower));
        let mut by_modified = base.clone();
        by_modified.sort_unstable_by_key(|index| std::cmp::Reverse(files[*index].modified));
        let mut by_assessment = base;
        by_assessment
            .sort_unstable_by_key(|index| std::cmp::Reverse(files[*index].cleanup_score()));
        let cleanup_by_size = cleanup_order(&by_size, files);
        let cleanup_by_name = cleanup_order(&by_name, files);
        let cleanup_by_modified = cleanup_order(&by_modified, files);
        let cleanup_by_assessment = cleanup_order(&by_assessment, files);
        Self {
            trigrams,
            by_size,
            by_name,
            by_modified,
            by_assessment,
            cleanup_by_size,
            cleanup_by_name,
            cleanup_by_modified,
            cleanup_by_assessment,
        }
    }

    pub(crate) fn query_page(
        &self,
        files: &[FileEntry],
        query: &str,
        bounds: (u64, u64),
        view: ViewMode,
        sort: SortRule,
        limit: usize,
    ) -> (Vec<usize>, bool) {
        self.query_with_limit(files, query, bounds, view, sort, limit)
    }

    fn query_with_limit(
        &self,
        files: &[FileEntry],
        query: &str,
        bounds: (u64, u64),
        view: ViewMode,
        sort: SortRule,
        limit: usize,
    ) -> (Vec<usize>, bool) {
        let normalized = query.trim().to_lowercase();
        let candidates = self.candidates(&normalized);
        let candidate_set = candidates.map(|items| items.into_iter().collect::<HashSet<_>>());
        let order = match (view, sort.mode) {
            (ViewMode::All, SortMode::Size) => &self.by_size,
            (ViewMode::All, SortMode::Name) => &self.by_name,
            (ViewMode::All, SortMode::Modified) => &self.by_modified,
            (ViewMode::All, SortMode::Assessment) => &self.by_assessment,
            (ViewMode::Cleanup, SortMode::Size) => &self.cleanup_by_size,
            (ViewMode::Cleanup, SortMode::Name) => &self.cleanup_by_name,
            (ViewMode::Cleanup, SortMode::Modified) => &self.cleanup_by_modified,
            (ViewMode::Cleanup, SortMode::Assessment) => &self.cleanup_by_assessment,
        };
        let iterator: Box<dyn Iterator<Item = &usize>> = if sort.descending
            == matches!(
                sort.mode,
                SortMode::Size | SortMode::Modified | SortMode::Assessment
            ) {
            Box::new(order.iter())
        } else {
            Box::new(order.iter().rev())
        };
        let mut result = Vec::with_capacity(limit.min(4096));
        for index in iterator
            .filter(|index| candidate_set.as_ref().is_none_or(|set| set.contains(index)))
            .copied()
            .filter(|index| file_matches(&files[*index], &normalized, bounds, view))
        {
            if result.len() >= limit {
                return (result, true);
            }
            result.push(index);
        }
        (result, false)
    }

    fn candidates(&self, query: &str) -> Option<Vec<usize>> {
        let keys = trigram_keys(query.as_bytes());
        if keys.is_empty() {
            return None;
        }
        let mut postings = keys
            .into_iter()
            .map(|key| self.trigrams.get(&key).cloned().unwrap_or_default())
            .collect::<Vec<_>>();
        postings.sort_unstable_by_key(Vec::len);
        let mut postings = postings.into_iter();
        let mut result = postings.next().unwrap_or_default();
        for next in postings {
            let allowed = next.into_iter().collect::<HashSet<_>>();
            result.retain(|index| allowed.contains(index));
            if result.is_empty() {
                break;
            }
        }
        Some(result)
    }
}

fn cleanup_order(order: &[usize], files: &[FileEntry]) -> Vec<usize> {
    order
        .iter()
        .copied()
        .filter(|index| files[*index].is_cleanup_candidate())
        .collect()
}

fn trigram_keys(value: &[u8]) -> Vec<u32> {
    value
        .windows(3)
        .map(|part| u32::from(part[0]) << 16 | u32::from(part[1]) << 8 | u32::from(part[2]))
        .collect()
}
