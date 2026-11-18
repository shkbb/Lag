//! Mirrors Services/VfrCapture/MicDenoiser.cs: abuffer → arnndn → aformat → abuffersink.
use crate::{check, AudioError, Result};
use lag_ffi::ffmpeg as ff;
use std::{
    ffi::{CStr, CString},
    path::PathBuf,
    ptr::{null, null_mut},
};
pub struct MicDenoiser {
    pub enabled: bool,
    model: PathBuf,
    failed: bool,
    rate: u32,
    channels: u16,
    graph: *mut ff::AVFilterGraph,
    src: *mut ff::AVFilterContext,
    sink: *mut ff::AVFilterContext,
    input: *mut ff::AVFrame,
    output: *mut ff::AVFrame,
    bytes: Vec<u8>,
}
// Single-owner graph; processing requires &mut self and is never concurrent.
unsafe impl Send for MicDenoiser {}
impl Default for MicDenoiser {
    fn default() -> Self {
        Self::new(Self::default_model_path())
    }
}
impl MicDenoiser {
    pub fn default_model_path() -> PathBuf {
        std::env::current_exe()
            .unwrap_or_default()
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("rnnoise/std.rnnn")
    }
    pub fn new(model: impl Into<PathBuf>) -> Self {
        Self {
            enabled: false,
            model: model.into(),
            failed: false,
            rate: 0,
            channels: 0,
            graph: null_mut(),
            src: null_mut(),
            sink: null_mut(),
            input: null_mut(),
            output: null_mut(),
            bytes: Vec::new(),
        }
    }
    /// None means use input unchanged; Some(empty) means drop this buffered startup chunk.
    pub fn process(&mut self, data: &[u8], rate: u32, channels: u16) -> Option<&[u8]> {
        if !self.enabled || self.failed || data.is_empty() || channels == 0 || rate == 0 {
            return None;
        }
        if self.input.is_null() || self.rate != rate || self.channels != channels {
            if !self.model.is_file() {
                log::warn!(
                    "[MicDenoiser] model missing: {} - noise suppression is OFF (the release must ship rnnoise/)",
                    self.model.display()
                );
                self.failed = true;
                return None;
            }
            if let Err(e) = self.setup(rate, channels) {
                log::warn!("[MicDenoiser] setup failed: {e}");
                self.failed = true;
                return None;
            }
        }
        let samples = data.len() / (channels as usize * 4);
        if samples == 0 {
            return None;
        }
        unsafe {
            ff::av_frame_unref(self.input);
            (*self.input).format = ff::AV_SAMPLE_FMT_FLT;
            (*self.input).sample_rate = rate as i32;
            (*self.input).nb_samples = samples as i32;
            ff::av_channel_layout_default(&mut (*self.input).ch_layout, channels as i32);
            if ff::av_frame_get_buffer(self.input, 0) < 0 {
                return None;
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), (*self.input).data[0], samples * channels as usize * 4);
            if ff::av_buffersrc_add_frame(self.src, self.input) < 0 {
                return None;
            }
            self.bytes.clear();
            loop {
                let r = ff::av_buffersink_get_frame(self.sink, self.output);
                if r < 0 {
                    break;
                }
                let n = (*self.output).nb_samples as usize * channels as usize * 4;
                self.bytes.extend_from_slice(std::slice::from_raw_parts((*self.output).data[0], n));
                ff::av_frame_unref(self.output);
            }
        }
        Some(&self.bytes)
    }
    fn setup(&mut self, rate: u32, channels: u16) -> Result<()> {
        self.teardown();
        if !self.model.is_file() {
            log::warn!("[MicDenoiser] RNNoise model not found — suppression disabled: {}", self.model.display());
            return Err(AudioError("RNNoise model missing".into()));
        }
        unsafe {
            self.graph = ff::avfilter_graph_alloc();
            if self.graph.is_null() {
                return Err(AudioError("filter graph allocation failed".into()));
            }
            let mut layout = std::mem::zeroed();
            let mut desc = [0i8; 128];
            ff::av_channel_layout_default(&mut layout, channels as i32);
            ff::av_channel_layout_describe(&layout, desc.as_mut_ptr(), desc.len());
            ff::av_channel_layout_uninit(&mut layout);
            let layout = CStr::from_ptr(desc.as_ptr()).to_string_lossy();
            self.src = self.create(
                "abuffer",
                "in",
                Some(format!("time_base=1/{rate}:sample_rate={rate}:sample_fmt=flt:channel_layout={layout}")),
            )?;
            let filter = ff::avfilter_get_by_name(c"arnndn".as_ptr());
            if filter.is_null() {
                log::warn!("[MicDenoiser] filter 'arnndn' missing.");
                return Err(AudioError("arnndn missing".into()));
            }
            let denoise = ff::avfilter_graph_alloc_filter(self.graph, filter, c"denoise".as_ptr());
            if denoise.is_null() {
                log::warn!("[MicDenoiser] alloc 'arnndn' failed.");
                return Err(AudioError("arnndn allocation failed".into()));
            }
            let model = CString::new(self.model.to_string_lossy().as_bytes()).map_err(|e| AudioError(e.to_string()))?;
            let r =
                ff::av_opt_set(denoise.cast(), c"model".as_ptr(), model.as_ptr(), ff::AV_OPT_SEARCH_CHILDREN as i32);
            if r < 0 {
                log::warn!("[MicDenoiser] set arnndn model: {}", lag_ffi::av_err_string(r));
            }
            check(r, "arnndn model")?;
            let r = ff::avfilter_init_str(denoise, null());
            if r < 0 {
                log::warn!("[MicDenoiser] init arnndn: {}", lag_ffi::av_err_string(r));
            }
            check(r, "arnndn init")?;
            let fmt = self.create(
                "aformat",
                "fmt",
                Some(format!("sample_fmts=flt:sample_rates={rate}:channel_layouts={layout}")),
            )?;
            self.sink = self.create("abuffersink", "out", None)?;
            check(ff::avfilter_link(self.src, 0, denoise, 0), "filter link")?;
            check(ff::avfilter_link(denoise, 0, fmt, 0), "filter link")?;
            check(ff::avfilter_link(fmt, 0, self.sink, 0), "filter link")?;
            check(ff::avfilter_graph_config(self.graph, null_mut()), "graph config")?;
            self.input = ff::av_frame_alloc();
            self.output = ff::av_frame_alloc();
            if self.input.is_null() || self.output.is_null() {
                return Err(AudioError("filter frame allocation failed".into()));
            }
        }
        self.rate = rate;
        self.channels = channels;
        log::info!("[MicDenoiser] arnndn (RNNoise) graph ready ({rate}Hz {channels}ch).");
        Ok(())
    }
    unsafe fn create(&self, name: &str, instance: &str, args: Option<String>) -> Result<*mut ff::AVFilterContext> {
        let cname = CString::new(name).unwrap();
        let instance = CString::new(instance).unwrap();
        let args = args.map(|s| CString::new(s).unwrap());
        let filter = ff::avfilter_get_by_name(cname.as_ptr());
        if filter.is_null() {
            log::warn!("[MicDenoiser] filter '{name}' missing.");
            return Err(AudioError(format!("{name} missing")));
        }
        let mut ctx = null_mut();
        let r = ff::avfilter_graph_create_filter(
            &mut ctx,
            filter,
            instance.as_ptr(),
            args.as_ref().map_or(null(), |s| s.as_ptr()),
            null_mut(),
            self.graph,
        );
        if r < 0 {
            log::warn!("[MicDenoiser] create '{name}': {}", lag_ffi::av_err_string(r));
        }
        check(r, "create filter")?;
        Ok(ctx)
    }
    pub fn teardown(&mut self) {
        unsafe {
            ff::av_frame_free(&mut self.input);
            ff::av_frame_free(&mut self.output);
            ff::avfilter_graph_free(&mut self.graph);
            self.src = null_mut();
            self.sink = null_mut();
        }
    }
}
impl Drop for MicDenoiser {
    fn drop(&mut self) {
        self.teardown();
    }
}
