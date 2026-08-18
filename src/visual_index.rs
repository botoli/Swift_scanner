use crate::model::{
    FileEntry, ImageIndexMessage, ImageIndexStats, ImageMatchKind, ImageSearchEngine,
    ImageSearchHit, ImageSearchMessage, ImageSearchMode, ImageSearchRequest, PixelImage,
};
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageDecoder, ImageReader};
use ort::ep;
use ort::session::Session;
use ort::value::Tensor;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use usearch::{Index, IndexOptions, MetricKind, ScalarKind};

const MODEL_DIMENSIONS: usize = 512;
const MODEL_IMAGE_SIZE: u32 = 256;
const ALGORITHM_VERSION: i64 = 1;
const MODEL_VERSION: i64 = 1;
const NEAR_VISUAL_RADIUS: u32 = 10;

struct MobileClipEngine {
    session: Session,
    accelerator: String,
    model_path: PathBuf,
}

#[derive(Clone)]
struct VisualRecord {
    key: u64,
    file_index: usize,
    perceptual_hash: u64,
    path_lower: String,
}

struct VisualIndex {
    ann: Index,
    records: Vec<VisualRecord>,
    record_by_key: HashMap<u64, usize>,
    engine: Mutex<MobileClipEngine>,
    accelerator: String,
}

struct CachedRow {
    id: i64,
    size: i64,
    modified_ns: i64,
    perceptual_hash: u64,
    embedding: Vec<u8>,
    algorithm_version: i64,
    model_version: i64,
}

/// Builds or incrementally refreshes the persistent photo index.
///
/// The application starts this on a worker thread after the regular `FileIndex`
/// is ready. `files` is the existing scan result, so this function never walks
/// the filesystem a second time. Progress and the finished search engine are
/// returned through the typed `ImageIndexMessage` channel.
pub(crate) fn build_image_index(
    files: Arc<Vec<FileEntry>>,
    root: PathBuf,
    sender: Sender<ImageIndexMessage>,
    cancelled: Arc<AtomicBool>,
) {
    let result = build_image_index_inner(&files, &root, &sender, &cancelled);
    match result {
        Ok(Some((engine, stats))) => {
            let _ = sender.send(ImageIndexMessage::Ready(engine, stats));
        }
        Ok(None) => {
            let _ = sender.send(ImageIndexMessage::Cancelled);
        }
        Err(error) => {
            let _ = sender.send(ImageIndexMessage::Failed(error));
        }
    }
}

fn build_image_index_inner(
    files: &[FileEntry],
    root: &Path,
    sender: &Sender<ImageIndexMessage>,
    cancelled: &AtomicBool,
) -> Result<Option<(Arc<dyn ImageSearchEngine>, ImageIndexStats)>, String> {
    let image_indices = files
        .iter()
        .enumerate()
        .filter_map(|(index, file)| is_supported_image(&file.path).then_some(index))
        .collect::<Vec<_>>();
    let mut stats = ImageIndexStats {
        total: image_indices.len() as u64,
        ..Default::default()
    };

    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }

    let model_path = model_path()?;
    let mut clip = MobileClipEngine::open(&model_path)?;
    let mut accelerator = clip.accelerator.clone();
    let _ = sender.send(ImageIndexMessage::Progress(stats, accelerator.clone()));

    let cache_dir = cache_directory(root)?;
    std::fs::create_dir_all(&cache_dir)
        .map_err(|error| format!("Не удалось создать кэш фото: {error}"))?;
    let db_path = cache_dir.join("metadata.sqlite3");
    let ann_path = cache_dir.join("semantic.usearch");
    let mut connection = Connection::open(&db_path)
        .map_err(|error| format!("Не удалось открыть кэш фото: {error}"))?;
    prepare_database(&connection)?;

    let mut ann = new_ann_index()?;
    if ann_path.exists()
        && ann
            .load(&ann_path.to_string_lossy())
            .map_err(|error| error.to_string())
            .is_err()
    {
        ann = rebuild_ann_from_cache(&connection, image_indices.len())?;
    }
    ann.reserve(image_indices.len().max(1))
        .map_err(|error| format!("Не удалось зарезервировать визуальный индекс: {error}"))?;

    let generation = generation_id();
    let transaction = connection
        .transaction()
        .map_err(|error| format!("Не удалось обновить кэш фото: {error}"))?;
    let mut records = Vec::with_capacity(image_indices.len());

    for file_index in image_indices {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let file = &files[file_index];
        let path_text = file.path.to_string_lossy().into_owned();
        let modified_ns = modified_ns(file.modified);
        let cached = read_cached(&transaction, &path_text)?;
        let unchanged = cached.as_ref().is_some_and(|row| {
            row.size == saturating_i64(file.size)
                && row.modified_ns == modified_ns
                && row.algorithm_version == ALGORITHM_VERSION
                && row.model_version == MODEL_VERSION
                && row.embedding.len() == MODEL_DIMENSIONS
        });

        let indexed = if unchanged {
            let row = cached.as_ref().expect("checked above");
            if !ann.contains(row.id as u64) {
                ann.add(row.id as u64, &decode_embedding(&row.embedding))
                    .map_err(|error| format!("Не удалось восстановить запись индекса: {error}"))?;
            }
            transaction
                .execute(
                    "UPDATE images SET seen_generation = ?1 WHERE id = ?2",
                    params![generation, row.id],
                )
                .map_err(|error| format!("Не удалось обновить кэш фото: {error}"))?;
            stats.cached += 1;
            Some((row.id, row.perceptual_hash))
        } else {
            match analyze_image(&file.path, &mut clip) {
                Ok((perceptual_hash, embedding)) => {
                    if accelerator != clip.accelerator {
                        accelerator.clone_from(&clip.accelerator);
                    }
                    let encoded = encode_embedding(&embedding);
                    let id = if let Some(row) = &cached {
                        let _ = ann.remove(row.id as u64);
                        transaction
                            .execute(
                                "UPDATE images SET size = ?1, modified_ns = ?2, perceptual_hash = ?3, embedding = ?4, algorithm_version = ?5, model_version = ?6, seen_generation = ?7 WHERE id = ?8",
                                params![saturating_i64(file.size), modified_ns, perceptual_hash as i64, encoded, ALGORITHM_VERSION, MODEL_VERSION, generation, row.id],
                            )
                            .map_err(|error| format!("Не удалось записать кэш фото: {error}"))?;
                        row.id
                    } else {
                        transaction
                            .execute(
                                "INSERT INTO images(path, size, modified_ns, perceptual_hash, embedding, algorithm_version, model_version, seen_generation) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                                params![path_text, saturating_i64(file.size), modified_ns, perceptual_hash as i64, encoded, ALGORITHM_VERSION, MODEL_VERSION, generation],
                            )
                            .map_err(|error| format!("Не удалось записать кэш фото: {error}"))?;
                        transaction.last_insert_rowid()
                    };
                    ann.add(id as u64, &embedding)
                        .map_err(|error| format!("Не удалось добавить фото в индекс: {error}"))?;
                    stats.processed += 1;
                    Some((id, perceptual_hash))
                }
                Err(_) => {
                    stats.failed += 1;
                    if let Some(row) = cached {
                        let _ = ann.remove(row.id as u64);
                        transaction
                            .execute("DELETE FROM images WHERE id = ?1", params![row.id])
                            .map_err(|error| format!("Не удалось очистить кэш фото: {error}"))?;
                    }
                    None
                }
            }
        };

        if let Some((id, perceptual_hash)) = indexed {
            records.push(VisualRecord {
                key: id as u64,
                file_index,
                perceptual_hash,
                path_lower: file.path_lower.clone(),
            });
        }

        let complete = stats.processed + stats.cached + stats.failed;
        if complete % 16 == 0 || complete == stats.total {
            let _ = sender.send(ImageIndexMessage::Progress(stats, accelerator.clone()));
        }
    }

    let stale_ids = stale_ids(&transaction, generation)?;
    for id in &stale_ids {
        let _ = ann.remove(*id as u64);
    }
    transaction
        .execute(
            "DELETE FROM images WHERE seen_generation <> ?1",
            params![generation],
        )
        .map_err(|error| format!("Не удалось удалить устаревший кэш: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("Не удалось сохранить кэш фото: {error}"))?;

    if !stale_ids.is_empty() && stale_ids.len().saturating_mul(10) > ann.size().max(1) {
        ann.compact()
            .map_err(|error| format!("Не удалось уплотнить визуальный индекс: {error}"))?;
    }
    ann.save(&ann_path.to_string_lossy())
        .map_err(|error| format!("Не удалось сохранить визуальный индекс: {error}"))?;

    let record_by_key = records
        .iter()
        .enumerate()
        .map(|(index, record)| (record.key, index))
        .collect::<HashMap<_, _>>();
    let engine = Arc::new(VisualIndex {
        ann,
        records,
        record_by_key,
        engine: Mutex::new(clip),
        accelerator: accelerator.clone(),
    });
    let _ = sender.send(ImageIndexMessage::Progress(stats, accelerator));
    Ok(Some((engine, stats)))
}

/// Runs one image query and streams thumbnails after the ranked result list.
///
/// `ScannerApp` calls this from a dedicated worker. The function accepts a
/// cancellation flag so changing the reference image cannot leave obsolete
/// decoding work running in the background.
pub(crate) fn run_image_search(
    engine: Arc<dyn ImageSearchEngine>,
    files: Arc<Vec<FileEntry>>,
    request: ImageSearchRequest,
    sender: Sender<ImageSearchMessage>,
    cancelled: Arc<AtomicBool>,
) {
    if cancelled.load(Ordering::Relaxed) {
        let _ = sender.send(ImageSearchMessage::Cancelled);
        return;
    }
    match engine.search(&request) {
        Ok(hits) => {
            let preview = thumbnail(&request.path, 320).ok();
            let _ = sender.send(ImageSearchMessage::Results(hits.clone(), preview));
            for hit in hits.iter().take(36) {
                if cancelled.load(Ordering::Relaxed) {
                    let _ = sender.send(ImageSearchMessage::Cancelled);
                    return;
                }
                if let Some(file) = files.get(hit.file_index)
                    && let Ok(image) = thumbnail(&file.path, 320)
                {
                    let _ = sender.send(ImageSearchMessage::Thumbnail(hit.file_index, image));
                }
            }
            let _ = sender.send(ImageSearchMessage::Finished);
        }
        Err(error) => {
            let _ = sender.send(ImageSearchMessage::Failed(error));
        }
    }
}

impl ImageSearchEngine for VisualIndex {
    fn search(&self, request: &ImageSearchRequest) -> Result<Vec<ImageSearchHit>, String> {
        let query_lower = request.path.to_string_lossy().to_lowercase();
        match request.mode {
            ImageSearchMode::NearVisual => {
                let image = load_oriented(&request.path)?;
                let query_hash = perceptual_hash(&image);
                let mut hits = self
                    .records
                    .iter()
                    .filter(|record| record.path_lower != query_lower)
                    .filter_map(|record| {
                        let distance = (record.perceptual_hash ^ query_hash).count_ones();
                        (distance <= NEAR_VISUAL_RADIUS).then_some(ImageSearchHit {
                            file_index: record.file_index,
                            score: 1.0 - distance as f32 / 64.0,
                            match_kind: ImageMatchKind::NearVisual,
                        })
                    })
                    .collect::<Vec<_>>();
                hits.sort_unstable_by(|left, right| right.score.total_cmp(&left.score));
                hits.truncate(request.limit);
                Ok(hits)
            }
            ImageSearchMode::Semantic => {
                let image = load_oriented(&request.path)?;
                let embedding = self
                    .engine
                    .lock()
                    .map_err(|_| "ML-модель недоступна".to_owned())?
                    .embed(&image)?;
                let matches = self
                    .ann
                    .search(&embedding, request.limit.saturating_add(1))
                    .map_err(|error| format!("Ошибка поиска по визуальному индексу: {error}"))?;
                let mut hits = Vec::with_capacity(request.limit);
                for (key, distance) in matches.keys.into_iter().zip(matches.distances) {
                    let Some(&record_index) = self.record_by_key.get(&key) else {
                        continue;
                    };
                    let record = &self.records[record_index];
                    if record.path_lower == query_lower {
                        continue;
                    }
                    hits.push(ImageSearchHit {
                        file_index: record.file_index,
                        score: (1.0 - distance).clamp(0.0, 1.0),
                        match_kind: ImageMatchKind::Semantic,
                    });
                    if hits.len() == request.limit {
                        break;
                    }
                }
                Ok(hits)
            }
        }
    }

    fn image_count(&self) -> usize {
        self.records.len()
    }

    fn accelerator(&self) -> String {
        self.engine
            .lock()
            .map(|engine| engine.accelerator.clone())
            .unwrap_or_else(|_| self.accelerator.clone())
    }
}

impl MobileClipEngine {
    fn open(path: &Path) -> Result<Self, String> {
        ensure_ort_initialized()?;
        #[cfg(target_os = "windows")]
        {
            let directml: Result<Session, String> = (|| {
                let builder = Session::builder().map_err(|error| error.to_string())?;
                let mut builder = builder
                    .with_execution_providers([ep::DirectML::default().build().error_on_failure()])
                    .map_err(|error| error.to_string())?;
                builder
                    .commit_from_file(path)
                    .map_err(|error| error.to_string())
            })();
            if let Ok(session) = directml {
                return Ok(Self {
                    session,
                    accelerator: "DirectML GPU".to_owned(),
                    model_path: path.to_owned(),
                });
            }
        }

        let session = Session::builder()
            .and_then(|mut builder| builder.commit_from_file(path))
            .map_err(|error| format!("Не удалось загрузить MobileCLIP2-S0: {error}"))?;
        Ok(Self {
            session,
            accelerator: "CPU fallback".to_owned(),
            model_path: path.to_owned(),
        })
    }

    fn embed(&mut self, image: &DynamicImage) -> Result<Vec<f32>, String> {
        match Self::run_session(&mut self.session, image) {
            Ok(embedding) => Ok(embedding),
            Err(gpu_error) if self.accelerator == "DirectML GPU" => {
                let mut session = Session::builder()
                    .and_then(|mut builder| builder.commit_from_file(&self.model_path))
                    .map_err(|cpu_error| {
                        format!(
                            "DirectML недоступен ({gpu_error}); не удалось включить CPU fallback: {cpu_error}"
                        )
                    })?;
                let embedding = Self::run_session(&mut session, image).map_err(|cpu_error| {
                    format!("DirectML недоступен ({gpu_error}); ошибка CPU fallback: {cpu_error}")
                })?;
                self.session = session;
                self.accelerator = "CPU fallback".to_owned();
                Ok(embedding)
            }
            Err(error) => Err(error),
        }
    }

    fn run_session(session: &mut Session, image: &DynamicImage) -> Result<Vec<f32>, String> {
        let input = model_input(image);
        let tensor = Tensor::from_array(([1usize, 3, 256, 256], input.into_boxed_slice()))
            .map_err(|error| format!("Не удалось подготовить ML-вход: {error}"))?;
        let outputs = session
            .run(ort::inputs!["pixel_values" => tensor])
            .map_err(|error| format!("Ошибка MobileCLIP inference: {error}"))?;
        let output = outputs.get("image_embeds").unwrap_or_else(|| &outputs[0]);
        let (_, values) = output
            .try_extract_tensor::<f32>()
            .map_err(|error| format!("Некорректный результат MobileCLIP: {error}"))?;
        if values.len() != MODEL_DIMENSIONS {
            return Err(format!(
                "MobileCLIP вернул {} значений вместо {MODEL_DIMENSIONS}",
                values.len()
            ));
        }
        let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
        if !norm.is_finite() || norm <= f32::EPSILON {
            return Err("MobileCLIP вернул пустой вектор".to_owned());
        }
        Ok(values.iter().map(|value| value / norm).collect())
    }
}

fn analyze_image(path: &Path, clip: &mut MobileClipEngine) -> Result<(u64, Vec<f32>), String> {
    let image = load_oriented(path)?;
    let hash = perceptual_hash(&image);
    let embedding = clip.embed(&image)?;
    Ok((hash, embedding))
}

fn load_oriented(path: &Path) -> Result<DynamicImage, String> {
    let reader = ImageReader::open(path)
        .map_err(|error| format!("Не удалось открыть изображение: {error}"))?
        .with_guessed_format()
        .map_err(|error| format!("Не удалось определить формат изображения: {error}"))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|error| format!("Не удалось создать декодер изображения: {error}"))?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder)
        .map_err(|error| format!("Не удалось декодировать изображение: {error}"))?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn model_input(image: &DynamicImage) -> Vec<f32> {
    let (width, height) = image.dimensions();
    let scale = MODEL_IMAGE_SIZE as f32 / width.min(height).max(1) as f32;
    let resized_width = ((width as f32 * scale).round() as u32).max(MODEL_IMAGE_SIZE);
    let resized_height = ((height as f32 * scale).round() as u32).max(MODEL_IMAGE_SIZE);
    let resized = image.resize_exact(resized_width, resized_height, FilterType::CatmullRom);
    let x = (resized_width - MODEL_IMAGE_SIZE) / 2;
    let y = (resized_height - MODEL_IMAGE_SIZE) / 2;
    let rgb = resized
        .crop_imm(x, y, MODEL_IMAGE_SIZE, MODEL_IMAGE_SIZE)
        .to_rgb8();
    let plane = (MODEL_IMAGE_SIZE * MODEL_IMAGE_SIZE) as usize;
    let mut input = vec![0.0_f32; plane * 3];
    for (index, pixel) in rgb.pixels().enumerate() {
        input[index] = pixel[0] as f32 / 255.0;
        input[plane + index] = pixel[1] as f32 / 255.0;
        input[plane * 2 + index] = pixel[2] as f32 / 255.0;
    }
    input
}

fn perceptual_hash(image: &DynamicImage) -> u64 {
    const SIZE: usize = 32;
    const LOW: usize = 8;
    static DCT_BASIS: OnceLock<[[f32; SIZE]; LOW]> = OnceLock::new();
    let basis = DCT_BASIS.get_or_init(|| {
        let mut basis = [[0.0_f32; SIZE]; LOW];
        for frequency in 0..LOW {
            for position in 0..SIZE {
                basis[frequency][position] =
                    ((2 * position + 1) as f32 * frequency as f32 * std::f32::consts::PI
                        / (2 * SIZE) as f32)
                        .cos();
            }
        }
        basis
    });
    let grayscale = image
        .resize_exact(SIZE as u32, SIZE as u32, FilterType::CatmullRom)
        .to_luma8();
    let mut coefficients = [0.0_f32; LOW * LOW];
    for v in 0..LOW {
        for u in 0..LOW {
            let mut sum = 0.0_f32;
            for y in 0..SIZE {
                let cy = basis[v][y];
                for x in 0..SIZE {
                    let cx = basis[u][x];
                    sum += grayscale.get_pixel(x as u32, y as u32)[0] as f32 * cx * cy;
                }
            }
            coefficients[v * LOW + u] = sum;
        }
    }
    let mut values = coefficients[1..].to_vec();
    values.sort_unstable_by(f32::total_cmp);
    let median = values[values.len() / 2];
    coefficients
        .iter()
        .enumerate()
        .fold(0_u64, |hash, (index, value)| {
            hash | (u64::from(*value > median) << index)
        })
}

pub(crate) fn thumbnail(path: &Path, maximum: u32) -> Result<PixelImage, String> {
    let image = load_oriented(path)?;
    let rgba = image.thumbnail(maximum, maximum).to_rgba8();
    Ok(PixelImage {
        width: rgba.width() as usize,
        height: rgba.height() as usize,
        rgba: rgba.into_raw(),
    })
}

pub(crate) fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "webp"
            )
        })
}

fn new_ann_index() -> Result<Index, String> {
    Index::new(&IndexOptions {
        dimensions: MODEL_DIMENSIONS,
        metric: MetricKind::Cos,
        quantization: ScalarKind::I8,
        connectivity: 16,
        expansion_add: 128,
        expansion_search: 64,
        ..Default::default()
    })
    .map_err(|error| format!("Не удалось создать визуальный индекс: {error}"))
}

fn rebuild_ann_from_cache(connection: &Connection, capacity: usize) -> Result<Index, String> {
    let index = new_ann_index()?;
    index
        .reserve(capacity.max(1))
        .map_err(|error| format!("Не удалось восстановить визуальный индекс: {error}"))?;
    let mut statement = connection
        .prepare("SELECT id, embedding FROM images WHERE model_version = ?1")
        .map_err(|error| format!("Не удалось прочитать кэш фото: {error}"))?;
    let rows = statement
        .query_map(params![MODEL_VERSION], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|error| format!("Не удалось прочитать кэш фото: {error}"))?;
    for row in rows {
        let (id, embedding) = row.map_err(|error| format!("Повреждён кэш фото: {error}"))?;
        if embedding.len() == MODEL_DIMENSIONS {
            index
                .add(id as u64, &decode_embedding(&embedding))
                .map_err(|error| format!("Не удалось восстановить визуальный индекс: {error}"))?;
        }
    }
    Ok(index)
}

fn prepare_database(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS images (
                 id INTEGER PRIMARY KEY,
                 path TEXT NOT NULL UNIQUE,
                 size INTEGER NOT NULL,
                 modified_ns INTEGER NOT NULL,
                 perceptual_hash INTEGER NOT NULL,
                 embedding BLOB NOT NULL,
                 algorithm_version INTEGER NOT NULL,
                 model_version INTEGER NOT NULL,
                 seen_generation INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS images_seen ON images(seen_generation);",
        )
        .map_err(|error| format!("Не удалось подготовить кэш фото: {error}"))
}

fn read_cached(transaction: &Transaction<'_>, path: &str) -> Result<Option<CachedRow>, String> {
    transaction
        .query_row(
            "SELECT id, size, modified_ns, perceptual_hash, embedding, algorithm_version, model_version FROM images WHERE path = ?1",
            params![path],
            |row| {
                Ok(CachedRow {
                    id: row.get(0)?,
                    size: row.get(1)?,
                    modified_ns: row.get(2)?,
                    perceptual_hash: row.get::<_, i64>(3)? as u64,
                    embedding: row.get(4)?,
                    algorithm_version: row.get(5)?,
                    model_version: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|error| format!("Не удалось прочитать кэш фото: {error}"))
}

fn stale_ids(transaction: &Transaction<'_>, generation: i64) -> Result<Vec<i64>, String> {
    let mut statement = transaction
        .prepare("SELECT id FROM images WHERE seen_generation <> ?1")
        .map_err(|error| format!("Не удалось проверить кэш фото: {error}"))?;
    let rows = statement
        .query_map(params![generation], |row| row.get::<_, i64>(0))
        .map_err(|error| format!("Не удалось проверить кэш фото: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Не удалось проверить кэш фото: {error}"))
}

fn encode_embedding(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .map(|value| (value.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8)
        .collect()
}

fn decode_embedding(values: &[u8]) -> Vec<f32> {
    values
        .iter()
        .map(|value| *value as i8 as f32 / 127.0)
        .collect()
}

fn cache_directory(root: &Path) -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("SwiftScan")
        .join("image-index");
    let mut hasher = DefaultHasher::new();
    root.to_string_lossy().to_lowercase().hash(&mut hasher);
    Ok(base.join(format!("{:016x}", hasher.finish())))
}

fn model_path() -> Result<PathBuf, String> {
    let file_name = "mobileclip2_s0_vision.onnx";
    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        candidates.push(directory.join("assets").join(file_name));
        candidates.push(directory.join(file_name));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(file_name),
    );
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            "Не найдена модель assets/mobileclip2_s0_vision.onnx. Поиск визуальных копий и по содержимому недоступен."
                .to_owned()
        })
}

fn ensure_ort_initialized() -> Result<(), String> {
    static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();
    match INITIALIZED.get_or_init(|| {
        let runtime = runtime_path()?;
        ort::init_from(&runtime)
            .map_err(|error| format!("Не удалось загрузить ONNX Runtime: {error}"))?
            .commit();
        Ok(())
    }) {
        Ok(()) => Ok(()),
        Err(error) => Err(error.clone()),
    }
}

fn runtime_path() -> Result<PathBuf, String> {
    let file_name = if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };
    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        candidates.push(directory.join("assets").join("runtime").join(file_name));
        candidates.push(directory.join("runtime").join(file_name));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("runtime")
            .join(file_name),
    );
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| format!("Не найден assets/runtime/{file_name}"))
}

fn modified_ns(modified: Option<SystemTime>) -> i64 {
    modified
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

fn generation_id() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(1)
}

fn saturating_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

#[cfg(test)]
pub(crate) fn perceptual_hash_for_test(image: &DynamicImage) -> u64 {
    perceptual_hash(image)
}

#[cfg(test)]
pub(crate) fn model_input_for_test(image: &DynamicImage) -> Vec<f32> {
    model_input(image)
}

#[cfg(test)]
pub(crate) fn semantic_embedding_for_test(image: &DynamicImage) -> Result<Vec<f32>, String> {
    let mut engine = MobileClipEngine::open(&model_path()?)?;
    engine.embed(image)
}
