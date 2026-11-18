//! Mirrors Services/FileLog.cs: Documents\Lag\Logs session files and appended crash.log.
//! Normal log calls only enqueue; disk writes, console tee and flushes run on a bounded worker.

use chrono::{DateTime, Local};
use crossbeam_channel::{bounded, Receiver, Sender};
use log::{LevelFilter, Log, Metadata, Record};
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};

pub fn log_directory() -> Option<PathBuf> {
    crate::hardware::documents().map(|p| p.join("Lag/Logs"))
}
pub fn crash_log_path() -> Option<PathBuf> {
    log_directory().map(|p| p.join("crash.log"))
}

/// Header values are explicit so the host can supply its release/build and runtime versions.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub version: String,
    pub os: String,
    pub is_64_bit: bool,
    pub runtime: String,
    pub machine: String,
}
impl Default for SessionInfo {
    fn default() -> Self {
        use windows::core::w;
        let build = crate::hardware::registry_string(
            w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            w!("CurrentBuildNumber"),
        )
        .unwrap_or_else(|| "?".into());
        Self {
            version: env!("CARGO_PKG_VERSION").into(),
            os: format!("Microsoft Windows NT 10.0.{build}.0"),
            is_64_bit: cfg!(target_pointer_width = "64"),
            runtime: "Rust".into(),
            machine: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "?".into()),
        }
    }
}
#[derive(Debug, Clone)]
pub struct Options {
    pub directory: PathBuf,
    pub session: SessionInfo,
    pub capacity: usize,
    pub console: bool,
    pub level: LevelFilter,
}
impl Options {
    pub fn for_user() -> Option<Self> {
        log_directory().map(Self::new)
    }
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            session: SessionInfo::default(),
            capacity: 4096,
            console: true,
            level: LevelFilter::Trace,
        }
    }
}
enum Command {
    Line { stamp: DateTime<Local>, message: String },
    Crash { stamp: DateTime<Local>, source: String, detail: String, echo: String },
    Flush(Sender<io::Result<()>>),
    Shutdown(Sender<io::Result<()>>),
}
struct Shared {
    sender: Sender<Command>,
    dropped: AtomicU64,
    path: PathBuf,
    level: LevelFilter,
}
#[derive(Clone)]
pub struct FileLog {
    shared: Arc<Shared>,
}
/// Retain this guard until application shutdown. Drop drains the queue and joins the writer.
pub struct LogGuard {
    logger: FileLog,
    thread: Option<JoinHandle<()>>,
}
impl FileLog {
    /// Startup I/O can fail without affecting application startup; callers may ignore this Result.
    pub fn start(options: Options) -> io::Result<(Self, LogGuard)> {
        std::fs::create_dir_all(&options.directory)?;
        let now = Local::now();
        let path = options.directory.join(format!("Lag-{}.txt", now.format("%Y-%m-%d_%H-%M-%S")));
        let file = File::create(&path)?;
        // Keep fifteen including the new launch, rather than briefly retaining sixteen.
        prune(&options.directory, 15, &path);
        let (sender, receiver) = bounded(options.capacity.max(1));
        let logger =
            Self { shared: Arc::new(Shared { sender, dropped: AtomicU64::new(0), path, level: options.level }) };
        let handle =
            thread::Builder::new().name("LagFileLog".into()).spawn(move || writer(file, receiver, options, now))?;
        Ok((logger.clone(), LogGuard { logger, thread: Some(handle) }))
    }
    pub fn install(options: Options) -> io::Result<LogGuard> {
        let level = options.level;
        let (logger, guard) = Self::start(options)?;
        log::set_boxed_logger(Box::new(logger)).map_err(io::Error::other)?;
        log::set_max_level(level);
        Ok(guard)
    }
    pub fn current_log_path(&self) -> &Path {
        &self.shared.path
    }
    /// Count rejected entries (queue full or writer already shut down); no I/O on callers.
    pub fn dropped_messages(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }
    fn enqueue(&self, command: Command) -> bool {
        if self.shared.sender.try_send(command).is_err() {
            self.shared.dropped.fetch_add(1, Ordering::Relaxed);
            false
        } else {
            true
        }
    }
    /// Verbatim message, with no Rust target/level prefix. False means the bounded queue rejected it.
    pub fn write_line(&self, message: impl Into<String>) -> bool {
        self.enqueue(Command::Line { stamp: Local::now(), message: message.into() })
    }
    /// Blocking barrier for previously queued writes. Call explicitly before exiting after a crash.
    pub fn flush(&self) -> io::Result<()> {
        self.barrier(false)
    }
    fn barrier(&self, shutdown: bool) -> io::Result<()> {
        let (tx, rx) = bounded(1);
        let command = if shutdown { Command::Shutdown(tx) } else { Command::Flush(tx) };
        self.shared
            .sender
            .send(command)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "log writer stopped"))?;
        rx.recv().map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "log writer stopped"))?
    }
    /// Appends the C# crash block and echoes `[CRASH:source] detail` into this session.
    /// Uses the same nonblocking queue; check the return value and flush before process termination.
    pub fn log_crash(&self, source: &str, exception: Option<&str>) -> bool {
        let detail = exception.unwrap_or("").to_owned();
        let echo = format!("[CRASH:{source}] {detail}");
        self.enqueue(Command::Crash { stamp: Local::now(), source: source.into(), detail, echo })
    }
    pub fn log_native_crash(&self, code: u32, address: u64, module: &str) -> bool {
        let name = native_exception_name(code);
        self.enqueue(Command::Crash {
            stamp: Local::now(),
            source: "NATIVE".into(),
            detail: format!("NATIVE CRASH {name} (0x{code:08X}) at 0x{address:016X} in {module}"),
            echo: format!("[CRASH:NATIVE] {name} in {module}"),
        })
    }
}
impl Log for FileLog {
    fn enabled(&self, m: &Metadata<'_>) -> bool {
        m.level() <= self.shared.level
    }
    fn log(&self, r: &Record<'_>) {
        if self.enabled(r.metadata()) {
            self.write_line(r.args().to_string());
        }
    }
    fn flush(&self) {
        let _ = FileLog::flush(self);
    }
}
impl LogGuard {
    pub fn logger(&self) -> &FileLog {
        &self.logger
    }
    pub fn flush(&self) -> io::Result<()> {
        self.logger.flush()
    }
    pub fn shutdown(mut self) -> io::Result<()> {
        self.stop()
    }
    fn stop(&mut self) -> io::Result<()> {
        let Some(handle) = self.thread.take() else {
            return Ok(());
        };
        let result = self.logger.barrier(true);
        handle.join().map_err(|_| io::Error::other("log writer panicked"))?;
        result
    }
}
impl Drop for LogGuard {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub fn native_exception_name(code: u32) -> &'static str {
    match code {
        0xC0000005 => "ACCESS_VIOLATION",
        0xC00000FD => "STACK_OVERFLOW",
        0xC0000409 => "STACK_BUFFER_OVERRUN",
        0xC0000094 => "INT_DIVIDE_BY_ZERO",
        0xC000001D => "ILLEGAL_INSTRUCTION",
        0x80000003 => "BREAKPOINT",
        _ => "EXCEPTION",
    }
}
fn header(stamp: DateTime<Local>, s: &SessionInfo) -> String {
    format!(
        "===== Lag log — {} =====\r\nLag {} | OS {} | 64-bit {} | CLR {} | machine {}\r\n{}\r\n",
        stamp.format("%Y-%m-%d %H:%M:%S"),
        s.version,
        s.os,
        if s.is_64_bit { "True" } else { "False" },
        s.runtime,
        s.machine,
        "-".repeat(60)
    )
}
fn stamped(stamp: DateTime<Local>, message: &str) -> String {
    format!("[{}] {message}\r\n", stamp.format("%H:%M:%S%.3f"))
}
fn crash_block(stamp: DateTime<Local>, source: &str, version: &str, detail: &str) -> String {
    format!("───── {} [{source}] Lag {version} ─────\r\n{detail}\r\n\r\n", stamp.format("%Y-%m-%d %H:%M:%S"))
}
fn writer(mut file: File, receiver: Receiver<Command>, options: Options, now: DateTime<Local>) {
    let mut error = file.write_all(header(now, &options.session).as_bytes()).err();
    while let Ok(command) = receiver.recv() {
        match command {
            Command::Line { stamp, message } => {
                if let Err(e) = file.write_all(stamped(stamp, &message).as_bytes()).and_then(|_| file.flush()) {
                    error = Some(e);
                }
                if options.console {
                    let _ = writeln!(io::stdout().lock(), "{message}");
                }
            }
            Command::Crash { stamp, source, detail, echo } => {
                let block = crash_block(stamp, &source, &options.session.version, &detail);
                let result = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(options.directory.join("crash.log"))
                    .and_then(|mut f| f.write_all(block.as_bytes()).and_then(|_| f.flush()));
                if let Err(e) = result {
                    error = Some(e);
                }
                if let Err(e) = file.write_all(stamped(stamp, &echo).as_bytes()).and_then(|_| file.flush()) {
                    error = Some(e);
                }
                if options.console {
                    let _ = writeln!(io::stdout().lock(), "{echo}");
                }
            }
            Command::Flush(reply) => {
                let result = file.flush().and_then(|_| error.take().map_or(Ok(()), Err));
                let _ = reply.send(result);
            }
            Command::Shutdown(reply) => {
                let result = file.flush().and_then(|_| error.take().map_or(Ok(()), Err));
                let _ = reply.send(result);
                break;
            }
        }
    }
}
fn prune(directory: &Path, keep: usize, current: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut logs: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| e.file_name().to_str().is_some_and(|s| s.starts_with("Lag-") && s.ends_with(".txt")))
        .filter_map(|e| e.metadata().and_then(|m| m.modified()).ok().map(|time| (e.path(), time)))
        .collect();
    logs.sort_by_key(|(path, time)| (std::cmp::Reverse(path == current), std::cmp::Reverse(*time)));
    for (path, _) in logs.into_iter().skip(keep) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    fn stamp() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 15, 22, 15, 4).single().unwrap() + chrono::Duration::milliseconds(818)
    }
    #[test]
    fn exact_real_log_line_and_header() {
        assert_eq!(
            stamped(stamp(), "[Ffmpeg] Bound libavcodec 61.19.101 from C:\\ffmpeg"),
            "[22:15:04.818] [Ffmpeg] Bound libavcodec 61.19.101 from C:\\ffmpeg\r\n"
        );
        let s = SessionInfo {
            version: "1.8.0".into(),
            os: "Microsoft Windows NT 10.0.26300.0".into(),
            is_64_bit: true,
            runtime: "8.0.25".into(),
            machine: "SHK-PC".into(),
        };
        assert_eq!(header(stamp(),&s),format!("===== Lag log — 2026-11-15 22:15:04 =====\r\nLag 1.8.0 | OS Microsoft Windows NT 10.0.26300.0 | 64-bit True | CLR 8.0.25 | machine SHK-PC\r\n{}\r\n","-".repeat(60)));
        assert_eq!(
            crash_block(stamp(), "AppDomain", "1.8.0", "Exception"),
            "───── 2026-11-15 22:15:04 [AppDomain] Lag 1.8.0 ─────\r\nException\r\n\r\n"
        );
    }
    #[test]
    fn flush_shutdown_crashes_and_rotation() {
        let dir = crate::test_directory("filelog");
        for i in 0..20 {
            std::fs::write(dir.join(format!("Lag-old-{i:02}.txt")), b"old").unwrap();
        }
        std::fs::write(dir.join("unrelated.txt"), b"keep").unwrap();
        let mut options = Options::new(&dir);
        options.console = false;
        options.capacity = 128;
        let (logger, guard) = FileLog::start(options).unwrap();
        assert!(logger.write_line("[Component] message"));
        assert!(logger.log_crash("AppDomain", Some("System.Exception: test")));
        assert!(logger.log_native_crash(0xC0000005, 0x1234, "avcodec.dll+0x42"));
        guard.flush().unwrap();
        let text = std::fs::read_to_string(logger.current_log_path()).unwrap();
        assert!(text.contains("] [Component] message\r\n"));
        let crashes = std::fs::read_to_string(dir.join("crash.log")).unwrap();
        assert!(crashes.contains("System.Exception: test\r\n\r\n"));
        assert!(crashes
            .contains("NATIVE CRASH ACCESS_VIOLATION (0xC0000005) at 0x0000000000001234 in avcodec.dll+0x42\r\n\r\n"));
        assert_eq!(
            std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().starts_with("Lag-"))
                .count(),
            15
        );
        assert!(logger.write_line("last"));
        guard.shutdown().unwrap();
        assert!(std::fs::read_to_string(logger.current_log_path()).unwrap().ends_with("last\r\n"));
        assert!(!logger.write_line("too late"));
        assert_eq!(logger.dropped_messages(), 1);
        assert!(dir.join("unrelated.txt").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn full_queue_never_waits_for_writer() {
        let (sender, _receiver) = bounded(1);
        let logger = FileLog {
            shared: Arc::new(Shared {
                sender,
                dropped: AtomicU64::new(0),
                path: PathBuf::new(),
                level: LevelFilter::Trace,
            }),
        };
        assert!(logger.write_line("first"));
        assert!(!logger.write_line("full"));
        assert_eq!(logger.dropped_messages(), 1);
    }

    #[test]
    fn facade_filter_drop_drain_and_writer_errors() {
        let dir = crate::test_directory("filelog-facade");
        let mut options = Options::new(&dir);
        options.console = false;
        options.level = LevelFilter::Warn;
        let (logger, guard) = FileLog::start(options).unwrap();
        logger.log(&Record::builder().level(log::Level::Info).args(format_args!("filtered")).build());
        logger.log(
            &Record::builder()
                .level(log::Level::Warn)
                .target("not a prefix")
                .args(format_args!("[Component] verbatim"))
                .build(),
        );
        // Making the crash filename a directory forces a real writer error without touching real logs.
        std::fs::create_dir(dir.join("crash.log")).unwrap();
        assert!(logger.log_crash("Test", None));
        assert!(guard.flush().is_err());
        assert!(logger.write_line("drained by Drop"));
        drop(guard);
        let text = std::fs::read_to_string(logger.current_log_path()).unwrap();
        assert!(!text.contains("filtered"));
        assert!(!text.contains("not a prefix"));
        assert!(text.contains("] [Component] verbatim\r\n"));
        assert!(text.ends_with("drained by Drop\r\n"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
