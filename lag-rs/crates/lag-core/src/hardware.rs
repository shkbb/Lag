//! Mirrors Services/HardwareDetector.cs and Services/HardwareProfiler.cs.
//! Read-only Win32/DXGI/WASAPI probes; encoder availability is supplied by the capture engine.

use std::{
    fmt,
    path::{Path, PathBuf},
};
use windows::{
    core::{w, BOOL, GUID, PCWSTR, PWSTR},
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Foundation::{HWND, LPARAM, RECT},
        Graphics::{
            Dxgi::{CreateDXGIFactory1, IDXGIFactory1},
            Gdi::*,
        },
        Media::Audio::*,
        Storage::FileSystem::GetDiskFreeSpaceExW,
        System::{Com::*, Registry::*, SystemInformation::*},
        UI::Shell::{
            FOLDERID_Documents, FOLDERID_LocalAppData, FOLDERID_Videos, SHGetKnownFolderPath, KF_FLAG_DEFAULT,
        },
    },
};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// `F1` formatting follows the Windows user culture, as C# interpolated labels do.
pub fn fixed_one(value: f64) -> String {
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SDECIMAL};
    let mut buffer = [0u16; 16];
    // A null locale name selects LOCALE_NAME_USER_DEFAULT. No global locale state is changed.
    let count = unsafe { GetLocaleInfoEx(PCWSTR::null(), LOCALE_SDECIMAL, Some(&mut buffer)) };
    let decimal = if count > 1 { utf16(&buffer) } else { ".".into() };
    format!("{value:.1}").replace('.', &decimal)
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn utf16(s: &[u16]) -> String {
    String::from_utf16_lossy(&s[..s.iter().position(|c| *c == 0).unwrap_or(s.len())])
}
unsafe fn take_string(p: PWSTR) -> String {
    let text = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0.cast())) };
    text
}
fn known_folder(id: &GUID) -> Option<PathBuf> {
    // SAFETY: the shell returns an allocated null-terminated UTF-16 string, freed exactly once.
    unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok().map(|p| PathBuf::from(take_string(p))) }
}
pub fn documents() -> Option<PathBuf> {
    known_folder(&FOLDERID_Documents)
}
pub fn local_appdata() -> Option<PathBuf> {
    known_folder(&FOLDERID_LocalAppData)
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct GpuProfile {
    pub index: i32,
    pub name: String,
    pub vendor_id: u32,
    pub dedicated_vram_bytes: i64,
    pub shared_memory_bytes: i64,
}
impl GpuProfile {
    pub fn vendor(&self) -> String {
        match self.vendor_id {
            0x10de => "NVIDIA".into(),
            0x1002 => "AMD".into(),
            0x8086 => "Intel".into(),
            0x1414 => "Microsoft (software)".into(),
            id => format!("0x{id:04X}"),
        }
    }
    pub fn vram_gib(&self) -> f64 {
        self.dedicated_vram_bytes as f64 / GIB
    }
    pub fn is_integrated(&self) -> bool {
        self.dedicated_vram_bytes < 1024 * 1024 * 1024
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuInfo {
    pub index: i32,
    pub name: String,
}
impl fmt::Display for GpuInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// DXGI output identity, useful for binding capture/vblank without changing GDI monitor order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputInfo {
    pub adapter_index: i32,
    pub output_index: u32,
    pub device_name: String,
    pub attached_to_desktop: bool,
}
pub fn get_dxgi_outputs() -> Vec<OutputInfo> {
    let mut found = Vec::new();
    // SAFETY: DXGI COM interfaces own their references; API structures are initialized by DXGI.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return found;
        };
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            let mut j = 0;
            while let Ok(output) = adapter.EnumOutputs(j) {
                if let Ok(d) = output.GetDesc() {
                    found.push(OutputInfo {
                        adapter_index: i as i32,
                        output_index: j,
                        device_name: utf16(&d.DeviceName),
                        attached_to_desktop: d.AttachedToDesktop.as_bool(),
                    });
                }
                j += 1;
            }
            i += 1;
        }
    }
    found
}
pub fn probe_gpus() -> Vec<GpuProfile> {
    let mut found = Vec::new();
    // SAFETY: DXGI interfaces are RAII-owned and GetDesc1 fills its own value.
    let result = unsafe {
        (|| -> windows::core::Result<()> {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut i = 0;
            while let Ok(adapter) = factory.EnumAdapters1(i) {
                let d = adapter.GetDesc1()?;
                if d.VendorId != 0x1414 {
                    found.push(GpuProfile {
                        index: i as i32,
                        name: utf16(&d.Description),
                        vendor_id: d.VendorId,
                        dedicated_vram_bytes: d.DedicatedVideoMemory as i64,
                        shared_memory_bytes: d.SharedSystemMemory as i64,
                    });
                }
                i += 1;
            }
            Ok(())
        })()
    };
    if let Err(e) = result {
        log::info!("[HardwareProfiler] GPU probe failed: {e}");
    }
    found
}
pub fn get_gpu_adapters() -> Vec<GpuInfo> {
    let mut gpus: Vec<_> = probe_gpus().into_iter().map(|g| GpuInfo { index: g.index, name: g.name }).collect();
    if gpus.is_empty() {
        gpus.push(GpuInfo { index: 0, name: "Default GPU".into() });
    }
    gpus
}
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    pub index: i32,
    pub device_name: String,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
    pub refresh_rate: u32,
    pub stable_id: String,
}
impl fmt::Display for MonitorInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hz = if self.refresh_rate > 0 { format!(" @{}Hz", self.refresh_rate) } else { String::new() };
        write!(
            f,
            "{} ({}x{}{}){}",
            self.device_name,
            self.width,
            self.height,
            hz,
            if self.is_primary { " [Primary]" } else { "" }
        )
    }
}
pub fn query_refresh_rate(device: &str) -> u32 {
    let device = wide(device);
    let mut mode = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
    // SAFETY: NUL-terminated input, correctly sized writable DEVMODEW.
    if unsafe { EnumDisplaySettingsW(PCWSTR(device.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode) }.as_bool()
        && mode.dmDisplayFrequency > 1
    {
        mode.dmDisplayFrequency
    } else {
        0
    }
}
pub fn query_monitor_stable_id(device: &str) -> String {
    let device = wide(device);
    let mut dd = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
    // EDD_GET_DEVICE_INTERFACE_NAME = 1: DeviceID is the PnP interface path, not hardware ID.
    if unsafe { EnumDisplayDevicesW(PCWSTR(device.as_ptr()), 0, &mut dd, 1) }.as_bool() {
        utf16(&dd.DeviceID)
    } else {
        String::new()
    }
}
fn monitor_info(monitor: HMONITOR) -> Option<MONITORINFOEXW> {
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO { cbSize: size_of::<MONITORINFOEXW>() as u32, ..Default::default() },
        ..Default::default()
    };
    // SAFETY: MONITORINFOEXW begins with MONITORINFO and cbSize selects the larger structure.
    unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) }.as_bool().then_some(info)
}
unsafe extern "system" fn enumerate_monitor(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    if let Some(info) = monitor_info(monitor) {
        // SAFETY: EnumDisplayMonitors invokes this synchronously, while the Vec is exclusively borrowed.
        let list = unsafe { &mut *(data.0 as *mut Vec<MonitorInfo>) };
        let name = utf16(&info.szDevice);
        let bounds = info.monitorInfo.rcMonitor;
        list.push(MonitorInfo {
            index: list.len() as i32,
            width: (bounds.right - bounds.left) as u32,
            height: (bounds.bottom - bounds.top) as u32,
            is_primary: info.monitorInfo.dwFlags & 1 != 0,
            refresh_rate: query_refresh_rate(&name),
            stable_id: query_monitor_stable_id(&name),
            device_name: name,
        });
    }
    BOOL(1)
}
pub fn get_available_monitors() -> Vec<MonitorInfo> {
    let mut list = Vec::new();
    // SAFETY: callback data remains valid for the synchronous enumeration.
    let _ = unsafe {
        EnumDisplayMonitors(None, None, Some(enumerate_monitor), LPARAM(&mut list as *mut Vec<MonitorInfo> as isize))
    };
    if list.is_empty() {
        log::debug!("[HardwareDetector] No monitors detected via native APIs. Using 1920x1080 default.");
        list.push(MonitorInfo {
            device_name: "Default Display".into(),
            width: 1920,
            height: 1080,
            is_primary: true,
            ..Default::default()
        });
    }
    list
}
/// Read-only monitor lookup; accepts a borrowed existing HWND and never creates a window.
pub fn get_window_monitor(hwnd: HWND) -> HMONITOR {
    if hwnd.0.is_null() {
        HMONITOR::default()
    } else {
        unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }
    }
}
pub fn get_window_refresh_rate(hwnd: HWND) -> u32 {
    monitor_info(get_window_monitor(hwnd)).map_or(0, |i| query_refresh_rate(&utf16(&i.szDevice)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicrophoneInfo {
    pub id: String,
    pub name: String,
}
pub type OutputDeviceInfo = MicrophoneInfo;
struct ComGuard(bool);
impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}
fn endpoints(flow: EDataFlow, list: &mut Vec<MicrophoneInfo>) -> windows::core::Result<()> {
    use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
    // SAFETY: COM calls operate on owned interfaces. Balance successful CoInitializeEx (incl S_FALSE),
    // leaving an existing STA apartment intact when initialization returns RPC_E_CHANGED_MODE.
    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _guard = ComGuard(hr.is_ok());
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let devices = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
        for i in 0..devices.GetCount()? {
            let endpoint = devices.Item(i)?;
            let id = take_string(endpoint.GetId()?);
            let store = endpoint.OpenPropertyStore(STGM_READ)?;
            let mut property = store.GetValue(&PKEY_Device_FriendlyName)?;
            let name = PropVariantToStringAlloc(&property).map(|p| take_string(p));
            let _ = PropVariantClear(&mut property);
            list.push(MicrophoneInfo { id, name: name? });
        }
        Ok(())
    }
}
pub fn get_microphones() -> Vec<MicrophoneInfo> {
    let mut mics = Vec::new();
    if let Err(e) = endpoints(eCapture, &mut mics) {
        log::debug!("[HardwareDetector] WASAPI mic enumeration failed: {e}");
    }
    if mics.is_empty() {
        mics.push(MicrophoneInfo { id: "default".into(), name: "Default Microphone".into() });
    }
    mics
}
pub fn get_output_devices() -> Vec<OutputDeviceInfo> {
    let mut outputs = Vec::new();
    if let Err(e) = endpoints(eRender, &mut outputs) {
        log::debug!("[HardwareDetector] WASAPI output enumeration failed: {e}");
    }
    outputs
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecTier {
    Av1,
    Hevc,
    H264,
}
/// Results of actually opening encoders in the capture service, in its original preference order.
#[derive(Debug, Clone)]
pub struct EncoderCapability {
    pub friendly_name: String,
    pub tier: CodecTier,
    pub is_hardware: bool,
}
#[derive(Debug, Default, Clone)]
pub struct MachineProfile {
    pub gpus: Vec<GpuProfile>,
    pub primary_gpu: Option<GpuProfile>,
    pub hardware_encoders: Vec<String>,
    pub has_av1: bool,
    pub has_hevc: bool,
    pub has_h264: bool,
    pub cpu_name: String,
    pub cpu_logical_cores: i32,
    pub ram_gib: f64,
    pub library_drive: String,
    pub disk_free_gib: f64,
    pub disk_total_gib: f64,
    pub monitor_width: i32,
    pub monitor_height: i32,
    pub monitor_hz: i32,
}
impl MachineProfile {
    /// The capture engine supplies its cached EncoderSelector probe results. No benchmarks run here.
    pub fn capture(library_path: Option<&Path>, encoders: &[EncoderCapability]) -> Self {
        let gpus = probe_gpus();
        // Preserve first adapter on equal VRAM, matching stable OrderByDescending().FirstOrDefault().
        let primary_gpu = gpus
            .iter()
            .enumerate()
            .max_by_key(|(i, g)| (g.dedicated_vram_bytes, std::cmp::Reverse(*i)))
            .map(|(_, g)| g.clone());
        let cpu_name =
            registry_string(w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"), w!("ProcessorNameString"))
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().into())
                .unwrap_or_else(|| "Unknown CPU".into());
        let mut memory = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        let ram_gib =
            if unsafe { GlobalMemoryStatusEx(&mut memory) }.is_ok() { memory.ullTotalPhys as f64 / GIB } else { 0.0 };
        let (library_drive, disk_free_gib, disk_total_gib) = probe_disk(library_path);
        let monitors = get_available_monitors();
        let monitor = monitors.iter().find(|m| m.is_primary).or_else(|| monitors.first());
        Self {
            primary_gpu,
            gpus,
            hardware_encoders: encoders.iter().filter(|e| e.is_hardware).map(|e| e.friendly_name.clone()).collect(),
            has_av1: encoders.iter().any(|e| e.tier == CodecTier::Av1),
            has_hevc: encoders.iter().any(|e| e.tier == CodecTier::Hevc),
            has_h264: encoders.iter().any(|e| e.tier == CodecTier::H264),
            cpu_name,
            cpu_logical_cores: std::thread::available_parallelism().map_or(1, |n| n.get() as i32),
            ram_gib,
            library_drive,
            disk_free_gib,
            disk_total_gib,
            monitor_width: monitor.map_or(0, |m| m.width as i32),
            monitor_height: monitor.map_or(0, |m| m.height as i32),
            monitor_hz: monitor.map_or(0, |m| m.refresh_rate as i32),
        }
    }
    pub fn describe(&self) -> String {
        let mut lines = vec!["===== Hardware profile =====".into()];
        for g in &self.gpus {
            lines.push(format!(
                "  GPU{}: {} [{}] VRAM {} GiB{}{}",
                g.index,
                g.name,
                g.vendor(),
                fixed_one(g.vram_gib()),
                if g.is_integrated() { " (integrated)" } else { "" },
                if self.primary_gpu.as_ref() == Some(g) { "  <- primary" } else { "" }
            ));
        }
        lines.push(format!(
            "  HW encoders: {}",
            if self.hardware_encoders.is_empty() {
                "none (CPU x264 only)".into()
            } else {
                self.hardware_encoders.join(", ")
            }
        ));
        let yes = |b| if b { "yes" } else { "no" };
        lines.push(format!(
            "  Codecs: AV1={} HEVC={} H264={}",
            yes(self.has_av1),
            yes(self.has_hevc),
            yes(self.has_h264)
        ));
        lines.push(format!("  CPU: {} ({} logical)", self.cpu_name, self.cpu_logical_cores));
        lines.push(format!("  RAM: {} GiB", fixed_one(self.ram_gib)));
        lines.push(format!(
            "  Disk ({}): {} GiB free / {} GiB",
            self.library_drive,
            fixed_one(self.disk_free_gib),
            fixed_one(self.disk_total_gib)
        ));
        lines.push(format!("  Monitor: {}x{} @ {}Hz", self.monitor_width, self.monitor_height, self.monitor_hz));
        lines.push("============================".into());
        lines.join("\r\n")
    }
    pub fn log_report(&self) {
        log::info!("{}", self.describe());
    }
}
pub(crate) fn registry_string(key: PCWSTR, value: PCWSTR) -> Option<String> {
    let mut bytes = 0;
    unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key, value, RRF_RT_REG_SZ, None, None, Some(&mut bytes)) }.ok().ok()?;
    let mut data = vec![0u16; bytes as usize / 2];
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    }
    .ok()
    .ok()?;
    Some(utf16(&data))
}
fn probe_disk(path: Option<&Path>) -> (String, f64, f64) {
    let path = path
        .filter(|p| !p.as_os_str().is_empty())
        .map(PathBuf::from)
        .or_else(|| known_folder(&FOLDERID_Videos).map(|p| p.join("Lag")));
    let Some(path) = path.and_then(|p| std::path::absolute(p).ok()) else {
        return ("?".into(), 0.0, 0.0);
    };
    let mut root = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => root.push(part),
            _ => break,
        }
    }
    let root_string = root.to_string_lossy().into_owned();
    let root_wide = wide(&root_string);
    let (mut free, mut total) = (0, 0);
    if unsafe { GetDiskFreeSpaceExW(PCWSTR(root_wide.as_ptr()), Some(&mut free), Some(&mut total), None) }.is_ok() {
        (root_string, free as f64 / GIB, total as f64 / GIB)
    } else {
        ("?".into(), 0.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headless_read_only_probes() {
        let p = MachineProfile::capture(
            None,
            &[EncoderCapability {
                friendly_name: "H.264 (CPU / x264)".into(),
                tier: CodecTier::H264,
                is_hardware: false,
            }],
        );
        assert!(p.cpu_logical_cores > 0);
        assert!(p.ram_gib > 0.0);
        assert!(p.has_h264);
        assert!(p.hardware_encoders.is_empty());
        assert!(!get_gpu_adapters().is_empty());
        assert!(!get_available_monitors().is_empty());
        assert_eq!(get_window_refresh_rate(HWND::default()), 0);
        for output in get_dxgi_outputs() {
            assert!(output.device_name.starts_with(r"\\.\DISPLAY"));
        }
        assert!(!get_microphones().is_empty());
        let _ = get_output_devices();
        assert!(p.describe().contains("H264=yes\r\n"));
    }
}
