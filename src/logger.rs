//! Log lines go to `~/Library/Logs/ClaudeControlPlane.log` as well as stderr: launched from
//! Finder or the Dock, stderr goes nowhere. Console.app opens the file from its Log Reports.

use std::{
    fmt::Display,
    fs::{self, File, OpenOptions},
    io::Write,
    panic::Location,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use log::{Level, LevelFilter, Metadata, Record};

use crate::sessions;

/// Past this the log starts over at launch, so it never grows without bound.
const MAX_LOG_BYTES: u64 = 1024 * 1024;

struct Logger {
    file: Option<Mutex<File>>,
}

impl log::Log for Logger {
    /// Ours from Info up; dependencies (gpui) only when something went wrong.
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Warn
            || (metadata.level() <= Level::Info
                && metadata.target().starts_with(env!("CARGO_CRATE_NAME")))
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!("{} {} {}\n", timestamp(), record.level(), record.args());
        eprint!("{line}");
        if let Some(file) = &self.file
            && let Ok(mut file) = file.lock()
        {
            file.write_all(line.as_bytes()).ok();
        }
    }

    fn flush(&self) {}
}

pub fn init() {
    let path = sessions::home_dir().join("Library/Logs/ClaudeControlPlane.log");
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_LOG_BYTES) {
        fs::remove_file(&path).ok();
    }
    let file = path
        .parent()
        .and_then(|dir| fs::create_dir_all(dir).ok())
        .and_then(|()| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()
        })
        .map(Mutex::new);
    if log::set_boxed_logger(Box::new(Logger { file })).is_ok() {
        log::set_max_level(LevelFilter::Info);
    }
    // A panic otherwise leaves no trace when launched from Finder (no crash report either).
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!(
            "panic: {info}\n{}",
            std::backtrace::Backtrace::force_capture()
        );
        default_hook(info);
    }));
}

/// Local time, `2026-10-12 09:41:07`.
fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as libc::time_t;
    // SAFETY: `localtime_r` writes only into the zeroed `tm` we pass.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&seconds, &mut tm) };
    format!(
        "{}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// `.ok()` that leaves a trace: the error and where it was dropped.
pub trait LogErr<T> {
    fn log_err(self) -> Option<T>;
}

impl<T, E: Display> LogErr<T> for Result<T, E> {
    #[track_caller]
    fn log_err(self) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(error) => {
                let caller = Location::caller();
                log::error!("{}:{}: {error}", caller.file(), caller.line());
                None
            }
        }
    }
}
