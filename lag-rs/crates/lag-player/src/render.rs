//! Mirrors Services/VlcVideoRenderer.cs: composited, clipped video without a native video window.
use crate::{check, Error, Handle, MpvPlayer, Result};
use glow::HasContext;
use lag_ffi::mpv::*;
use std::{
    ffi::{c_void, CStr},
    marker::PhantomData,
    ptr::NonNull,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

struct Update {
    pending: AtomicBool,
    redraw: Box<dyn Fn() + Send + Sync>,
}
unsafe extern "C" fn updated(data: *mut c_void) {
    let update = unsafe { &*data.cast::<Update>() };
    if !update.pending.swap(true, Ordering::AcqRel) {
        // Do not unwind through C. Callers must only enqueue work here (no mpv calls).
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (update.redraw)()));
    }
}
type Loader<'a> = &'a dyn Fn(&CStr) -> *const c_void;
unsafe extern "C" fn get_proc(data: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void {
    let loader = unsafe { &*data.cast::<Loader<'_>>() };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loader(unsafe { CStr::from_ptr(name) }) as *mut c_void))
        .unwrap_or(std::ptr::null_mut())
}

/// A render context confined to its creating GL thread. The caller owns the FBO/texture.
/// Free this before destroying that OpenGL context. No control API calls from render callbacks.
pub struct MpvGlRenderer {
    context: NonNull<mpv_render_context>,
    _handle: Arc<Handle>,
    update: Box<Update>,
    _thread: PhantomData<Rc<()>>,
}
impl MpvGlRenderer {
    /// # Safety
    /// A compatible GL context must be current for construction, render, update and drop.
    /// `get_proc_address` must return valid entry points for that context. It is used only here.
    pub unsafe fn new(
        player: &MpvPlayer,
        get_proc_address: Loader<'_>,
        redraw: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        let mut loader = get_proc_address;
        let mut gl = mpv_opengl_init_params {
            get_proc_address: Some(get_proc),
            get_proc_address_ctx: (&mut loader as *mut Loader<'_>).cast(),
        };
        // Advanced control removes the render API's implicit wait for the video thread.
        let mut advanced: i32 = 1;
        let mut params = [
            mpv_render_param { type_: MPV_RENDER_PARAM_API_TYPE, data: MPV_RENDER_API_TYPE_OPENGL.as_ptr() as *mut _ },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: (&mut gl as *mut mpv_opengl_init_params).cast(),
            },
            mpv_render_param { type_: MPV_RENDER_PARAM_ADVANCED_CONTROL, data: (&mut advanced as *mut i32).cast() },
            mpv_render_param::default(),
        ];
        let mut context = std::ptr::null_mut();
        check(unsafe { mpv_render_context_create(&mut context, player.handle.raw(), params.as_mut_ptr()) })?;
        let context = NonNull::new(context).ok_or_else(|| Error("null mpv render context".into()))?;
        let mut update = Box::new(Update { pending: AtomicBool::new(true), redraw: Box::new(redraw) });
        unsafe {
            mpv_render_context_set_update_callback(
                context.as_ptr(),
                Some(updated),
                (&mut *update as *mut Update).cast(),
            );
        }
        Ok(Self { context, _handle: player.handle.clone(), update, _thread: PhantomData })
    }
    /// Drain mpv's render work on every notified update, including while no clip is visible.
    pub fn needs_render(&self) -> bool {
        if !self.update.pending.swap(false, Ordering::AcqRel) {
            return false;
        }
        unsafe { mpv_render_context_update(self.context.as_ptr()) & u64::from(MPV_RENDER_UPDATE_FRAME) != 0 }
    }
    /// # Safety
    /// `fbo` is a complete RGBA framebuffer in this renderer's current GL context, sized `size`.
    pub unsafe fn render(&self, fbo: u32, size: (u32, u32)) -> Result<()> {
        if size.0 == 0 || size.1 == 0 || size.0 > i32::MAX as u32 || size.1 > i32::MAX as u32 {
            return Err(Error("invalid framebuffer size".into()));
        }
        let mut fbo =
            mpv_opengl_fbo { fbo: fbo as i32, w: size.0 as i32, h: size.1 as i32, internal_format: glow::RGBA8 as i32 };
        // 0: we render into an FBO texture that Slint samples with a top-left origin (1 is only for the
        // default framebuffer) - with 1 the video showed upside down.
        let mut flip: i32 = 0;
        let mut block: i32 = 0;
        let mut params = [
            mpv_render_param { type_: MPV_RENDER_PARAM_OPENGL_FBO, data: (&mut fbo as *mut mpv_opengl_fbo).cast() },
            mpv_render_param { type_: MPV_RENDER_PARAM_FLIP_Y, data: (&mut flip as *mut i32).cast() },
            mpv_render_param { type_: MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME, data: (&mut block as *mut i32).cast() },
            mpv_render_param::default(),
        ];
        check(unsafe { mpv_render_context_render(self.context.as_ptr(), params.as_mut_ptr()) })
    }
    pub fn report_swap(&self) {
        unsafe {
            mpv_render_context_report_swap(self.context.as_ptr());
        }
    }
}
impl Drop for MpvGlRenderer {
    fn drop(&mut self) {
        unsafe {
            // Unregister waits for in-flight callbacks before releasing their stable boxed userdata.
            mpv_render_context_set_update_callback(self.context.as_ptr(), None, std::ptr::null_mut());
            mpv_render_context_free(self.context.as_ptr());
        }
    }
}

/// RGBA target used by callers embedding video into their compositor.
pub struct GlTarget {
    pub texture: glow::NativeTexture,
    pub fbo: glow::NativeFramebuffer,
    pub size: (u32, u32),
}
impl GlTarget {
    /// # Safety
    /// `gl` must be current; release the target with `destroy` before that context is destroyed.
    pub unsafe fn new(gl: &glow::Context, size: (u32, u32)) -> Result<Self> {
        if size.0 == 0 || size.1 == 0 || size.0 > i32::MAX as u32 || size.1 > i32::MAX as u32 {
            return Err(Error("invalid texture size".into()));
        }
        unsafe {
            let texture = gl.create_texture().map_err(Error)?;
            let fbo = match gl.create_framebuffer() {
                Ok(fbo) => fbo,
                Err(e) => {
                    gl.delete_texture(texture);
                    return Err(Error(e));
                }
            };
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                size.0 as i32,
                size.1 as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
            let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            gl.disable(glow::SCISSOR_TEST);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if !complete {
                gl.delete_framebuffer(fbo);
                gl.delete_texture(texture);
                return Err(Error("incomplete player framebuffer".into()));
            }
            Ok(Self { texture, fbo, size })
        }
    }
    /// # Safety
    /// The creating context is current and no compositor image still borrows this texture.
    pub unsafe fn destroy(self, gl: &glow::Context) {
        unsafe {
            gl.delete_framebuffer(self.fbo);
            gl.delete_texture(self.texture);
        }
    }
}
