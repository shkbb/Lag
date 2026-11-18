//! Headless ports of ProcessLoopbackCapture.cs SelfTest and PlaybackTrackProbe.cs Run.
struct Console;
impl log::Log for Console {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        println!("{}", record.args());
    }
    fn flush(&self) {}
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    static LOGGER: Console = Console;
    log::set_logger(&LOGGER).map_err(|e| e.to_string())?;
    log::set_max_level(log::LevelFilter::Info);
    if std::env::var("LAG_MFPROBE").is_ok_and(|v| !v.is_empty()) {
        lag_audio::playback_probe::run_from_env();
    } else if let Some(pid) = std::env::var("LAG_PROCTEST").ok().and_then(|v| v.trim().parse::<i32>().ok()) {
        // SelfTest already emits the C# Start FAILED diagnostic; do not print it twice.
        let _ = lag_audio::ProcessLoopbackCapture::self_test(pid);
    } else {
        println!("Set LAG_PROCTEST=<pid> or LAG_MFPROBE=file1;file2;...");
    }
    Ok(())
}
