//! Hand-written mirror of libavutil/hwcontext_d3d11va.h (FFmpeg 7.1). Kept out of bindgen so the
//! generator never has to parse <d3d11.h>. COM pointers are raw `*mut c_void`; callers pass
//! `windows::Win32::Graphics::Direct3D11` interface raw pointers (`Interface::as_raw`).
#![allow(non_snake_case)]
use std::ffi::c_void;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AVD3D11VADeviceContext {
    pub device: *mut c_void,
    pub device_context: *mut c_void,
    pub video_device: *mut c_void,
    pub video_context: *mut c_void,
    pub lock: Option<unsafe extern "C" fn(lock_ctx: *mut c_void)>,
    pub unlock: Option<unsafe extern "C" fn(lock_ctx: *mut c_void)>,
    pub lock_ctx: *mut c_void,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AVD3D11FrameDescriptor {
    pub texture: *mut c_void,
    pub index: isize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AVD3D11VAFramesContext {
    pub texture: *mut c_void,
    pub BindFlags: u32,
    pub MiscFlags: u32,
    pub texture_infos: *mut AVD3D11FrameDescriptor,
}
