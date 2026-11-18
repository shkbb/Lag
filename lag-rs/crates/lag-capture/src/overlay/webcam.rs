//! Port of `Services/VfrCapture/WebcamCaptureSource.cs` using WinRT MediaCapture (Media Foundation).
//! Enumeration never initializes a camera or opens a stream.
use std::sync::{Arc, Mutex};
use windows::core::{Error, Interface, Result, HSTRING};
use windows::Devices::Enumeration::{DeviceClass, DeviceInformation};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Capture::Frames::*;
use windows::Media::Capture::*;
use windows::Storage::Streams::Buffer;
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::System::WinRT::{IBufferByteAccess, RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

struct Apartment(bool, std::marker::PhantomData<std::rc::Rc<()>>);
impl Apartment {
    fn new() -> Result<Self> {
        let result = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        // An existing STA is also a valid caller of the asynchronous MediaCapture APIs.
        if result.as_ref().is_err_and(|e| e.code() == windows::Win32::Foundation::RPC_E_CHANGED_MODE) {
            return Ok(Self(false, std::marker::PhantomData));
        }
        result?;
        Ok(Self(true, std::marker::PhantomData))
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                RoUninitialize();
            }
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
}
impl std::fmt::Display for DeviceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}
#[derive(Default)]
struct Slot {
    buffer: Option<Buffer>,
    width: u32,
    height: u32,
    seq: u64,
    disposed: bool,
}
pub struct WebcamCaptureSource {
    reader: Option<MediaFrameReader>,
    token: Option<i64>,
    capture: Option<MediaCapture>,
    slot: Arc<Mutex<Slot>>,
    _apartment: Apartment,
}
impl WebcamCaptureSource {
    pub fn list_devices() -> Vec<DeviceInfo> {
        let query = || -> Result<Vec<DeviceInfo>> {
            let _apartment = Apartment::new()?;
            let devices = DeviceInformation::FindAllAsyncDeviceClass(DeviceClass::VideoCapture)?.join()?;
            let mut all = Vec::new();
            for d in &devices {
                all.push(DeviceInfo { id: d.Id()?.to_string(), name: d.Name()?.to_string() });
            }
            Ok(all)
        };
        query().unwrap_or_else(|e| {
            log::info!("[Webcam] enumeration failed: {}", e.message());
            Vec::new()
        })
    }
    pub fn new() -> Result<Self> {
        Ok(Self {
            reader: None,
            token: None,
            capture: None,
            slot: Arc::new(Mutex::new(Slot::default())),
            _apartment: Apartment::new()?,
        })
    }
    /// `want` = (width, height, fps) from the settings' webcam mode; zeros = Auto.
    pub fn start(&mut self, device_id: Option<&str>, want: (u32, u32, u32)) -> Result<()> {
        let settings = MediaCaptureInitializationSettings::new()?;
        settings.SetStreamingCaptureMode(StreamingCaptureMode::Video)?;
        settings.SetMemoryPreference(MediaCaptureMemoryPreference::Cpu)?;
        if let Some(id) = device_id.filter(|s| !s.is_empty()) {
            settings.SetVideoDeviceId(&HSTRING::from(id))?;
        }
        let capture = MediaCapture::new()?;
        self.capture = Some(capture.clone());
        capture.InitializeWithSettingsAsync(&settings)?.join()?;
        let mut source = None;
        for pair in capture.FrameSources()? {
            let s = pair.Value()?;
            if s.Info()?.SourceKind()? == MediaFrameSourceKind::Color {
                source = Some(s);
                break;
            }
        }
        let source = source.ok_or_else(|| Error::new(E_FAIL, "camera has no color stream"))?;
        let mut formats = Vec::new();
        let mut offered = Vec::new();
        for f in source.SupportedFormats()? {
            if let Ok(v) = f.VideoFormat() {
                let fps = fps(&f)?;
                let subtype = f.Subtype().map(|s| s.to_string()).unwrap_or_default();
                offered.push(format!("{}x{}@{:.0} {}", v.Width()?, v.Height()?, fps, subtype));
                let mode =
                    CameraMode { width: v.Width()?, height: v.Height()?, fps, compressed: is_compressed(&subtype) };
                formats.push((mode, f));
            }
        }
        let modes: Vec<CameraMode> = formats.iter().map(|(m, _)| *m).collect();
        let best = pick_mode(&modes, want).map(|i| &formats[i]);
        if let Some((_, format)) = best {
            let _ = source.SetFormatAsync(format).and_then(|op| op.join());
        }
        if want.0 > 0 {
            log::info!("[Webcam] requested {}x{}@{}; camera offers: {}", want.0, want.1, want.2, offered.join(", "));
        }
        // A compressed mode (MJPG at 1080p60 on most USB cameras) has no CPU SoftwareBitmap: ask the reader to
        // decode to BGRA. Uncompressed modes keep the plain reader (our own NV12/YUY2 -> BGRA convert).
        let reader = match best {
            Some((m, _)) if m.compressed => {
                let bgra = windows::Media::MediaProperties::MediaEncodingSubtypes::Bgra8()?;
                capture.CreateFrameReaderWithSubtypeAsync(&source, &bgra)?.join()?
            }
            _ => capture.CreateFrameReaderAsync(&source)?.join()?,
        };
        self.reader = Some(reader.clone());
        reader.SetAcquisitionMode(MediaFrameReaderAcquisitionMode::Realtime)?;
        let slot = self.slot.clone();
        self.token = Some(reader.FrameArrived(
            &TypedEventHandler::<MediaFrameReader, MediaFrameArrivedEventArgs>::new(move |sender, _| {
                let Some(sender) = sender.as_ref() else {
                    return Ok(());
                };
                let result = (|| -> Result<()> {
                    let frame = match sender.TryAcquireLatestFrame() {
                        Ok(f) => f,
                        Err(_) => return Ok(()),
                    };
                    let result = store_frame(&frame, &slot);
                    let _ = frame.Close();
                    result
                })();
                if let Err(e) = result {
                    log::info!("[Webcam] frame error: {}", e.message());
                }
                Ok(())
            }),
        )?);
        let status = reader.StartAsync()?.join()?;
        if status != MediaFrameReaderStartStatus::Success {
            return Err(Error::new(E_FAIL, format!("frame reader start: {}", start_status_name(status))));
        }
        let name = source.Info()?.DeviceInformation().and_then(|d| d.Name()).unwrap_or_default();
        let (width, height, rate) = if let Some((m, _)) = best {
            (m.width.to_string(), m.height.to_string(), m.fps)
        } else {
            (String::new(), String::new(), 0.0)
        };
        log::info!("[Webcam] streaming '{}' {}x{}@{:.0}.", name, width, height, rate);
        Ok(())
    }
    /// False leaves caller storage/sequence untouched; dimensions exist only on a new frame.
    pub fn try_copy_latest(&self, output: &mut Vec<u8>, last_seq: &mut u64) -> Result<Option<(u32, u32)>> {
        let slot = self.slot.lock().map_err(|_| Error::new(E_FAIL, "camera frame lock poisoned"))?;
        let Some(buffer) = &slot.buffer else {
            return Ok(None);
        };
        if slot.seq == *last_seq {
            return Ok(None);
        }
        let len = buffer.Length()? as usize;
        let access: IBufferByteAccess = buffer.cast()?;
        let ptr = unsafe { access.Buffer()? };
        if ptr.is_null() && len != 0 {
            return Err(Error::new(E_FAIL, "camera frame buffer is null"));
        }
        output.resize(len, 0);
        if len > 0 {
            unsafe {
                output.copy_from_slice(std::slice::from_raw_parts(ptr, len));
            }
        }
        *last_seq = slot.seq;
        Ok(Some((slot.width, slot.height)))
    }
}
fn start_status_name(status: MediaFrameReaderStartStatus) -> String {
    match status {
        MediaFrameReaderStartStatus::Success => "Success".into(),
        MediaFrameReaderStartStatus::UnknownFailure => "UnknownFailure".into(),
        MediaFrameReaderStartStatus::DeviceNotAvailable => "DeviceNotAvailable".into(),
        MediaFrameReaderStartStatus::OutputFormatNotSupported => "OutputFormatNotSupported".into(),
        MediaFrameReaderStartStatus::ExclusiveControlNotAvailable => "ExclusiveControlNotAvailable".into(),
        other => other.0.to_string(),
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
struct CameraMode {
    width: u32,
    height: u32,
    fps: f64,
    compressed: bool,
}
fn is_compressed(subtype: &str) -> bool {
    let s = subtype.to_ascii_uppercase();
    s.contains("MJPG") || s.contains("H264") || s.contains("HEVC") || s.contains("H265")
}
/// Index of the camera mode closest to `want` = (width, height, fps), zeros = Auto (~640 wide at ~30 fps).
/// Order: nearest size (by height when one is asked for), then the rate nearest the wanted one (ties: the
/// faster), then uncompressed (no MJPG decode). Modes under 15 fps only when nothing else exists.
/// C# took the LOWEST rate >= 15, so a camera offering 15/30/60 always streamed at 15 fps and the
/// picture-in-picture stuttered over a 60 fps recording (settings audit 2026-11-17: StreamCam -> 640x480@15).
fn pick_mode(modes: &[CameraMode], want: (u32, u32, u32)) -> Option<usize> {
    let (ww, wh, wf) = if want.0 == 0 { (640, 0, 30) } else { want };
    let rate_rank = |fps: f64| ((fps - wf as f64).abs() * 100.0) as i64 * 2 + i64::from(fps < wf as f64);
    let size_rank = |m: &CameraMode| {
        if wh > 0 {
            m.height.abs_diff(wh) * 4 + m.width.abs_diff(ww)
        } else {
            m.width.abs_diff(ww)
        }
    };
    (0..modes.len()).min_by_key(|&i| {
        let m = &modes[i];
        (m.fps < 14.5, size_rank(m), rate_rank(m.fps), m.compressed)
    })
}
fn fps(f: &MediaFrameFormat) -> Result<f64> {
    let ratio = f.FrameRate()?;
    let denominator = ratio.Denominator()?;
    Ok(if denominator > 0 { ratio.Numerator()? as f64 / denominator as f64 } else { 0.0 })
}
fn store_frame(frame: &MediaFrameReference, slot: &Mutex<Slot>) -> Result<()> {
    let bitmap = match frame.VideoMediaFrame().and_then(|v| v.SoftwareBitmap()) {
        Ok(b) => b,
        Err(_) => return Ok(()),
    };
    let converted = bitmap.BitmapPixelFormat()? != BitmapPixelFormat::Bgra8
        || bitmap.BitmapAlphaMode()? != BitmapAlphaMode::Premultiplied;
    let bgra = if converted {
        SoftwareBitmap::ConvertWithAlpha(&bitmap, BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied)?
    } else {
        bitmap
    };
    let result = (|| -> Result<()> {
        let width = bgra.PixelWidth()? as u32;
        let height = bgra.PixelHeight()? as u32;
        let bytes = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| Error::new(E_FAIL, "camera frame too large"))?;
        let mut slot = slot.lock().map_err(|_| Error::new(E_FAIL, "camera frame lock poisoned"))?;
        if slot.disposed {
            return Ok(());
        }
        if slot.buffer.as_ref().and_then(|b| b.Capacity().ok()) != Some(bytes) {
            slot.buffer = Some(Buffer::Create(bytes)?);
        }
        if let Some(buffer) = &slot.buffer {
            buffer.SetLength(bytes)?;
            bgra.CopyToBuffer(buffer)?;
        }
        slot.width = width;
        slot.height = height;
        slot.seq = slot.seq.wrapping_add(1);
        Ok(())
    })();
    if converted {
        let _ = bgra.Close();
    }
    result
}
impl Drop for WebcamCaptureSource {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.slot.lock() {
            slot.disposed = true;
        }
        if let Some(reader) = self.reader.take() {
            if let Some(token) = self.token.take() {
                let _ = reader.RemoveFrameArrived(token);
            }
            let _ = reader.StopAsync().and_then(|op| op.join());
            let _ = reader.Close();
        }
        if let Some(capture) = self.capture.take() {
            let _ = capture.Close();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enumeration_does_not_open_camera() {
        for d in WebcamCaptureSource::list_devices() {
            assert!(!d.id.is_empty());
            assert_eq!(d.to_string(), d.name);
        }
    }
    #[test]
    fn mode_choice() {
        let m = |width, height, fps, compressed| CameraMode { width, height, fps, compressed };
        let cam = [
            m(640, 480, 15.0, false),
            m(640, 480, 30.0, false),
            m(640, 480, 60.0, true),
            m(1280, 720, 30.0, false),
            m(1280, 720, 60.0, true),
            m(1280, 720, 60.0, false),
            m(1920, 1080, 30.0, true),
            m(1920, 1080, 60.0, true),
            m(160, 120, 5.0, false),
        ];
        let pick = |w| cam[pick_mode(&cam, w).unwrap()];
        assert_eq!(pick((0, 0, 0)), m(640, 480, 30.0, false)); // Auto: modest, 30 fps (not 15)
        assert_eq!(pick((1280, 720, 60)), m(1280, 720, 60.0, false)); // uncompressed wins a tie
        assert_eq!(pick((1920, 1080, 60)), m(1920, 1080, 60.0, true));
        assert_eq!(pick((1920, 1080, 30)), m(1920, 1080, 30.0, true));
        // a 720p-only camera asked for 1080p60 gets its 720p60
        let small = [m(1280, 720, 30.0, false), m(1280, 720, 60.0, false), m(640, 480, 30.0, false)];
        assert_eq!(small[pick_mode(&small, (1920, 1080, 60)).unwrap()], m(1280, 720, 60.0, false));
        // only a slow mode exists -> still used
        assert_eq!(pick_mode(&[m(640, 480, 10.0, false)], (0, 0, 0)), Some(0));
        assert_eq!(pick_mode(&[], (0, 0, 0)), None);
    }
    #[test]
    fn empty_latest_slot() {
        let source = WebcamCaptureSource::new().unwrap();
        let mut pixels = vec![42];
        let mut seq = 0;
        assert_eq!(source.try_copy_latest(&mut pixels, &mut seq).unwrap(), None);
        assert_eq!(pixels, vec![42]);
    }
}
