//! The device's log file, bounded in size: past `max_bytes` the file moves
//! to `<file>.1` (an older `<file>.1` to `<file>.2`, up to `keep` old
//! files; the oldest goes) and a new file starts. A service runs for
//! months; its log must never fill the disk (the FOH PC's instance 1 wrote
//! 63 MB in a day and a half).

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 10 MB per file.
pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Old files kept beside the current one.
pub const KEEP: u32 = 3;

/// An appending log file that rotates at `max_bytes`.
pub struct RotatingLog {
    path: PathBuf,
    max_bytes: u64,
    keep: u32,
    /// None only inside a rotation (Windows renames no open file).
    file: Option<File>,
    len: u64,
}

impl RotatingLog {
    /// Opens `path` for appending (its old content stays).
    pub fn open(path: &Path, max_bytes: u64, keep: u32) -> io::Result<RotatingLog> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let len = file.metadata()?.len();
        Ok(RotatingLog {
            path: path.to_path_buf(),
            max_bytes,
            keep,
            file: Some(file),
            len,
        })
    }

    fn old(&self, n: u32) -> PathBuf {
        let mut p = self.path.clone().into_os_string();
        p.push(format!(".{n}"));
        PathBuf::from(p)
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Closed first: Windows renames no open file.
        self.file = None;
        if self.keep > 0 {
            let _ = std::fs::remove_file(self.old(self.keep));
            for n in (1..self.keep).rev() {
                let from = self.old(n);
                if from.exists() {
                    std::fs::rename(&from, self.old(n + 1))?;
                }
            }
            std::fs::rename(&self.path, self.old(1))?;
        }
        self.file = Some(File::create(&self.path)?);
        self.len = 0;
        Ok(())
    }
}

impl Write for RotatingLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // One record stays whole in one file; a file never starts empty.
        if self.len > 0 && self.len + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        let file = match self.file.as_mut() {
            Some(f) => f,
            None => {
                // A rotation that failed after closing: open again.
                self.file = Some(
                    OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&self.path)?,
                );
                self.file.as_mut().unwrap()
            }
        };
        let n = file.write(buf)?;
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

/// The device's counters line: every 10 s.
pub const COUNTS_EVERY: Duration = Duration::from_secs(10);
/// The device's full status (several KB): at the first counts line, then
/// every 5 min (every 10 s it alone wrote some 60 MB a day).
pub const STATUS_EVERY: Duration = Duration::from_secs(300);

/// A periodic log line: due once `every` has passed since it last was, and
/// at the first ask when `at_once`.
pub struct Every {
    every: Duration,
    last: Option<Instant>,
}

impl Every {
    pub fn new(every: Duration, start: Instant, at_once: bool) -> Every {
        Every {
            every,
            last: (!at_once).then_some(start),
        }
    }

    /// Whether the line is due at `now` (then it counts as written).
    pub fn due(&mut self, now: Instant) -> bool {
        match self.last {
            Some(last) if now.duration_since(last) < self.every => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dasio-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap_or_default()
    }

    #[test]
    fn a_file_past_its_bound_moves_aside_and_a_new_one_starts() {
        let d = dir("rotate");
        let path = d.join("x.log");
        let mut log = RotatingLog::open(&path, 10, 2).unwrap();
        log.write_all(b"aaaaaaaa\n").unwrap(); // 9 bytes
        log.write_all(b"bbbb\n").unwrap(); // 14 > 10: rotates first
        log.flush().unwrap();
        assert_eq!(read(&path), "bbbb\n");
        assert_eq!(read(&d.join("x.log.1")), "aaaaaaaa\n");
        assert!(!d.join("x.log.2").exists());
    }

    #[test]
    fn only_keep_old_files_stay_and_the_oldest_goes() {
        let d = dir("keep");
        let path = d.join("x.log");
        let mut log = RotatingLog::open(&path, 4, 2).unwrap();
        for line in ["1111\n", "2222\n", "3333\n", "4444\n"] {
            log.write_all(line.as_bytes()).unwrap();
        }
        log.flush().unwrap();
        assert_eq!(read(&path), "4444\n");
        assert_eq!(read(&d.join("x.log.1")), "3333\n");
        assert_eq!(read(&d.join("x.log.2")), "2222\n");
        assert!(!d.join("x.log.3").exists());
    }

    #[test]
    fn a_reopened_log_counts_what_is_already_in_it() {
        let d = dir("reopen");
        let path = d.join("x.log");
        std::fs::write(&path, "old-old\n").unwrap(); // 8 bytes
        let mut log = RotatingLog::open(&path, 10, 1).unwrap();
        log.write_all(b"new\n").unwrap(); // 12 > 10
        log.flush().unwrap();
        assert_eq!(read(&path), "new\n");
        assert_eq!(read(&d.join("x.log.1")), "old-old\n");
    }

    #[test]
    fn a_record_larger_than_the_bound_is_written_whole() {
        let d = dir("large");
        let path = d.join("x.log");
        let mut log = RotatingLog::open(&path, 4, 1).unwrap();
        log.write_all(b"0123456789\n").unwrap();
        log.flush().unwrap();
        assert_eq!(read(&path), "0123456789\n");
        assert!(!d.join("x.log.1").exists());
    }

    #[test]
    fn keep_zero_starts_the_file_over() {
        let d = dir("zero");
        let path = d.join("x.log");
        let mut log = RotatingLog::open(&path, 4, 0).unwrap();
        log.write_all(b"aaaa").unwrap();
        log.write_all(b"bb").unwrap();
        log.flush().unwrap();
        assert_eq!(read(&path), "bb");
        assert!(!d.join("x.log.1").exists());
    }

    #[test]
    fn a_periodic_line_is_due_once_per_period() {
        let t = Instant::now();
        let s = Duration::from_secs;
        let mut e = Every::new(s(10), t, false);
        assert!(!e.due(t));
        assert!(!e.due(t + s(9)));
        assert!(e.due(t + s(10)));
        assert!(!e.due(t + s(19)));
        assert!(e.due(t + s(21)));
        assert!(!e.due(t + s(30)));
    }

    #[test]
    fn an_at_once_line_is_due_at_the_first_ask() {
        let t = Instant::now();
        let s = Duration::from_secs;
        let mut e = Every::new(s(300), t, true);
        assert!(e.due(t + s(10)));
        assert!(!e.due(t + s(20)));
        assert!(!e.due(t + s(309)));
        assert!(e.due(t + s(310)));
    }

    #[test]
    fn the_bound_is_10_mib_with_3_old_files() {
        assert_eq!(MAX_BYTES, 10_485_760);
        assert_eq!(KEEP, 3);
    }

    #[test]
    fn a_record_that_just_fits_stays_in_the_file() {
        let d = dir("fits");
        let path = d.join("x.log");
        let mut log = RotatingLog::open(&path, 10, 1).unwrap();
        log.write_all(b"aaaaa\n").unwrap(); // 6
        log.write_all(b"bbb\n").unwrap(); // 10 == 10: still fits
        log.flush().unwrap();
        assert_eq!(read(&path), "aaaaa\nbbb\n");
        assert!(!d.join("x.log.1").exists());
    }

    #[test]
    fn the_bound_counts_the_sum_of_what_is_in_the_file_and_the_record() {
        let d = dir("sum");
        let path = d.join("x.log");
        let mut log = RotatingLog::open(&path, 10, 1).unwrap();
        log.write_all(b"a").unwrap(); // 1
        log.write_all(b"012345678\n").unwrap(); // 1 + 10 = 11 > 10
        log.flush().unwrap();
        assert_eq!(read(&path), "012345678\n");
        assert_eq!(read(&d.join("x.log.1")), "a");
    }
}
