//! The microphone level meter and "hear yourself" on a worker thread.

use crate::AppWindow;
use crate::ComponentHandle;
use std::{
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

/// Retained by the settings listener; all native audio operations run on this worker.
#[derive(Clone, PartialEq)]
pub(super) struct MonitorConfig {
    pub(super) active: bool,
    pub(super) mic: Option<String>,
    pub(super) output: Option<String>,
    pub(super) auto: bool,
    pub(super) threshold: i32,
    pub(super) denoise: bool,
    pub(super) listen: bool,
}
#[derive(Default)]
pub(super) struct Peaks {
    pub(super) mic: f32,
    pub(super) output: f32,
    pub(super) threshold: f32,
    pub(super) open: bool,
}
pub(super) struct AudioWorker {
    pub(super) send: mpsc::Sender<MonitorConfig>,
    pub(super) peaks: Arc<Mutex<Peaks>>,
}
impl AudioWorker {
    pub(super) fn new() -> Self {
        let (send, recv) = mpsc::channel::<MonitorConfig>();
        let peaks = Arc::new(Mutex::new(Peaks::default()));
        let target = peaks.clone();
        std::thread::spawn(move || {
            let mut meter = lag_audio::monitor::AudioMonitor::new();
            let mut previous: Option<MonitorConfig> = None;
            loop {
                match recv.recv_timeout(Duration::from_millis(33)) {
                    Ok(mut cfg) => {
                        while let Ok(next) = recv.try_recv() {
                            cfg = next;
                        }
                        let restart = previous
                            .as_ref()
                            .is_none_or(|p| p.active != cfg.active || p.mic != cfg.mic || p.output != cfg.output);
                        if restart {
                            if cfg.active {
                                meter.start(
                                    cfg.mic.clone().filter(|s| !s.is_empty() && s != "default"),
                                    cfg.output.clone().filter(|s| !s.is_empty()),
                                );
                            } else {
                                meter.stop();
                            }
                        }
                        if cfg.active {
                            meter.set_input_gate(cfg.auto, cfg.threshold as f32 / 100.);
                            meter.set_noise_suppression(cfg.denoise);
                            if restart || previous.as_ref().is_none_or(|p| p.listen != cfg.listen) {
                                meter.set_monitoring(cfg.listen, cfg.output.clone().filter(|s| !s.is_empty()));
                            }
                        }
                        previous = Some(cfg);
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if previous.as_ref().is_some_and(|p| p.active) {
                    *target.lock().unwrap() = Peaks {
                        mic: meter.read_mic_peak(),
                        output: meter.read_output_peak(),
                        threshold: meter.gate_threshold(),
                        open: meter.gate_open(),
                    };
                } else {
                    *target.lock().unwrap() = Peaks::default();
                }
            }
        });
        Self { send, peaks }
    }
}
pub(super) fn smooth(current: f32, raw: f32) -> f32 {
    let target = raw.clamp(0., 1.).powf(0.6);
    current + (target - current) * if target > current { 0.5 } else { 0.16 }
}
pub(super) fn window_active(ui: &AppWindow) -> bool {
    ui.window().is_visible()
        && crate::shell::nofocus::hwnd(ui.window())
            .is_some_and(|hwnd| unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() == hwnd })
}
