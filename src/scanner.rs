use crate::model::{FileEntry, ScanMessage, ScanStats};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc::Sender};
use std::time::SystemTime;

#[cfg(windows)]
mod windows_ntfs;

pub(crate) fn scan_directory(
    root: PathBuf,
    sender: Sender<ScanMessage>,
    cancelled: Arc<AtomicBool>,
) {
    #[cfg(windows)]
    if windows_ntfs::scan_fast(&root, &sender, &cancelled).is_ok() {
        return;
    }

    #[cfg(windows)]
    if windows_ntfs::scan_ntfs(&root, &sender, &cancelled).is_ok() {
        return;
    }

    scan_portable(root, sender, cancelled);
}

pub(crate) fn scan_target_capacity(root: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        windows_ntfs::total_capacity(root)
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        None
    }
}

fn scan_portable(root: PathBuf, sender: Sender<ScanMessage>, cancelled: Arc<AtomicBool>) {
    let scan_time = SystemTime::now();
    let mut stats = ScanStats::default();
    let mut batch = Vec::with_capacity(512);
    for entry in jwalk::WalkDir::new(root)
        .skip_hidden(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        match entry.metadata() {
            Ok(metadata) => {
                stats.files += 1;
                stats.bytes = stats.bytes.saturating_add(metadata.len());
                batch.push(FileEntry::new_at(
                    entry.path(),
                    metadata.len(),
                    metadata.modified().ok(),
                    scan_time,
                ));
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
    if !batch.is_empty() && sender.send(ScanMessage::Batch(batch, stats)).is_err() {
        return;
    }
    let _ = sender.send(ScanMessage::Finished(
        stats,
        cancelled.load(Ordering::Relaxed),
    ));
}

pub(crate) fn scan_engine_label() -> &'static str {
    #[cfg(windows)]
    {
        "WIN32 LARGE FETCH / OWN INDEX"
    }
    #[cfg(not(windows))]
    {
        "JWALK / OWN INDEX"
    }
}
