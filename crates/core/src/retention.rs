//! Keeps the recordings folder under the size cap by deleting the oldest
//! videos. Only video files directly in the folder are counted or deleted.

use std::fs;
use std::path::Path;

use crate::recorder::has_extension;

const VIDEO_EXTENSIONS: [&str; 5] = ["mp4", "mkv", "avi", "mov", "wmv"];
const GIB: u64 = 1024 * 1024 * 1024;

pub fn gib_to_bytes(gib: f64) -> u64 {
    (gib.max(0.0) * GIB as f64) as u64
}

/// Deletes the oldest videos until the folder fits in `cap_bytes`.
pub fn enforce_size_cap(folder: &Path, cap_bytes: u64) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    let mut videos: Vec<_> = entries
        .flatten()
        .filter(|e| has_extension(&e.path(), &VIDEO_EXTENSIONS))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file()
                .then(|| Some((meta.modified().ok()?, meta.len(), e.path())))?
        })
        .collect();

    let mut total: u64 = videos.iter().map(|(_, size, _)| size).sum();
    videos.sort();
    for (_, size, path) in videos {
        if total <= cap_bytes {
            break;
        }
        match fs::remove_file(&path) {
            Ok(()) => {
                total -= size;
                log::info!(
                    "size cap: deleted {:?} ({:.2} GB)",
                    path.file_name().unwrap_or_default(),
                    size as f64 / GIB as f64
                );
            }
            // In use: a later run will catch it.
            Err(e) => log::warn!("size cap: could not delete {path:?}: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::time::{Duration, SystemTime};

    use super::*;

    #[test]
    fn deletes_the_oldest_videos_first_and_leaves_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let mib = 1024 * 1024;
        for (age, name) in [(3, "old.mp4"), (2, "mid.mkv"), (1, "new.mp4")] {
            let path = dir.path().join(name);
            fs::write(&path, vec![b'x'; mib]).unwrap();
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(age * 3600))
                .unwrap();
        }
        fs::write(dir.path().join("notes.txt"), vec![b'y'; mib]).unwrap();

        enforce_size_cap(dir.path(), 2 * mib as u64);
        assert!(!dir.path().join("old.mp4").exists());
        assert!(dir.path().join("mid.mkv").exists());
        assert!(dir.path().join("new.mp4").exists());
        assert!(dir.path().join("notes.txt").exists());
    }

    #[test]
    fn does_nothing_under_the_cap_or_without_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.mp4"), b"x").unwrap();
        enforce_size_cap(dir.path(), gib_to_bytes(1.0));
        assert!(dir.path().join("a.mp4").exists());
        enforce_size_cap(&dir.path().join("missing"), 0);
    }
}
