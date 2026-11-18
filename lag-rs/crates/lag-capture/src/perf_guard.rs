//! Port of `Services/PerformanceGuard.cs`: keep capture/encode running at full speed while a game owns
//! the foreground and the GPU.
//!
//! 1. Opt OUT of Windows 11 power throttling (EcoQoS) for this process.
//! 2. Raise the process' GPU scheduling priority class (D3DKMT): Realtime (admin only), else High, else
//!    AboveNormal. Without it a game at 100% GPU starves our NVENC submissions (measured 2026-11-16 on CS2:
//!    11-25 ms per frame and multi-second stalls, ~22 fps recorded).
//!
//! In C# the app called this once at startup; here the engine calls [`apply_once`] when it is armed, so every
//! user of the engine gets it.

use std::sync::Once;
use windows::core::{s, w};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::{
    GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
};

pub fn apply_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(apply);
}

fn apply() {
    unsafe {
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: 0, // never throttle
        };
        if let Err(e) = SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const _ as *const _,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        ) {
            log::info!("[PerformanceGuard] power throttling opt-out failed: {}", e.message());
        }

        // gdi32!D3DKMTSetProcessSchedulingPriorityClass, resolved at runtime (not in the MinGW import libs).
        type SetPrio = unsafe extern "system" fn(HANDLE, i32) -> i32;
        let set: Option<SetPrio> = LoadLibraryW(w!("gdi32.dll"))
            .ok()
            .and_then(|m| GetProcAddress(m, s!("D3DKMTSetProcessSchedulingPriorityClass")))
            .map(|f| std::mem::transmute::<_, SetPrio>(f));
        let Some(set) = set else {
            log::info!("[PerformanceGuard] GPU priority failed: D3DKMTSetProcessSchedulingPriorityClass unavailable");
            return;
        };
        // 5 = Realtime (needs admin), 4 = High, 3 = AboveNormal.
        for cls in [5, 4, 3] {
            if set(GetCurrentProcess(), cls) == 0 {
                log::info!("[PerformanceGuard] GPU scheduling priority class set to {cls}.");
                return;
            }
        }
        log::info!("[PerformanceGuard] GPU scheduling priority unchanged (all classes denied \u{2014} not elevated?).");
    }
}
