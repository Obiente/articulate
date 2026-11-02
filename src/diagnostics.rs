//! Local diagnostics: a bounded, rotating log file for the desktop app,
//! panic reports, and visible console output for command-line use.
//!
//! The log lives in `logs/` under the app data folder. It records lifecycle
//! events and errors only; never transcripts, notes or other document text.

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// A log file rotates when it would grow past this size.
const MAX_BYTES: u64 = 1024 * 1024;
/// Rotated files kept: `articulate.1.log` (newest) to `articulate.3.log`.
const ROTATED: usize = 3;
/// Longer messages are cut to keep one entry from filling the log.
const MAX_MESSAGE: usize = 4096;
const FILE_NAME: &str = "articulate.log";

/// The folder holding the log files.
pub fn log_directory() -> PathBuf {
    crate::model::data_dir().join("logs")
}

struct LogFile {
    directory: PathBuf,
    file: File,
    written: u64,
}

impl LogFile {
    fn open(directory: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(directory)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join(FILE_NAME))?;
        let written = file.metadata()?.len();
        Ok(Self {
            directory: directory.to_owned(),
            file,
            written,
        })
    }

    fn rotated(&self, index: usize) -> PathBuf {
        self.directory.join(format!("articulate.{index}.log"))
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        self.file.flush()?;
        let _ = fs::remove_file(self.rotated(ROTATED));
        for index in (1..ROTATED).rev() {
            let _ = fs::rename(self.rotated(index), self.rotated(index + 1));
        }
        fs::rename(self.directory.join(FILE_NAME), self.rotated(1))?;
        self.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(self.directory.join(FILE_NAME))?;
        self.written = 0;
        Ok(())
    }

    fn append(&mut self, line: &[u8]) {
        if self.written > 0 && self.written + line.len() as u64 > MAX_BYTES {
            let _ = self.rotate();
        }
        if self.file.write_all(line).is_ok() {
            self.written += line.len() as u64;
        }
    }
}

fn level_from_env(variable: &str, default: log::LevelFilter) -> log::LevelFilter {
    std::env::var(variable)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn timestamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.millisecond()
    )
}

fn format_entry(record: &log::Record<'_>) -> String {
    let mut message = record.args().to_string();
    if message.len() > MAX_MESSAGE {
        let mut end = MAX_MESSAGE;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push('…');
    }
    format!(
        "{} {:<5} {}: {}\n",
        timestamp(),
        record.level(),
        record.target(),
        message.replace('\n', " ")
    )
}

/// Application messages at `level`; other crates only at warnings or above.
struct Filter {
    level: log::LevelFilter,
}

impl Filter {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        let limit = if metadata.target().starts_with("articulate") {
            self.level
        } else {
            self.level.min(log::LevelFilter::Warn)
        };
        metadata.level() <= limit
    }
}

struct FileLogger {
    filter: Filter,
    file: Mutex<LogFile>,
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        self.filter.enabled(metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let entry = format_entry(record);
            self.file
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .append(entry.as_bytes());
        }
    }

    fn flush(&self) {
        let _ = self
            .file
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .file
            .flush();
    }
}

struct ConsoleLogger {
    filter: Filter,
}

impl log::Log for ConsoleLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        self.filter.enabled(metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let _ = std::io::stderr().write_all(format_entry(record).as_bytes());
        }
    }

    fn flush(&self) {}
}

fn install(logger: Box<dyn log::Log>, level: log::LevelFilter) {
    if log::set_logger(Box::leak(logger)).is_ok() {
        log::set_max_level(level);
    }
    install_panic_hook();
}

/// Log to the rotating file. Set `ARTICULATE_LOG` (error, warn, info,
/// debug or trace) to change the level; the default is info.
pub fn init_file_log() {
    let level = level_from_env("ARTICULATE_LOG", log::LevelFilter::Info);
    match LogFile::open(&log_directory()) {
        Ok(file) => install(
            Box::new(FileLogger {
                filter: Filter { level },
                file: Mutex::new(file),
            }),
            level,
        ),
        Err(error) => {
            // Diagnostics must never prevent the app from starting.
            eprintln!("Could not open the diagnostic log: {error}");
            install_panic_hook();
        }
    }
    log::info!("Articulate {} started", env!("CARGO_PKG_VERSION"));
}

/// Log warnings and errors to standard error, for command-line use.
pub fn init_console_log() {
    let level = level_from_env("ARTICULATE_LOG", log::LevelFilter::Warn);
    install(
        Box::new(ConsoleLogger {
            filter: Filter { level },
        }),
        level,
    );
}

/// Record panics in the log, then run the default report.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "an unknown location".into());
        log::error!(
            "Panic at {location}: {}",
            info.payload_as_str()
                .unwrap_or("an unexpected internal error")
        );
        log::logger().flush();
        previous(info);
    }));
}

/// Release builds use the Windows GUI subsystem, so command-line output is
/// invisible unless the process attaches to the console that started it.
/// Output appears after the shell prompt returns; redirect it or pipe it to
/// another program to capture it in order.
pub fn attach_parent_console() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE},
            Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
            System::Console::{
                ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE,
                STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
            },
        };
        // Fails when there is no parent console or one is already attached.
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        // Redirected handles stay as they are; missing ones use the console.
        for (id, device) in [
            (STD_OUTPUT_HANDLE, "CONOUT$"),
            (STD_ERROR_HANDLE, "CONOUT$"),
            (STD_INPUT_HANDLE, "CONIN$"),
        ] {
            let current = GetStdHandle(id);
            if !current.is_null() && current != INVALID_HANDLE_VALUE {
                continue;
            }
            let name: Vec<u16> = device.encode_utf16().chain(Some(0)).collect();
            let handle = CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if handle != INVALID_HANDLE_VALUE {
                SetStdHandle(id, handle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_files_rotate_and_stay_bounded() {
        let directory = std::env::temp_dir().join(format!(
            "articulate-log-test-{}-{}",
            std::process::id(),
            crate::history::Session::new(crate::history::Kind::Note).id
        ));
        let mut log = LogFile::open(&directory).unwrap();
        let line = vec![b'x'; 64 * 1024];
        for _ in 0..80 {
            log.append(&line);
        }
        let size = |path: PathBuf| fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        assert!(size(directory.join(FILE_NAME)) <= MAX_BYTES);
        for index in 1..=ROTATED {
            assert!(size(log.rotated(index)) <= MAX_BYTES);
            assert!(size(log.rotated(index)) > 0, "rotated file {index}");
        }
        assert!(!log.rotated(ROTATED + 1).exists());
        drop(log);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn entries_are_single_bounded_lines() {
        let long = "é".repeat(MAX_MESSAGE);
        let entry = format_entry(
            &log::Record::builder()
                .args(format_args!("first\nsecond {long}"))
                .level(log::Level::Warn)
                .target("articulate::test")
                .build(),
        );
        assert!(entry.ends_with("…\n"));
        assert_eq!(entry.matches('\n').count(), 1);
        assert!(entry.contains(" WARN  articulate::test: first second"));
        assert!(entry.len() < MAX_MESSAGE + 100);
    }

    #[test]
    fn other_crates_log_only_warnings() {
        let filter = Filter {
            level: log::LevelFilter::Info,
        };
        let metadata = |target, level| log::Metadata::builder().target(target).level(level).build();
        assert!(filter.enabled(&metadata("articulate::app", log::Level::Info)));
        assert!(!filter.enabled(&metadata("tao::platform", log::Level::Info)));
        assert!(filter.enabled(&metadata("tao::platform", log::Level::Warn)));
    }
}
