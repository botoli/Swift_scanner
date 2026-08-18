use crate::CLEANUP_SCORE_THRESHOLD;
use crate::app::read_saved_root;
use crate::filtering::{file_matches, file_matches_category};
use crate::index::FileIndex;
use crate::model::{CleanupCategory, FileEntry, SortMode, SortRule, ViewMode};
use crate::ui::format_file_age;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn file_filter_combines_query_and_size_bounds() {
    let file = FileEntry::new(
        PathBuf::from(r"C:\Users\Test\Cache\video.tmp"),
        150 * 1024 * 1024,
        None,
    );
    assert!(file_matches(
        &file,
        "cache",
        (100 * 1024 * 1024, 200 * 1024 * 1024),
        ViewMode::All,
    ));
    assert!(!file_matches(&file, "photo", (0, u64::MAX), ViewMode::All));
    assert!(file_matches(&file, "", (0, u64::MAX), ViewMode::Cleanup));
}

#[test]
fn dot_query_matches_file_extension_not_directory_name() {
    let image_in_dotted_directory =
        FileEntry::new(PathBuf::from(r"C:\Images\.iso\photo.jpg"), 1024, None);
    let disk_image = FileEntry::new(PathBuf::from(r"C:\Images\archive.ISO"), 1024, None);

    assert!(!file_matches(
        &image_in_dotted_directory,
        ".iso",
        (0, u64::MAX),
        ViewMode::All,
    ));
    assert!(file_matches(
        &disk_image,
        ".iso",
        (0, u64::MAX),
        ViewMode::All,
    ));
}

#[test]
fn cleanup_rating_protects_system_files_and_prioritizes_cache() {
    let protected = FileEntry::new(PathBuf::from(r"C:\Windows\System32\driver.sys"), 10, None);
    let cache = FileEntry::new(
        PathBuf::from(r"C:\Users\Test\AppData\Local\Cache\archive.tmp"),
        200 * 1024 * 1024,
        None,
    );
    assert_eq!(protected.cleanup_score(), 0);
    assert!(!protected.is_cleanup_candidate());
    assert!(cache.cleanup_score() >= CLEANUP_SCORE_THRESHOLD);
}

#[test]
fn own_index_finds_path_substrings_and_keeps_size_order() {
    let files = vec![
        FileEntry::new(PathBuf::from(r"C:\Data\reports\small.log"), 10, None),
        FileEntry::new(PathBuf::from(r"C:\Data\reports\large.log"), 1_000, None),
        FileEntry::new(PathBuf::from(r"C:\Data\photo.jpg"), 2_000, None),
    ];
    let index = FileIndex::build(&files);
    let (first_page, has_more) = index.query_page(
        &files,
        "reports",
        (0, u64::MAX),
        ViewMode::All,
        None,
        SortRule {
            mode: SortMode::Size,
            descending: true,
        },
        1,
    );
    assert_eq!(first_page, vec![1]);
    assert!(has_more);

    let (result, has_more) = index.query_page(
        &files,
        "reports",
        (0, u64::MAX),
        ViewMode::All,
        None,
        SortRule {
            mode: SortMode::Size,
            descending: true,
        },
        50,
    );
    assert_eq!(result, vec![1, 0]);
    assert!(!has_more);
}

#[test]
fn category_filter_matches_only_cleanup_candidates() {
    let cache = FileEntry::new(
        PathBuf::from(r"C:\Users\Test\Cache\preview.bin"),
        1024,
        None,
    );
    let ordinary = FileEntry::new(PathBuf::from(r"C:\Data\notes.txt"), 1024, None);

    assert!(file_matches_category(&cache, None));
    assert!(file_matches_category(&cache, Some(CleanupCategory::Cache)));
    assert!(!file_matches_category(
        &cache,
        Some(CleanupCategory::Temporary)
    ));
    assert!(!file_matches_category(
        &ordinary,
        Some(CleanupCategory::Ordinary)
    ));
}

#[test]
fn own_index_filters_cleanup_category() {
    let files = vec![
        FileEntry::new(PathBuf::from(r"C:\Data\Cache\large.bin"), 2_000, None),
        FileEntry::new(PathBuf::from(r"C:\Data\Temp\small.tmp"), 1_000, None),
    ];
    let index = FileIndex::build(&files);
    let (result, has_more) = index.query_page(
        &files,
        "",
        (0, u64::MAX),
        ViewMode::Cleanup,
        Some(CleanupCategory::Cache),
        SortRule {
            mode: SortMode::Size,
            descending: true,
        },
        50,
    );

    assert_eq!(result, vec![0]);
    assert!(!has_more);
}

#[test]
fn saved_root_is_used_only_while_directory_exists() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must be after Unix epoch")
        .as_nanos();
    let base = std::env::temp_dir().join(format!("swiftscan-root-test-{unique}"));
    let root = base.join("scan-root");
    let config = base.join("last_root.txt");
    std::fs::create_dir_all(&root).expect("test root must be created");
    std::fs::write(&config, root.to_string_lossy().as_bytes())
        .expect("test config must be written");

    assert_eq!(read_saved_root(&config), Some(root.clone()));
    std::fs::remove_dir_all(&root).expect("test root must be removed");
    assert_eq!(read_saved_root(&config), None);
    std::fs::remove_dir_all(&base).expect("test directory must be removed");
}

#[test]
fn file_age_uses_readable_russian_units() {
    assert_eq!(format_file_age(None), "возраст неизвестен");
    assert_eq!(
        format_file_age(Some(SystemTime::now() - Duration::from_secs(2 * 86_400))),
        "2 дня назад"
    );
}
