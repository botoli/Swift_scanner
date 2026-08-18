use crate::CLEANUP_SCORE_THRESHOLD;
use crate::filtering::{file_matches, file_matches_category};
use crate::index::FileIndex;
use crate::model::{CleanupCategory, FileEntry, SortMode, SortRule, ViewMode};
use crate::visual_index::{
    is_supported_image, model_input_for_test, perceptual_hash_for_test, semantic_embedding_for_test,
};
use image::{DynamicImage, ImageBuffer, Rgb};
use std::path::PathBuf;

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

fn patterned_image(width: u32, height: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(ImageBuffer::from_fn(width, height, |x, y| {
        let cell = ((x / 8) + (y / 8)) % 2;
        if cell == 0 {
            Rgb([230, (x % 255) as u8, 35])
        } else {
            Rgb([20, 80, (y % 255) as u8])
        }
    }))
}

#[test]
fn visual_index_accepts_only_v1_image_formats() {
    assert!(is_supported_image(&PathBuf::from("photo.JPEG")));
    assert!(is_supported_image(&PathBuf::from("photo.png")));
    assert!(is_supported_image(&PathBuf::from("photo.webp")));
    assert!(!is_supported_image(&PathBuf::from("photo.heic")));
    assert!(!is_supported_image(&PathBuf::from("notes.txt")));
}

#[test]
fn perceptual_hash_survives_resize() {
    let source = patterned_image(96, 64);
    let resized = source.resize_exact(384, 256, image::imageops::FilterType::Lanczos3);
    let distance =
        (perceptual_hash_for_test(&source) ^ perceptual_hash_for_test(&resized)).count_ones();
    assert!(distance <= 2, "unexpected Hamming distance: {distance}");
}

#[test]
fn mobileclip_input_has_expected_shape_and_range() {
    let input = model_input_for_test(&patterned_image(96, 64));
    assert_eq!(input.len(), 3 * 256 * 256);
    assert!(input.iter().all(|value| value.is_finite()));
    assert!(input.iter().all(|value| (0.0..=1.0).contains(value)));
}

#[test]
fn bundled_mobileclip_returns_normalized_embedding() {
    let embedding = semantic_embedding_for_test(&patterned_image(96, 64))
        .expect("bundled MobileCLIP model must run");
    assert_eq!(embedding.len(), 512);
    let norm = embedding
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    assert!((norm - 1.0).abs() < 1e-4, "embedding norm: {norm}");
}
