//! Non-UI Lag core, mirroring Services/{FileLog,ClipMetadata,HardwareDetector,
//! HardwareProfiler,PresetResolver,RecorderOptions}.cs, Models/ReplayClip.cs,
//! Core/Localizer.cs and ViewModels/SettingsViewModel.cs settings data.

pub mod clip;
pub mod filelog;
pub mod hardware;
pub mod i18n;
pub mod metadata;
pub(crate) mod netjson;
pub mod presets;
pub mod recorder_options;
pub mod settings;

#[cfg(test)]
fn test_directory(name: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-data").join(format!(
        "{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}
