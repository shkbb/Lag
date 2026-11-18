//! Lag's native replay engine: WGC capture -> D3D11 BGRA->NV12 -> hardware encoder -> rolling packet
//! buffer -> MP4. Port of `InstantReplay/Services/VfrCapture/` (see docs/spec/engine.md).

pub mod cadence;
pub mod carry;
pub mod convert;
pub mod d3d;
pub mod detect;
pub mod encoder;
pub mod engine;
mod engine_impl;
pub mod mux;
pub mod overlay;
pub mod perf_guard;
pub mod ring;
pub mod stall;
pub mod wgc;
