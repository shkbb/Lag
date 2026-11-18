//! Mirrors ViewModels/MainViewModel.cs:713-742 (Assets/save.wav and screenshot.wav).
use std::os::windows::ffi::OsStrExt;
use windows::{
    core::PCWSTR,
    Win32::Media::Audio::{PlaySoundW, SND_FILENAME, SND_MEMORY, SND_NODEFAULT, SND_SYNC},
};

pub fn play(file: &'static str) {
    std::thread::spawn(move || {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let path = exe.with_file_name("Assets").join(file);
        let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // Play synchronously on a worker, so the buffer and path live through the complete sound.
        // Standalone Cargo builds have no asset-copy stage. Keep the exact C# WAVs embedded as a
        // fallback; installed Assets/*.wav files still take precedence.
        let embedded: &[u8] = match file {
            "save.wav" => include_bytes!("assets/save.wav"),
            "screenshot.wav" => include_bytes!("assets/screenshot.wav"),
            _ => return,
        };
        let (source, flags) = if path.is_file() {
            (PCWSTR(wide.as_ptr()), SND_FILENAME)
        } else {
            (PCWSTR(embedded.as_ptr().cast()), SND_MEMORY)
        };
        if !unsafe { PlaySoundW(source, None, flags | SND_NODEFAULT | SND_SYNC) }.as_bool() {
            log::warn!("[MainViewModel] Failed to play sound '{file}': PlaySound failed");
        }
    });
}
