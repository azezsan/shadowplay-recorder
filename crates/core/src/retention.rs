//! Deletes the oldest videos in the recordings folder under the retention
//! rule: the size cap keeps the folder under a total size, and fill the disk
//! keeps the free-space reserve free on the folder's disk. Only video files
//! directly in the folder are counted or deleted.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::recorder::has_extension;

const VIDEO_EXTENSIONS: [&str; 5] = ["mp4", "mkv", "avi", "mov", "wmv"];
pub const GIB: u64 = 1024 * 1024 * 1024;
/// The automatic reserve is this many times the largest recording.
const AUTOMATIC_RESERVE_FACTOR: f64 = 1.5;
/// The automatic reserve while the folder holds no recording to learn from.
pub const STARTING_RESERVE: u64 = 30 * GIB;

pub fn gib_to_bytes(gib: f64) -> u64 {
    (gib.max(0.0) * GIB as f64) as u64
}

struct Video {
    modified: SystemTime,
    size: u64,
    path: PathBuf,
}

/// The videos directly in `folder`, oldest first.
fn videos(folder: &Path) -> Vec<Video> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut videos: Vec<_> = entries
        .flatten()
        .filter(|e| has_extension(&e.path(), &VIDEO_EXTENSIONS))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file().then(|| {
                Some(Video {
                    modified: meta.modified().ok()?,
                    size: meta.len(),
                    path: e.path(),
                })
            })?
        })
        .collect();
    videos.sort_by(|a, b| (a.modified, &a.path).cmp(&(b.modified, &b.path)));
    videos
}

/// Deletes `video`, returning whether it went.
fn delete(video: &Video, rule: &str) -> bool {
    match fs::remove_file(&video.path) {
        Ok(()) => {
            log::info!(
                "{rule}: deleted {:?} ({:.2} GB)",
                video.path.file_name().unwrap_or_default(),
                video.size as f64 / GIB as f64
            );
            true
        }
        // In use: a later run will catch it.
        Err(e) => {
            log::warn!("{rule}: could not delete {:?}: {e}", video.path);
            false
        }
    }
}

/// Deletes the oldest videos until the folder fits in `cap_bytes`.
pub fn enforce_size_cap(folder: &Path, cap_bytes: u64) {
    let videos = videos(folder);
    let mut total: u64 = videos.iter().map(|v| v.size).sum();
    for video in videos {
        if total <= cap_bytes {
            break;
        }
        if delete(&video, "size cap") {
            total -= video.size;
        }
    }
}

/// True for a recording named after its match, which is how the app leaves
/// every match it recorded. Test recordings and other videos keep
/// ShadowPlay's own name.
fn is_match_recording(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}

/// The automatic free-space reserve: enough for a recording as large as the
/// largest match recording in the folder, with a margin.
pub fn automatic_reserve(folder: &Path) -> u64 {
    videos(folder)
        .iter()
        .filter(|v| is_match_recording(&v.path))
        .map(|v| v.size)
        .max()
        .map_or(STARTING_RESERVE, |largest| {
            (largest as f64 * AUTOMATIC_RESERVE_FACTOR) as u64
        })
}

/// What keeping the free-space reserve came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveOutcome {
    /// The reserve was already free, or deleting made it free.
    Kept,
    /// Even deleting every recording would not free the reserve, so nothing
    /// was deleted.
    Short,
}

/// Deletes the oldest videos until `free` plus what was deleted reaches
/// `reserve`. Videos modified at or after `keep_since` belong to the
/// recording in progress: they are never deleted or counted. If deleting
/// every other video would still fall short, nothing is deleted.
pub fn keep_reserve(
    folder: &Path,
    free: u64,
    reserve: u64,
    keep_since: Option<SystemTime>,
) -> ReserveOutcome {
    if free >= reserve {
        return ReserveOutcome::Kept;
    }
    let needed = reserve - free;
    let videos: Vec<_> = videos(folder)
        .into_iter()
        .filter(|v| keep_since.is_none_or(|since| v.modified < since))
        .collect();
    if videos.iter().map(|v| v.size).sum::<u64>() < needed {
        return ReserveOutcome::Short;
    }
    let mut freed = 0;
    for video in videos {
        if freed >= needed {
            break;
        }
        if delete(&video, "free-space reserve") {
            freed += video.size;
        }
    }
    // A video that could not be deleted is retried at the next check.
    ReserveOutcome::Kept
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::time::Duration;

    use super::*;

    const MIB: u64 = 1024 * 1024;

    /// Writes a video of `mib` MiB, last modified `age_hours` ago.
    fn video(folder: &Path, name: &str, mib: u64, age_hours: u64) -> PathBuf {
        let path = folder.join(name);
        let file = File::create(&path).unwrap();
        file.set_len(mib * MIB).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(age_hours * 3600))
            .unwrap();
        path
    }

    #[test]
    fn deletes_the_oldest_videos_first_and_leaves_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let old = video(dir.path(), "old.mp4", 1, 3);
        let mid = video(dir.path(), "mid.mkv", 1, 2);
        let new = video(dir.path(), "new.mp4", 1, 1);
        fs::write(dir.path().join("notes.txt"), vec![b'y'; MIB as usize]).unwrap();

        enforce_size_cap(dir.path(), 2 * MIB);
        assert!(!old.exists());
        assert!(mid.exists());
        assert!(new.exists());
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

    #[test]
    fn the_automatic_reserve_follows_the_largest_match_recording() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(automatic_reserve(dir.path()), STARTING_RESERVE);

        // A test recording is no guide to how large a match gets.
        video(dir.path(), "DefaultShadowPlay.mp4", 1, 1);
        assert_eq!(automatic_reserve(dir.path()), STARTING_RESERVE);

        video(dir.path(), "81038409.mp4", 4, 2);
        video(dir.path(), "81038410.mp4", 2, 1);
        assert_eq!(automatic_reserve(dir.path()), 6 * MIB);
    }

    #[test]
    fn deletes_the_oldest_videos_until_the_reserve_is_free() {
        let dir = tempfile::tempdir().unwrap();
        let old = video(dir.path(), "1.mp4", 2, 3);
        let mid = video(dir.path(), "2.mp4", 2, 2);
        let new = video(dir.path(), "3.mp4", 2, 1);

        let outcome = keep_reserve(dir.path(), MIB, 4 * MIB, None);
        assert_eq!(outcome, ReserveOutcome::Kept);
        assert!(!old.exists());
        assert!(!mid.exists());
        assert!(new.exists());
    }

    #[test]
    fn deletes_nothing_when_the_reserve_is_free() {
        let dir = tempfile::tempdir().unwrap();
        let old = video(dir.path(), "1.mp4", 2, 3);
        assert_eq!(
            keep_reserve(dir.path(), 4 * MIB, 4 * MIB, None),
            ReserveOutcome::Kept
        );
        assert!(old.exists());
    }

    #[test]
    fn deletes_nothing_when_every_video_would_not_be_enough() {
        let dir = tempfile::tempdir().unwrap();
        let old = video(dir.path(), "1.mp4", 2, 3);
        let new = video(dir.path(), "2.mp4", 2, 2);
        assert_eq!(
            keep_reserve(dir.path(), 0, 5 * MIB, None),
            ReserveOutcome::Short
        );
        assert!(old.exists());
        assert!(new.exists());
    }

    #[test]
    fn never_deletes_or_counts_the_recording_in_progress() {
        let dir = tempfile::tempdir().unwrap();
        let old = video(dir.path(), "1.mp4", 2, 3);
        let in_progress = video(dir.path(), "DefaultShadowPlay.mp4", 8, 0);
        let match_started = SystemTime::now() - Duration::from_secs(1800);

        // Deleting the recording in progress would be enough, but it is not
        // a candidate, so the old one alone falls short.
        assert_eq!(
            keep_reserve(dir.path(), 0, 4 * MIB, Some(match_started)),
            ReserveOutcome::Short
        );
        assert!(old.exists());

        assert_eq!(
            keep_reserve(dir.path(), 0, 2 * MIB, Some(match_started)),
            ReserveOutcome::Kept
        );
        assert!(!old.exists());
        assert!(in_progress.exists());
    }
}
