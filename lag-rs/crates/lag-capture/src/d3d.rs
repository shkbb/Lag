//! Port of `D3D11Context.cs`: the single Direct3D11 device the whole pipeline shares (WGC delivers
//! frames on it, the colour converter runs on it, FFmpeg's hwframes wrap its textures), so every frame
//! stays on ONE GPU end to end.

use windows::core::{Interface, Result};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIDevice, IDXGIFactory1};

pub const VENDOR_NVIDIA: u32 = 0x10DE;
pub const VENDOR_AMD: u32 = 0x1002;
pub const VENDOR_INTEL: u32 = 0x8086;

/// Shared D3D11 device + immediate context.
///
/// Thread-safety: the device is created multithread-protected (`ID3D11Multithread::SetMultithreadProtected`)
/// exactly like C#, and the engine still serialises its own use of the immediate context. That makes it
/// sound to share across the WGC callback thread, the encoder and teardown.
pub struct D3D11Context {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub adapter_name: String,
    /// PCI vendor id (0x10DE NVIDIA, 0x1002 AMD, 0x8086 Intel): picks a hardware encoder of the SAME vendor.
    pub vendor_id: u32,
}

// SAFETY: see the struct docs: the device is multithread-protected and callers serialise context use.
unsafe impl Send for D3D11Context {}
unsafe impl Sync for D3D11Context {}

impl D3D11Context {
    /// `adapter_index` selects a DXGI adapter (the user's GPU choice); `-1` = default hardware adapter.
    pub fn new(adapter_index: i32) -> Result<Self> {
        let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
        let mut device = None;
        let mut context = None;
        let mut created = false;
        if adapter_index >= 0 {
            let chosen = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }
                .and_then(|f| unsafe { f.EnumAdapters1(adapter_index as u32) });
            match chosen {
                Ok(adapter) => {
                    let adapter: IDXGIAdapter = adapter.cast()?;
                    let r = unsafe {
                        D3D11CreateDevice(
                            &adapter,
                            D3D_DRIVER_TYPE_UNKNOWN,
                            HMODULE::default(),
                            flags,
                            None,
                            D3D11_SDK_VERSION,
                            Some(&mut device),
                            None,
                            Some(&mut context),
                        )
                    };
                    match r {
                        Ok(()) => created = true,
                        Err(e) => log::info!(
                            "[D3D11Context] adapter {adapter_index} unavailable ({}); using default.",
                            e.message()
                        ),
                    }
                }
                Err(e) => {
                    log::info!("[D3D11Context] adapter {adapter_index} unavailable ({}); using default.", e.message())
                }
            }
        }
        if !created {
            unsafe {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    HMODULE::default(),
                    flags,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )?;
            }
        }
        let device = device.expect("D3D11CreateDevice returned no device");
        let context = context.expect("D3D11CreateDevice returned no context");

        let dxgi: IDXGIDevice = device.cast()?;
        let desc = unsafe { dxgi.GetAdapter()?.GetDesc()? };
        let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        let adapter_name = String::from_utf16_lossy(&desc.Description[..len]);

        // Driver-internal locking: the device is used from the WGC callback thread, the converter and
        // the encoder submit path. Defence in depth on top of our own serialisation.
        let mt: ID3D11Multithread = device.cast()?;
        unsafe {
            let _ = mt.SetMultithreadProtected(true);
        }
        log::info!("[D3D11Context] Device created on adapter: {adapter_name}");
        Ok(Self { device, context, adapter_name, vendor_id: desc.VendorId })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_default_device() {
        let d = D3D11Context::new(-1).expect("D3D11 device");
        assert!(!d.adapter_name.is_empty());
        eprintln!("adapter: {} vendor {:#06x}", d.adapter_name, d.vendor_id);
    }

    #[test]
    fn bad_adapter_index_falls_back() {
        let d = D3D11Context::new(99).expect("fallback device");
        assert!(!d.adapter_name.is_empty());
    }
}
