use crate::model::{FileEntry, ScanMessage, ScanStats};
use std::collections::{HashMap, HashSet, VecDeque};
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc::Sender};
use std::time::{Duration, Instant, SystemTime};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_HANDLE_EOF, FILETIME, GENERIC_READ, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic,
    FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, GetDiskFreeSpaceExW, OPEN_EXISTING,
    WIN32_FIND_DATAW,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FSCTL_ENUM_USN_DATA, FSCTL_QUERY_USN_JOURNAL, MFT_ENUM_DATA_V0, USN_JOURNAL_DATA_V0,
    USN_RECORD_V2,
};

const BUFFER_SIZE: usize = 1024 * 1024;

struct VolumeHandle(HANDLE);

impl Drop for VolumeHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[derive(Clone)]
struct MftNode {
    parent: u64,
    name: String,
    directory: bool,
}

struct FindHandle(HANDLE);

impl Drop for FindHandle {
    fn drop(&mut self) {
        unsafe {
            FindClose(self.0);
        }
    }
}

struct DirectoryCandidate {
    path: PathBuf,
    key: String,
}

struct DirectoryWork {
    directories: VecDeque<PathBuf>,
    pending: usize,
    visited: HashSet<String>,
}

struct FastStats {
    files: AtomicU64,
    bytes: AtomicU64,
    skipped: AtomicU64,
}

pub(super) fn total_capacity(path: &Path) -> Option<u64> {
    let absolute = std::path::absolute(path).ok()?;
    let wide_path = absolute
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut total_bytes = 0_u64;
    let succeeded = unsafe {
        GetDiskFreeSpaceExW(wide_path.as_ptr(), null_mut(), &mut total_bytes, null_mut())
    };
    (succeeded != 0).then_some(total_bytes)
}

pub(super) fn scan_fast(
    root: &Path,
    sender: &Sender<ScanMessage>,
    cancelled: &Arc<AtomicBool>,
) -> Result<(), String> {
    let scan_time = SystemTime::now();
    let result_sender = Arc::new(Mutex::new(sender.clone()));
    let stats = Arc::new(FastStats {
        files: AtomicU64::new(0),
        bytes: AtomicU64::new(0),
        skipped: AtomicU64::new(0),
    });
    let mut root_batch = Vec::with_capacity(512);
    let mut root_update = Instant::now();
    let candidates = scan_windows_folder(
        root,
        &result_sender,
        &stats,
        cancelled,
        scan_time,
        &mut root_batch,
        &mut root_update,
    )?;
    emit_batch(&result_sender, &stats, &mut root_batch, cancelled);

    let mut initial_work = DirectoryWork {
        directories: VecDeque::new(),
        pending: 0,
        visited: HashSet::from([directory_key(root)]),
    };
    register_directories(&mut initial_work, candidates);
    let queue = Arc::new((Mutex::new(initial_work), Condvar::new()));
    let workers = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .saturating_mul(4)
        .clamp(8, 32);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let queue = Arc::clone(&queue);
            let result_sender = Arc::clone(&result_sender);
            let stats = Arc::clone(&stats);
            let cancelled = Arc::clone(cancelled);
            scope.spawn(move || {
                let mut batch = Vec::with_capacity(512);
                let mut last_update = Instant::now();
                loop {
                    let Some(directory) = take_directory(&queue, &cancelled) else {
                        break;
                    };
                    let candidates = match scan_windows_folder(
                        &directory,
                        &result_sender,
                        &stats,
                        &cancelled,
                        scan_time,
                        &mut batch,
                        &mut last_update,
                    ) {
                        Ok(candidates) => candidates,
                        Err(_) => {
                            stats.skipped.fetch_add(1, Ordering::Relaxed);
                            Vec::new()
                        }
                    };
                    complete_directory(&queue, candidates);
                }
                emit_batch(&result_sender, &stats, &mut batch, &cancelled);
            });
        }
    });

    let final_stats = fast_stats(&stats);
    if let Ok(sender) = result_sender.lock() {
        let _ = sender.send(ScanMessage::Finished(
            final_stats,
            cancelled.load(Ordering::Relaxed),
        ));
    }
    Ok(())
}

fn take_directory(
    queue: &(Mutex<DirectoryWork>, Condvar),
    cancelled: &AtomicBool,
) -> Option<PathBuf> {
    let (work, wake) = queue;
    let mut work = work.lock().ok()?;
    loop {
        if cancelled.load(Ordering::Relaxed) || work.pending == 0 {
            return None;
        }
        if let Some(directory) = work.directories.pop_front() {
            return Some(directory);
        }
        let (next, _) = wake.wait_timeout(work, Duration::from_millis(25)).ok()?;
        work = next;
    }
}

fn complete_directory(
    queue: &(Mutex<DirectoryWork>, Condvar),
    candidates: Vec<DirectoryCandidate>,
) {
    let (work, wake) = queue;
    if let Ok(mut work) = work.lock() {
        register_directories(&mut work, candidates);
        work.pending = work.pending.saturating_sub(1);
        wake.notify_all();
    }
}

fn register_directories(work: &mut DirectoryWork, candidates: Vec<DirectoryCandidate>) {
    for candidate in candidates {
        if work.visited.insert(candidate.key) {
            work.pending += 1;
            work.directories.push_back(candidate.path);
        }
    }
}

fn scan_windows_folder(
    directory: &Path,
    sender: &Mutex<Sender<ScanMessage>>,
    stats: &FastStats,
    cancelled: &AtomicBool,
    scan_time: SystemTime,
    batch: &mut Vec<FileEntry>,
    last_update: &mut Instant,
) -> Result<Vec<DirectoryCandidate>, String> {
    let pattern = windows_search_pattern(directory);
    let mut find_data = std::mem::MaybeUninit::<WIN32_FIND_DATAW>::zeroed();
    let mut handle = unsafe {
        FindFirstFileExW(
            pattern.as_ptr(),
            FindExInfoBasic,
            find_data.as_mut_ptr().cast(),
            FindExSearchNameMatch,
            null(),
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        handle = unsafe {
            FindFirstFileExW(
                pattern.as_ptr(),
                FindExInfoBasic,
                find_data.as_mut_ptr().cast(),
                FindExSearchNameMatch,
                null(),
                0,
            )
        };
    }
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!("Каталог недоступен (Win32 {})", unsafe {
            GetLastError()
        }));
    }
    let handle = FindHandle(handle);
    let mut find_data = unsafe { find_data.assume_init() };
    let mut directories = Vec::new();

    loop {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        let name_length = find_data
            .cFileName
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(find_data.cFileName.len());
        let name = std::ffi::OsString::from_wide(&find_data.cFileName[..name_length]);
        if name != "." && name != ".." {
            let attributes = find_data.dwFileAttributes;
            let path = directory.join(&name);
            if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                let reparse = attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
                let link_like =
                    reparse && matches!(find_data.dwReserved0, 0xA000_0003 | 0xA000_000C);
                if !link_like {
                    let resolved = if reparse {
                        std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone())
                    } else {
                        path.clone()
                    };
                    directories.push(DirectoryCandidate {
                        path,
                        key: directory_key(&resolved),
                    });
                }
            } else {
                let size = ((find_data.nFileSizeHigh as u64) << 32) | find_data.nFileSizeLow as u64;
                batch.push(FileEntry::new_at(
                    path,
                    size,
                    filetime_to_system_time(find_data.ftLastWriteTime),
                    scan_time,
                ));
                if batch.len() >= 512 || last_update.elapsed() >= Duration::from_millis(200) {
                    emit_batch(sender, stats, batch, cancelled);
                    *last_update = Instant::now();
                }
            }
        }
        if unsafe { FindNextFileW(handle.0, &mut find_data) } == 0 {
            break;
        }
    }
    Ok(directories)
}

fn emit_batch(
    sender: &Mutex<Sender<ScanMessage>>,
    stats: &FastStats,
    batch: &mut Vec<FileEntry>,
    cancelled: &AtomicBool,
) {
    if batch.is_empty() {
        return;
    }
    let added_files = batch.len() as u64;
    let added_bytes = batch
        .iter()
        .fold(0_u64, |total, entry| total.saturating_add(entry.size));
    stats.files.fetch_add(added_files, Ordering::Relaxed);
    stats.bytes.fetch_add(added_bytes, Ordering::Relaxed);
    let outgoing = std::mem::replace(batch, Vec::with_capacity(512));
    let sent = sender.lock().ok().is_some_and(|sender| {
        sender
            .send(ScanMessage::Batch(outgoing, fast_stats(stats)))
            .is_ok()
    });
    if !sent {
        cancelled.store(true, Ordering::Relaxed);
    }
}

fn fast_stats(stats: &FastStats) -> ScanStats {
    ScanStats {
        files: stats.files.load(Ordering::Relaxed),
        bytes: stats.bytes.load(Ordering::Relaxed),
        skipped: stats.skipped.load(Ordering::Relaxed),
    }
}

fn directory_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn windows_search_pattern(directory: &Path) -> Vec<u16> {
    let absolute = std::path::absolute(directory).unwrap_or_else(|_| directory.to_path_buf());
    let raw: Vec<u16> = absolute.as_os_str().encode_wide().collect();
    let mut pattern: Vec<u16> = if raw.starts_with(&['\\' as u16, '\\' as u16]) {
        r"\\?\UNC\"
            .encode_utf16()
            .chain(raw.into_iter().skip(2))
            .collect()
    } else if raw.starts_with(&['\\' as u16, '\\' as u16, '?' as u16, '\\' as u16]) {
        raw
    } else {
        r"\\?\".encode_utf16().chain(raw).collect()
    };
    if !pattern.ends_with(&['\\' as u16]) {
        pattern.push('\\' as u16);
    }
    pattern.extend(['*' as u16, 0]);
    pattern
}

fn filetime_to_system_time(value: FILETIME) -> Option<SystemTime> {
    const WINDOWS_TO_UNIX_EPOCH: u64 = 116_444_736_000_000_000;
    let ticks = ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64;
    let unix_ticks = ticks.checked_sub(WINDOWS_TO_UNIX_EPOCH)?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::new(
        unix_ticks / 10_000_000,
        ((unix_ticks % 10_000_000) * 100) as u32,
    ))
}

pub(super) fn scan_ntfs(
    root: &Path,
    sender: &Sender<ScanMessage>,
    cancelled: &Arc<AtomicBool>,
) -> Result<(), String> {
    let drive = drive_letter(root)
        .ok_or_else(|| "Выбранный путь не находится на локальном томе".to_owned())?;
    let volume = open_volume(drive)?;
    let before = query_journal(volume.0)?;
    let mut nodes = HashMap::new();
    enumerate_mft(volume.0, 0, i64::MAX, &mut nodes, cancelled)?;
    let after = query_journal(volume.0)?;
    if after.UsnJournalID == before.UsnJournalID && after.NextUsn > before.NextUsn {
        enumerate_mft(
            volume.0,
            before.NextUsn,
            after.NextUsn,
            &mut nodes,
            cancelled,
        )?;
    }
    if cancelled.load(Ordering::Relaxed) {
        let _ = sender.send(ScanMessage::Finished(ScanStats::default(), true));
        return Ok(());
    }

    let volume_root = PathBuf::from(format!("{drive}:\\"));
    let selected = root
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_lowercase();
    let mut cache = HashMap::new();
    let mut paths = Vec::new();
    for (&reference, node) in &nodes {
        if node.directory {
            continue;
        }
        if let Some(path) = resolve_path(reference, &nodes, &volume_root, &mut cache, 0) {
            let normalized = path.to_string_lossy().to_lowercase();
            if normalized == selected || normalized.starts_with(&(selected.clone() + "\\")) {
                paths.push(path);
            }
        }
    }
    hydrate_metadata(paths, sender, cancelled);
    Ok(())
}

fn drive_letter(path: &Path) -> Option<char> {
    let value = path.to_string_lossy();
    let mut chars = value.chars();
    let drive = chars.next()?.to_ascii_uppercase();
    (chars.next()? == ':' && drive.is_ascii_alphabetic()).then_some(drive)
}

fn open_volume(drive: char) -> Result<VolumeHandle, String> {
    let wide = format!(r"\\.\{drive}:")
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(format!("MFT недоступна (Win32 {})", unsafe {
            GetLastError()
        }))
    } else {
        Ok(VolumeHandle(handle))
    }
}

fn query_journal(handle: HANDLE) -> Result<USN_JOURNAL_DATA_V0, String> {
    let mut data: USN_JOURNAL_DATA_V0 = unsafe { zeroed() };
    let mut returned = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_QUERY_USN_JOURNAL,
            null(),
            0,
            (&mut data as *mut USN_JOURNAL_DATA_V0).cast(),
            size_of::<USN_JOURNAL_DATA_V0>() as u32,
            &mut returned,
            null_mut(),
        )
    };
    if ok == 0 {
        Err(format!("USN Journal недоступен (Win32 {})", unsafe {
            GetLastError()
        }))
    } else {
        Ok(data)
    }
}

fn enumerate_mft(
    handle: HANDLE,
    low_usn: i64,
    high_usn: i64,
    nodes: &mut HashMap<u64, MftNode>,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let mut input = MFT_ENUM_DATA_V0 {
        StartFileReferenceNumber: 0,
        LowUsn: low_usn,
        HighUsn: high_usn,
    };
    let mut output = vec![0_u8; BUFFER_SIZE];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut returned = 0_u32;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_ENUM_USN_DATA,
                (&input as *const MFT_ENUM_DATA_V0).cast(),
                size_of::<MFT_ENUM_DATA_V0>() as u32,
                output.as_mut_ptr().cast(),
                output.len() as u32,
                &mut returned,
                null_mut(),
            )
        };
        if ok == 0 {
            let error = unsafe { GetLastError() };
            if error == ERROR_HANDLE_EOF {
                break;
            }
            return Err(format!("Ошибка чтения MFT (Win32 {error})"));
        }
        if returned < 8 {
            break;
        }
        input.StartFileReferenceNumber = u64::from_ne_bytes(output[..8].try_into().unwrap());
        let mut offset = 8_usize;
        while offset + size_of::<USN_RECORD_V2>() <= returned as usize {
            let record =
                unsafe { (output.as_ptr().add(offset) as *const USN_RECORD_V2).read_unaligned() };
            if record.RecordLength == 0 || offset + record.RecordLength as usize > returned as usize
            {
                break;
            }
            if record.MajorVersion == 2 {
                let name_start = offset + record.FileNameOffset as usize;
                let name_units = record.FileNameLength as usize / 2;
                if name_start + name_units * 2 <= returned as usize {
                    let name = unsafe {
                        String::from_utf16_lossy(std::slice::from_raw_parts(
                            output.as_ptr().add(name_start) as *const u16,
                            name_units,
                        ))
                    };
                    nodes.insert(
                        record.FileReferenceNumber,
                        MftNode {
                            parent: record.ParentFileReferenceNumber,
                            name,
                            directory: record.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
                        },
                    );
                }
            }
            offset += record.RecordLength as usize;
        }
    }
    Ok(())
}

fn resolve_path(
    reference: u64,
    nodes: &HashMap<u64, MftNode>,
    volume_root: &Path,
    cache: &mut HashMap<u64, PathBuf>,
    depth: usize,
) -> Option<PathBuf> {
    if let Some(path) = cache.get(&reference) {
        return Some(path.clone());
    }
    if depth > 256 {
        return None;
    }
    let node = nodes.get(&reference)?;
    let path =
        if reference & 0x0000_FFFF_FFFF_FFFF == 5 || node.parent == reference || node.name == "." {
            volume_root.to_path_buf()
        } else {
            resolve_path(node.parent, nodes, volume_root, cache, depth + 1)?.join(&node.name)
        };
    cache.insert(reference, path.clone());
    Some(path)
}

fn hydrate_metadata(
    paths: Vec<PathBuf>,
    sender: &Sender<ScanMessage>,
    cancelled: &Arc<AtomicBool>,
) {
    let scan_time = SystemTime::now();
    let paths = Arc::new(paths);
    let next = Arc::new(AtomicUsize::new(0));
    let workers = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .clamp(2, 12);
    let (entry_sender, entry_receiver) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let paths = Arc::clone(&paths);
            let next = Arc::clone(&next);
            let cancelled = Arc::clone(cancelled);
            let entry_sender = entry_sender.clone();
            scope.spawn(move || {
                loop {
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index) else {
                        break;
                    };
                    let result = std::fs::metadata(path).map(|metadata| {
                        FileEntry::new_at(
                            path.clone(),
                            metadata.len(),
                            metadata.modified().ok(),
                            scan_time,
                        )
                    });
                    if entry_sender.send(result).is_err() {
                        break;
                    }
                }
            });
        }
        drop(entry_sender);
        let mut stats = ScanStats::default();
        let mut batch = Vec::with_capacity(512);
        for result in entry_receiver {
            match result {
                Ok(entry) => {
                    stats.files += 1;
                    stats.bytes = stats.bytes.saturating_add(entry.size);
                    batch.push(entry);
                    if batch.len() >= 512 {
                        if sender
                            .send(ScanMessage::Batch(std::mem::take(&mut batch), stats))
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                Err(_) => stats.skipped += 1,
            }
        }
        if !batch.is_empty() {
            let _ = sender.send(ScanMessage::Batch(batch, stats));
        }
        let _ = sender.send(ScanMessage::Finished(
            stats,
            cancelled.load(Ordering::Relaxed),
        ));
    });
}
