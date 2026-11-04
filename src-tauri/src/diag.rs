use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock, TryLockError},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_BYTES: u64 = 5 * 1024 * 1024;
static WRITER: OnceLock<Mutex<Writer>> = OnceLock::new();
static INIT: std::sync::Once = std::sync::Once::new();

struct Writer {
    path: PathBuf,
    max: u64,
    file: Option<File>,
}

impl Writer {
    fn write(&mut self, line: &str) {
        let _ = self.write_inner(line);
    }

    fn write_inner(&mut self, line: &str) -> std::io::Result<()> {
        if self.file.is_none() {
            self.file = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
        }
        let len = self.file.as_ref().unwrap().metadata()?.len();
        if len > 0 && len.saturating_add(line.len() as u64 + 1) > self.max {
            self.file.take();
            let backup = self.path.with_extension("log.1");
            match std::fs::remove_file(&backup) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
            std::fs::rename(&self.path, backup)?;
            self.file = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
        }
        writeln!(self.file.as_mut().unwrap(), "{line}")
    }
}

pub fn init(dir: &Path) {
    INIT.call_once(|| {
        let _ = std::fs::create_dir_all(dir);
        let _ = WRITER.set(Mutex::new(Writer {
            path: dir.join("stream-assist.log"),
            max: MAX_BYTES,
            file: None,
        }));
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            line("panic", info);
            previous(info);
        }));
    });
}

pub fn line(area: &str, msg: impl std::fmt::Display) {
    let text = format!("{} {area} {msg}", format_ts(SystemTime::now()));
    if let Some(writer) = WRITER.get() {
        match writer.try_lock() {
            Ok(mut writer) => writer.write(&text),
            Err(TryLockError::Poisoned(e)) => e.into_inner().write(&text),
            Err(TryLockError::WouldBlock) => (),
        }
    }
    let _ = writeln!(std::io::stderr().lock(), "{text}");
}

pub fn log_path() -> Option<PathBuf> {
    WRITER
        .get()
        .map(|w| w.lock().unwrap_or_else(|e| e.into_inner()).path.clone())
}

pub fn tail(n: usize) -> Vec<String> {
    let Some(writer) = WRITER.get() else {
        return Vec::new();
    };
    let writer = writer.lock().unwrap_or_else(|e| e.into_inner());
    tail_of(&writer.path, n)
}

fn tail_of(path: &Path, n: usize) -> Vec<String> {
    fn read(path: &Path, n: usize) -> std::io::Result<Vec<String>> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        let start = len.saturating_sub(256 * 1024);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.take(256 * 1024).read_to_end(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes);
        let text = if start > 0 {
            text.split_once('\n').map_or("", |(_, rest)| rest)
        } else {
            &text
        };
        let mut lines: Vec<_> = text.lines().rev().take(n).map(str::to_owned).collect();
        lines.reverse();
        Ok(lines)
    }
    read(path, n).unwrap_or_default()
}

pub fn http_err(e: &anyhow::Error) -> String {
    for cause in e.chain() {
        if let Some(e) = cause.downcast_ref::<reqwest::Error>() {
            return format!(
                "http status={:?} timeout={} connect={} request={}",
                e.status(),
                e.is_timeout(),
                e.is_connect(),
                e.is_request()
            );
        }
    }
    let text = format!("{e:#}");
    let mut result = String::with_capacity(text.len());
    for part in text.split_inclusive(char::is_whitespace) {
        if let Some(scheme) = part.find("://")
            && let Some(query) = part[scheme + 3..].find('?')
        {
            result.push_str(&part[..scheme + 3 + query]);
            result.extend(part.chars().filter(|c| c.is_whitespace()));
            continue;
        }
        result.push_str(part);
    }
    result
}

fn format_ts(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let days = (secs / 86400) as i64 + 719468;
    let era = days / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        secs / 3600 % 24,
        secs / 60 % 60,
        secs % 60,
        duration.subsec_millis()
    )
}

#[macro_export]
macro_rules! dlog {
    ($area:expr, $($arg:tt)*) => {
        $crate::diag::line($area, format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn writer(path: PathBuf, max: u64) -> Writer {
        Writer {
            path,
            max,
            file: None,
        }
    }

    #[test]
    fn writes_line_with_area() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stream-assist.log");
        writer(path.clone(), MAX_BYTES).write("timestamp tts hello");
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .ends_with("tts hello\n")
        );
    }

    #[test]
    fn rotates_over_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stream-assist.log");
        let mut w = writer(path.clone(), 100);
        w.write(&"a".repeat(79));
        w.write(&"b".repeat(79));
        assert_eq!(
            std::fs::read_to_string(path.with_extension("log.1")).unwrap(),
            format!("{}\n", "a".repeat(79))
        );
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            format!("{}\n", "b".repeat(79))
        );
    }

    #[test]
    fn rotation_replaces_old_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stream-assist.log");
        let mut w = writer(path.clone(), 100);
        for c in ['a', 'b', 'c'] {
            w.write(&c.to_string().repeat(79));
        }
        assert_eq!(
            std::fs::read_to_string(path.with_extension("log.1")).unwrap(),
            format!("{}\n", "b".repeat(79))
        );
    }

    #[test]
    fn write_failure_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("file");
        std::fs::write(&parent, "x").unwrap();
        writer(parent.join("x.log"), MAX_BYTES).write("hello");
    }

    #[test]
    fn tail_returns_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stream-assist.log");
        let mut w = writer(path.clone(), MAX_BYTES);
        for i in 0..300 {
            w.write(&format!("line {i}"));
        }
        let lines = tail_of(&path, 200);
        assert_eq!(lines.len(), 200);
        assert_eq!(lines.last().unwrap(), "line 299");
    }

    #[test]
    fn timestamp_is_utc_iso() {
        assert_eq!(
            format_ts(UNIX_EPOCH + Duration::from_millis(1500)),
            "1970-01-01T00:00:01.500Z"
        );
        assert_eq!(
            format_ts(UNIX_EPOCH + Duration::from_secs(1_793_800_000)),
            "2026-11-04T13:46:40.000Z"
        );
    }

    #[test]
    fn http_err_strips_query() {
        let error = anyhow::anyhow!("GET https://x/y?token=abc failed");
        assert_eq!(http_err(&error), "GET https://x/y failed");
    }
}
