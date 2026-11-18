# lag-audio

Headless Rust port of the C# capture engine's audio subsystem. Uses raw WASAPI/COM
through `windows`, FFmpeg 7.1 through `lag_ffi::ffmpeg`, and the `log` facade.
All timestamps and packet DTS/PTS are **microseconds on the capture session clock**.
No GUI, child audio recorder, NAudio, or additional native audio DLL is used at runtime.

## Engine integration

The engine can use `tracks::AudioSession` for the complete capture/encode/pump lifecycle,
or wire `WasapiAudioSource`, `AudioTracks`, and the clock separately.

```rust,no_run
use lag_audio::{AudioControls, AudioSourceOptions, clock::EngineClock};
use lag_audio::tracks::AudioSession;
use std::sync::Arc;

// Create this at the engine session origin, and share this exact clock with video.
let clock = Arc::new(EngineClock::new());
let controls = Arc::new(AudioControls::default());
controls.set_mic_volume(1.0);
controls.set_mic_muted(false);
controls.set_input_gate(true, 0.08);
controls.set_noise_suppression(false);

let mut audio = AudioSession::start(
    AudioSourceOptions::default(), controls, true, clock.clone(),
    |p| {
        // p.track_index is stable for the session. Add p.packet to that track's
        // engine-owned rolling ring; do not acquire the video capture lock here.
    },
)?;
let specs = audio.tracks().specs();
clock.set_paused(true);  // also freezes the VIDEO timeline; incoming PCM is dropped
clock.set_paused(false);
audio.pump();            // immediately before taking save/ring snapshots
audio.finish()?;         // stop timer + capture workers, then publish delayed AAC packets
# Ok::<(), lag_audio::AudioError>(())
```

If video already records `_startQpc`, use `EngineClock::from_qpc_origin(start_qpc)`.
`clock::qpc()`, `qpc_frequency()`, and `engine_now_us(&clock)` are public for
lag-capture. The epoch is session-relative, not absolute QPC time or a process-global
origin. Pause state is read as one consistent snapshot. Do not maintain a second
independent pause accumulator in video.

Match `VfrReplayEngine.StartAudio` at the engine boundary:

* When audio is disabled, skip session creation and log
  `[VfrReplayEngine] audio disabled (video-only).`.
* Set `AudioSourceOptions.apps_mode` to `is_game_target` when AudioModeByTarget is
  enabled, otherwise to the manual AppsAudioMode setting. Set `game_pid` only for
  a locked game; use zero on desktop. Source options use linear gains, not percentages.
* Treat `AudioSession::start` failure as nonfatal video-only operation and log
  `[VfrReplayEngine] audio capture unavailable — video-only: {error}`.
* Each `TrackPacket` contains the shared `lag_ffi::media::EncodedPacket` with owned,
  reference-counted bytes. `specs()` supplies shared `StreamSpec` values with AAC ASC
  extradata. Packet callbacks run after encoder/track locks have been released, under
  a separate audio-only publication lock that preserves packet ordering. A sink may
  read specs but must not recursively call `pump()`/`finish()` or encode audio.
* Snapshot using the video's absolute base; omit uncreated or empty microphone tracks.
  Preserve track order. The engine owns ring retention, rebasing, muxing, and carry-over.
* Do not hold the engine/video lock while stopping audio or executing a packet sink.
  `finish()` joins audio workers before flushing; `Drop` stops without flushing, like
  C# Dispose. Save does **not** flush (which would end the codec session).

The 100 ms silence timer operates independently of video frames. It uses the same
frozen clock during pause, and `pump()` advances the save tail. Separate mode creates
`All` (index 0, 192 kbps), `System` (index 1, 160 kbps), and an immediately visible
`Microphone` placeholder (index 2). Its stream spec remains `None` until the first
delivered mic buffer. Single mode creates `Audio` at 160 kbps. All AAC output is
48 kHz stereo; the All bitrate intentionally exceeds the source tracks so Windows
media playback selects the complete mix.

## Lower-level API and source behavior

`WasapiAudioSource::start(options, system_callback, mic_callback)` starts raw PCM
delivery and exposes `game_format()` and an `Arc<AudioControls>`. With
`start_with_controls`, configure atomics before starting. Callback slices are borrowed
and reusable: copy only if retaining them. Every capture stream has its own MTA
thread. Buffers are copied into preallocated storage and released back to WASAPI
**before** DSP/encoding or subscriber code runs. Audio workers never acquire any
video lock. The engine's packet sink should only append to a ring or nonblocking queue.
Small audio-only locks serialize encoder state and app queues, as in C#.

`AudioSourceOptions` mirrors `WasapiAudioSource.Start`:

| Field | Meaning/default |
|---|---|
| `microphone_id` | `None`: default Capture/Communications; `Some("")`: off; other ID: exact endpoint, no fallback |
| `system_device_id` | Default Render/Multimedia for absent/empty; invalid selection logs and falls back |
| `system_enabled`, `system_volume` | Whole-system loopback enabled, linear gain 1.0 |
| `apps_mode`, `apps` | Manual selected-app mode off; `(exe, linear_gain)` list empty |
| `game_pid`, `game_enabled`, `game_volume` | PID 0, enabled, gain 1.0; only used in effective apps mode |
| `mic_mono` | False; stereo float is averaged to mono after gate/denoiser |

Effective apps mode requires a selected app or an enabled nonzero game PID. Capture
the game first; selected executables prefer a visible main-window process, otherwise
the first process. Deduplicate successful captures by PID. There is no live retry or
process re-enumeration. If no app capture succeeds, fall back to full-system loopback
even if `system_enabled` is false (intentional C# parity). Each process uses fixed
48 kHz/stereo/float capture. App queues discard an incoming chunk when the existing
backlog exceeds 96,000 floats; a 20 ms mixer dequeues 1,920 floats per app, sums,
clamps, and emits even when empty. System gain does not affect that app sum.

Mic/system gains only clamp negative configuration to zero; there is no gain upper
bound. Gains clamp float samples to [-1,1]. App gains are stored unchanged. Muting
drops mic buffers before gate or denoiser state advances. Float mic processing is:
raw peak → perceptual peak^0.6 gate decision; volume → gate ramp → optional RNNoise
→ optional mono → callback. Ramp position is interleaved sample index / sample count,
including the C# last-sample endpoint convention. Integer PCM bypasses mic DSP.

`ProcessLoopbackCapture::start(pid, IncludeTree | ExcludeTree, callback)` uses
`ActivateAudioInterfaceAsync`, `AUDIOCLIENT_ACTIVATION_PARAMS`, and the virtual
`VAD\Process_Loopback` device. Activation waits five seconds, Start waits six;
shared-mode initialization uses loopback + event flags and a 200 ms buffer. SILENT
packets become zeroes. All COM operations and cleanup stay on the owning MTA.
Unsupported Windows builds fail through the API rather than a guessed OS version gate.
The C# comment claiming build 19041 support is inaccurate; see Microsoft's
[activation documentation](https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-activateaudiointerfaceasync).

`AacAudioEncoder::new(PcmFormat, bitrate_kbps)` exposes `encode(bytes, engine_us)`,
`encode_mic(bytes, mic_format)`, `pump_silence_to(engine_us)`, `get_stream_spec()`, and
`flush()`. Encoding/pumping/flushing return owned packet vectors; there are no callbacks
under its native-state mutex. `encode_mic` queues into the mix and returns no packets.
Use `AudioTracks` for the corresponding indexed multi-track methods if the engine
owns the source and pump itself; publish the returned vectors in call order if using
multiple audio threads. Input data must match the encoder's construction
format; source formats are fixed for a session.

AAC timeline quirks are preserved: sample origin zero, silence padding only for a
positive gap of at most five seconds (after integer sample conversion), no reanchor
for larger gaps, FIFO arrival-order mic mixing, one-second normal mic backlog with
oldest-drop, hard-clipped sums, complete 1024-sample frames, and negative codec-priming
PTS. All packets have DTS=PTS and keyframe=true. Flush only submits EOF: it does not
pad a partial FIFO or drain resampler delay. It is idempotent and final.

`NoiseGate` ports the automatic floor, threshold clamps, 280 ms hold, 70/s attack,
7/s release, dt clamp, and Reset behavior exactly. `MicDenoiser::process` returns
`None` for pass-through, `Some(&[])` for startup buffering, or a borrowed processed
slice. Its default model is `<current_exe directory>/rnnoise/std.rnnn`; `new(path)`
allows fixtures or a different installation directory. The graph is
abuffer → arnndn → aformat → abuffersink. Set the model using `av_opt_set`, preserving
Windows drive-letter colons. Failure latches pass-through for the instance lifetime.

## Settings and diagnostics

* `devices::enumerate_devices()` returns active render/capture IDs and friendly names.
* `devices::AudioDeviceWatcher::new(callback)` reports state/add/remove/default changes
  on system threads and ignores property changes. Marshal/debounce in the UI. It
  owns a dedicated registration MTA and unregisters before releasing the enumerator.
* `sessions::get_active_audio_apps()` returns sorted, case-insensitively deduplicated
  active executable sessions on the default multimedia render device, excluding self
  and PID 0. Limited-rights image paths, version-resource descriptions, and native
  associated icons encoded as PNG are included where available; lookup errors are
  nonfatal. Window enumeration is read-only and does not open or activate windows.
* `monitor::AudioMonitor` has `start(mic_id, output_id)`, `stop()`, reset-on-read raw
  peaks, `gate_threshold()`, `gate_open()`, gate/suppression setters and
  `set_monitoring(on, output_id)`. Invalid device IDs fall back to defaults here.
  Optional hear-yourself uses a 500 ms bounded queue, zero-fill on underrun,
  100 ms shared push rendering and a dedicated render worker. Stop capture before
  freeing microphone DSP. Peak exchange is atomic, so concurrent meter reads cannot
  erase a newly delivered callback peak.
* `ProcessLoopbackCapture::self_test(pid)` ports the five-second callback/bytes/peak
  diagnostic, including REAL AUDIO only for peak >0.0001.
* `playback_probe::probe(path)` returns Windows audio-track properties/default index;
  `run_from_env()` ports semicolon-separated `LAG_MFPROBE` output. Neither creates a
  player or window. `examples/audio_diagnostics.rs` installs a console log backend
  and dispatches nonempty `LAG_MFPROBE` before signed-integer `LAG_PROCTEST`, matching
  Program.cs. The application entry point can call these same functions directly.

## Intentional fixes and implementation differences

* `ProcessLoopbackCapture.cs:68` ignores the six-second readiness wait result and can
  report success without setup. Rust returns an error and cancels/joins the worker.
* `ProcessLoopbackCapture.cs:208–211` closes the event even if a timed join expires.
  Rust joins fully and closes the event on its owner thread, preventing use-after-close.
* `AacAudioEncoder.cs:51–79` leaks native allocations if construction fails midway.
  Rust RAII releases partial construction, including resamplers and channel layouts.
* `AudioMonitor.cs:59,62` races peak read/reset against callback updates. Atomic
  fetch-max/swap prevents lost meter peaks without changing the scale or smoothing.
* `AacAudioEncoder.cs:201` initializes mic resampling only once. The Rust low-level
  `encode_mic` rebuilds it if input format changes; ordinary fixed-format sessions
  behave identically.
* Unsupported packed 24-bit PCM is rejected rather than interpreted as float as in
  `VfrReplayEngine.cs:807–811`. Float WAVEFORMATEXTENSIBLE is recognized explicitly.
  This avoids feeding invalid/misaligned data to FFmpeg.
* Settings preview uses the existing FFmpeg resampler and WASAPI shared automatic
  conversion instead of NAudio's managed WDL resampler. This avoids a new native/C++
  dependency; queue/latency, gate, denoiser, unity mono duplication and silence behavior
  are retained. Resampling filter response can differ. Audible real-mic preview has
  not been exercised by the automated tests.
* Reusable WASAPI/mixer/resampler storage removes per-buffer allocations where possible.
  Callback ownership and worker joins replace C# event unsubscription and Dispose locks.
  Native errors return Rust `Result`; missing-mic/source fallbacks retain C# behavior.

## Verification

From the workspace, with `stable-x86_64-pc-windows-gnu`:

```powershell
$env:PATH = 'E:\project\InstantReplay\InstantReplay\ffmpeg;E:\project\InstantReplay\lag-rs\vendor\mpv;' + $env:PATH
cargo test -p lag-audio
cargo clippy -p lag-audio --all-targets -- -D warnings
cargo fmt -p lag-audio -- --check
```

`tools/verify.ps1` runs those checks. If GNU dlltool cannot invoke its assembler in
the managed environment, `tools/verify.ps1 -UseLld` builds a tiny import-library
adapter with the installed GNU Rust toolchain's own LLD. It changes only the current
process environment and `lag-audio/target`; no system/toolchain/workspace config is
modified. No new dependency or MSVC toolchain is required.

The default suite includes gate arithmetic, decoded AAC sine/gap silence and timing,
mic during system silence, five-second padding bounds, lazy track creation, QPC pause,
one second of stereo noise through the real repository `std.rnnn`, active device
enumeration/default render loopback for 200 ms, own-process tree activation for 200 ms,
and a generated three-track MP4 tested against Windows' default track selection.
Fixture FFmpeg runs with CREATE_NO_WINDOW and generated files stay under this crate's
ignored target directory. Real user audio is discarded immediately and never saved.
The five-second selected-process diagnostic and arbitrary existing-clip probe are
explicit ignored tests, requiring `LAG_PROCTEST` / `LAG_MFPROBE` respectively.

## Source map

| C# source | Rust module/files |
|---|---|
| `VfrCapture/WasapiAudioSource.cs` | `source.rs`, `pcm.rs`, endpoint capture in `wasapi.rs` |
| `VfrCapture/ProcessLoopbackCapture.cs` | `wasapi.rs`, `examples/audio_diagnostics.rs` |
| `VfrCapture/AacAudioEncoder.cs` | `aac.rs` |
| `VfrCapture/NoiseGate.cs` | `noise_gate.rs` |
| `VfrCapture/MicDenoiser.cs` | `denoiser.rs` |
| `AudioSessionService.cs` | `sessions.rs` |
| `AudioDeviceWatcher.cs` | `devices.rs` |
| `AudioMonitor.cs` | `monitor.rs` |
| `VfrCapture/PlaybackTrackProbe.cs` | `playback_probe.rs`, diagnostics example |
| `VfrCapture/VfrReplayEngine.cs` audio/clock integration | `tracks.rs`, `clock.rs` |

`src/lib.rs` supplies exports/errors; `tests/audio.rs` supplies integration/native
checks; `Cargo.toml`, `.gitignore`, and `tools/*` supply package/build support.
Handwritten edits are confined to `crates/lag-audio/**`. Cargo also refreshed the
shared workspace lockfile during dependency resolution; concurrent workspace changes
were left intact. No commits were made.
