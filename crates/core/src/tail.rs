//! Follows a log file that another process is appending to.
//!
//! The first successful poll skips to the end, so old matches are never
//! replayed. A file that shrinks was restarted by a new game session and is
//! read again from its beginning. A trailing line without a newline is held
//! back until the rest of it arrives.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct TailResult {
    pub lines: Vec<String>,
    /// The file was restarted and these lines come from its beginning.
    pub truncated: bool,
}

pub struct Tailer {
    path: PathBuf,
    /// Bytes already read; `None` until the file has been seen once.
    offset: Option<u64>,
    pending: Vec<u8>,
}

impl Tailer {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            offset: None,
            pending: Vec::new(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Lines appended since the last poll, or `None` while the file is missing.
    pub fn poll(&mut self) -> Option<TailResult> {
        let size = std::fs::metadata(&self.path).ok()?.len();

        let Some(mut offset) = self.offset else {
            self.offset = Some(size);
            return Some(TailResult::default());
        };

        if size < offset {
            self.pending.clear();
            offset = 0;
        }
        if size == offset {
            self.offset = Some(offset);
            return Some(TailResult::default());
        }

        let truncated = offset == 0;
        let chunk = self.read_range(offset, size)?;
        self.offset = Some(offset + chunk.len() as u64);
        Some(TailResult {
            lines: self.split_lines(&chunk),
            truncated,
        })
    }

    fn read_range(&self, from: u64, to: u64) -> Option<Vec<u8>> {
        let mut file = File::open(&self.path).ok()?;
        file.seek(SeekFrom::Start(from)).ok()?;
        let mut chunk = Vec::new();
        file.take(to - from).read_to_end(&mut chunk).ok()?;
        Some(chunk)
    }

    fn split_lines(&mut self, chunk: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.pending.drain(..=end).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            lines.push(String::from_utf8_lossy(&line).into_owned());
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::Write;

    use super::*;

    fn append(path: &Path, text: &str) {
        let mut f = OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn missing_file_polls_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Tailer::new(dir.path().join("console.log")).poll(), None);
    }

    #[test]
    fn skips_history_then_reads_appended_lines_and_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("console.log");
        fs::write(&log, "preexisting line\n").unwrap();
        let mut tailer = Tailer::new(&log);
        assert_eq!(tailer.poll(), Some(TailResult::default()));

        append(
            &log,
            "Lobby 1 for Match 7 created\r\n[Client] Map: \"start\"\n",
        );
        let appended = tailer.poll().unwrap();
        assert_eq!(
            appended.lines,
            ["Lobby 1 for Match 7 created", "[Client] Map: \"start\""]
        );
        assert!(!appended.truncated);

        fs::write(&log, "Lobby 2 for Match 8 created\n").unwrap();
        let restarted = tailer.poll().unwrap();
        assert!(restarted.truncated);
        assert_eq!(restarted.lines, ["Lobby 2 for Match 8 created"]);
    }

    #[test]
    fn holds_back_a_partial_line() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("console.log");
        fs::write(&log, "").unwrap();
        let mut tailer = Tailer::new(&log);
        tailer.poll();

        append(&log, "partial without newline");
        assert!(tailer.poll().unwrap().lines.is_empty());
        append(&log, " continued\n");
        assert_eq!(
            tailer.poll().unwrap().lines,
            ["partial without newline continued"]
        );
    }
}
