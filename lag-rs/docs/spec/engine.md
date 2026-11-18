# Native capture/replay engine — 1:1 port specification

## 1. Scope

This is an acceptance specification of the C# implementation, read without building or running it. Source paths below are relative to E:\project\InstantReplay\InstantReplay. Unless a path is qualified, a VFR filename denotes Services/VfrCapture/<filename>; the six shared service filenames and five legacy filenames denote Services/<filename>. Line references describe the inspected source, not an inferred intended implementation. All arithmetic, fallback rules, omissions and quirks below are normative for parity unless explicitly described as a proposed fix.

Every listed file was read in full:

| Services/VfrCapture file | Lines | Classification |
| --- | ---: | --- |
| AacAudioEncoder.cs | 361 | Live audio encoder/mixer |
| Bgra2Nv12Converter.cs | 291 | Live GPU conversion and optional readback |
| CadenceGrid.cs | 99 | Live timestamp regularizer/cap |
| CadenceSelfTest.cs | 116 | Headless diagnostic |
| CaptureStallMonitor.cs | 186 | Live diagnostic |
| CarryOverSegment.cs | 109 | Live previous-session tail |
| CarryOverSelfTest.cs | 149 | Headless diagnostic |
| D3D11Context.cs | 72 | Live shared GPU device |
| DuplicationProbe.cs | 99 | Headless DXGI diagnostic; not the recording source |
| DxgiDuplicationCaptureSource.cs | 150 | Unused capture implementation, referenced only by unused HybridGameCaptureSource |
| EncoderSelector.cs | 185 | Live encoder discovery/selection |
| FfmpegInterop.cs | 104 | Live FFmpeg binding |
| GameDetector.cs | 278 | Live capture and audio-picker classification |
| GcSelfTest.cs | 158 | Headless diagnostic |
| GpuUsageProbe.cs | 125 | Live detection signal |
| HybridGameCaptureSource.cs | 138 | Unused capture implementation; no constructor call site |
| KeystrokeTracker.cs | 150 | Live overlay input state |
| MicDenoiser.cs | 211 | Live recorder and settings-monitor helper |
| Mp4Muxer.cs | 171 | Live packet muxing |
| NoiseGate.cs | 77 | Live recorder and settings-monitor helper |
| NvencVfrEncoder.cs | 447 | Live universal video encoder, despite its name |
| OverlayCompositor.cs | 433 | Live webcam/keys compositor; baked stats available only to direct/test clients |
| PacketRingBuffer.cs | 186 | Live compressed in-memory rolling buffer |
| PlaybackTrackProbe.cs | 48 | Headless Windows media diagnostic |
| ProcessLoopbackCapture.cs | 312 | Live per-process WASAPI and self-test |
| ReplayStitcher.cs | 196 | Live save-time join |
| StallMonitorSelfTest.cs | 106 | Headless diagnostic |
| SystemStatsSampler.cs | 158 | Live shared stats helper |
| VfrEngineTest.cs | 131 | Headless end-to-end diagnostic |
| VfrRecorderAdapter.cs | 121 | Live IReplayRecorder implementation |
| VfrReplayEngine.cs | 1178 | Live orchestrator |
| WasapiAudioSource.cs | 380 | Live audio capture/mixer |
| WebcamCaptureSource.cs | 168 | Live webcam and device enumeration |
| WgcCaptureSource.cs | 281 | Live window/monitor source |
| WgcTimestampProbe.cs | 174 | Headless timestamp diagnostic |

| Services file | Lines | Classification |
| --- | ---: | --- |
| IReplayRecorder.cs | 66 | Live public contract and CaptureTarget |
| RecorderOptions.cs | 114 | Live session snapshot and AppAudioCapture |
| PerformanceGuard.cs | 86 | Live startup scheduling tweaks |
| AudioSessionService.cs | 124 | Live active-app picker enumeration |
| AudioDeviceWatcher.cs | 41 | Live endpoint notifications |
| ScreenshotService.cs | 96 | Live GDI PNG capture |
| RecordingEngine.cs | 306 | Dead legacy orchestration |
| WindowsCaptureService.cs | 233 | Dead legacy GPU-to-CPU screen source |
| RingBufferManager.cs | 159 | Dead legacy disk-segment ring |
| IScreenCaptureService.cs | 78 | Dead legacy contract, MonitorInfo and CapturedFrame |
| FFmpegService.cs | 381 | Dead legacy process helper |

Reference audit: searched handwritten C# and project files recursively, excluding generated/deployed binaries and package output. The five legacy types have only their definitions and references to each other. App.axaml.cs:346–366 registers HardwareDetector, VfrRecorderAdapter and an IReplayRecorder alias to that adapter, with no legacy registration. MainViewModel.cs:170–180 consumes IReplayRecorder. VfrReplayEngine.cs:366–388 constructs WgcCaptureSource for both windows and monitors; no call site constructs HybridGameCaptureSource. Its only capture-source dependency is DxgiDuplicationCaptureSource (HybridGameCaptureSource.cs:31–58). The independent DuplicationProbe remains reachable through Program.cs:95–98. These are source reachability findings, not a claim that dead files are excluded from compilation: InstantReplay.csproj:1–105 has no explicit removal of the legacy files.

Additional call-site evidence was inspected in App.axaml.cs, Program.cs, InstantReplay.csproj, Services/GlobalHotkeyManager.cs, Services/FileLog.cs, ViewModels/MainViewModel.cs, ViewModels/SettingsViewModel.cs and ViewModels/SettingsViewModel.AudioMeter.cs. Their UI/persistence implementations remain cross-area concerns except where they establish engine behavior below.

## 2. Features

1. Arm an in-memory compressed replay buffer, automatically capture a detected game or fall back to the selected desktop monitor; hold the game through alt-tab until its HWND or process dies. Explicit display and forced-window modes also exist for direct engine clients. VfrReplayEngine.cs:303–388.
2. Save genuine variable-timestamp video with no duplicated CFR padding; regularize arrival timestamps onto an exact rational cadence grid and drop collisions to enforce the selected FPS cap. VfrReplayEngine.cs:484–545; CadenceGrid.cs:72–97.
3. Choose available H.264/HEVC/AV1 hardware encoders by actual open probes, with CPU x264 fallback, and support fixed bitrate or automatic constant quality. EncoderSelector.cs:27–38,71–120; NvencVfrEncoder.cs:144–250.
4. Preserve source aspect during optional downscale, convert full-range BGRA to limited-range BT.709 NV12, and keep vendor-matched NVENC/AMF frames on the GPU. Cross-vendor, QSV and CPU paths download NV12. VfrReplayEngine.cs:391–396; Bgra2Nv12Converter.cs:95–118,209–273; NvencVfrEncoder.cs:82–111.
5. Capture whole-PC audio on the selected render endpoint, or sum selected application process trees and detected-game audio, with per-source gain and one capture per PID. WasapiAudioSource.cs:90–210.
6. Capture an optional default/specific microphone, gain/mute it, apply automatic/manual noise gating, optional RNNoise suppression and optional stereo-to-mono downmix. WasapiAudioSource.cs:105–121,259–319.
7. Produce one mixed AAC track, or default All plus separate System and lazily created Microphone tracks; continuously pad silence so mic and audio survive silent desktop/system spans. VfrReplayEngine.cs:672–795; AacAudioEncoder.cs:138–183.
8. Pause without destroying WGC or codec sessions: discard incoming A/V and freeze the shared engine clock, excluding the pause from saved clips. Resume repins cadence on the capture thread. VfrReplayEngine.cs:658–665,986–1005.
9. Retain the last session's encoded tail when a game exits or a target switches, shrink it as the new session fills, save repeatedly without consuming it, and stitch it before the live footage. CarryOverSegment.cs:57–107; VfrReplayEngine.cs:875–936,1019–1066.
10. Save MP4, MOV or MKV; unknown format/AVI becomes MP4. Direct mux is synchronous on the save caller and has no faststart rewrite. VfrReplayEngine.cs:798–805,939–945; Mp4Muxer.cs:18–21,37–80.
11. Bake optional webcam picture-in-picture and a keyboard/mouse visualization into the recording. Baked resource stats are supported by the engine and tests but the normal app uses a separate live HUD and does not enable baked stats. OverlayCompositor.cs:128–163; MainViewModel.cs:529–540.
12. Enumerate active audio apps with friendly executable descriptions/icons and notify settings of endpoint additions/removals/default changes. AudioSessionService.cs:31–81; AudioDeviceWatcher.cs:22–39.
13. Capture a monitor PNG independently of recording via GDI; fall back to primary if the selected device is absent. ScreenshotService.cs:41–94.
14. Expose target changes, saved paths, carried game tags, cumulative frame counts and stage timing; attribute stalls and supply headless capture/audio/cadence/carry/GC/probe diagnostics. IReplayRecorder.cs:13–66; VfrReplayEngine.cs:187–224; Program.cs:70–135.

## 3. UI

There is no Avalonia screen/control tree in the scoped service files. Recorder options are cold-start snapshots; overlay geometry below is burned into recorded pixels, not an interactive engine-side UI. Settings/hotkey bindings, their visual states and localization belong to their own areas. The engine has no localized strings, hover/pressed controls, menus or tooltips. IReplayRecorder.cs:21–43; RecorderOptions.cs:5–9; OverlayCompositor.cs:38–46.

### Recorded overlay layout and styling

OverlayOptions defaults: WebcamEnabled/KeysEnabled/StatsEnabled false; webcam X=.97, Y=.95, Scale=.24; keys X=.03, Y=.95, Scale=.20; stats X=.5, Y=.02, Scale=.045, Detail=1. AnyEnabled is the OR of those three booleans. X/Y multiply free frame space, not frame coordinates; they are not clamped themselves. OverlayOptions duplicates corresponding engine/recorder defaults. OverlayCompositor.cs:15–35; VfrReplayEngine.cs:56–72; RecorderOptions.cs:88–110.

The draw order is source copy → webcam → keys → stats. Use a SingleThreaded Direct2D Factory1, device on the shared DXGI device, DeviceContextOptions.None and Shared DirectWrite factory. Compose polls webcam and increments stats before deciding whether anything is drawable; with nothing drawable return the original texture. Otherwise create/recreate a frame-sized B8G8R8A8_UNorm, Default, 1 mip, 1 slice, sample(1,0), RenderTarget|ShaderResource texture. Wrap its surface with premultiplied BGRA, 96×96 DPI, BitmapOptions.Target|CannotDraw. CopyResource, set D2D target, BeginDraw, draw in order, EndDraw, clear target, return composed texture. No clear replaces the copied background. OverlayCompositor.cs:91–100,128–163,183–204.

Brushes use exact float RGBA, not rounded CSS equivalents: idle (0.06,.065,.085,.62), active (.133,.827,.933,1), idle text (1,1,1,.85), active text (.04,.05,.06,1), outline (1,1,1,.28), webcam border (1,1,1,.35). The active brush comment calls it brand cyan #22D3EE; preserve the actual floats. OverlayCompositor.cs:206–211.

Webcam: unit=frameH/1080f; boxH=WebcamScale*frameH; boxW=boxH*camW/max(1,camH); x=WebcamX*max(0,frameW-boxW); y=WebcamY*max(0,frameH-boxH). Rounded radius=14*unit. Clip using RoundedRectangleGeometry in a layer with ContentBounds=dest, identity transform, opacity=1, LayerOptions.None; draw bitmap with opacity 1 and Linear interpolation; pop; draw rounded white-alpha border 2*unit. Camera bitmap is premultiplied BGRA8 at 96 DPI, BitmapOptions.None, recreated on camera dimensions and updated only on a changed sequence via CopyFromMemory with stride w*4. OverlayCompositor.cs:166–180,214–238.

Keyboard rows, label IDs, displayed text and widths in key units (space text is empty):

| Row | Keys in order (ID/text/width where not equal to ID/1) |
| --- | --- |
| 1 | Esc, 1, 2, 3, 4, 5 |
| 2 | Tab/Tab/1.5, Q, W, E, R, T |
| 3 | Caps/Caps/1.75, A, S, D, F, G |
| 4 | Shift/Shift/2.25, Z, X, C, V, B |
| 5 | Ctrl/Ctrl/1.25, Alt/Alt/1.25, Space/(empty)/3.5 |

These are the complete drawn keys, even though the tracker recognizes more. OverlayCompositor.cs:82–89.

Input geometry: unit=frameH/1080f; keyU=(KeysScale*frameH)/5.47f; gap=keyU/8.5f; radius=.15f*keyU; fontPx=max(6,.38f*keyU). Recreate Segoe UI SemiBold Normal text format if absent or |previousFont-fontPx|>.5; horizontal/paragraph alignment Center. kbW=max over rows of sum(width*keyU+gap)-gap; kbH=5*(keyU+gap)-gap. MouseW=1.82*keyU, MouseH=2.82*keyU, mouseGap=.41*keyU; blockW=kbW+mouseGap+mouseW; blockH=max(kbH,mouseH). bx=KeysX*max(0,frameW-blockW), by=KeysY*max(0,frameH-blockH); vertically center keyboard and mouse within blockH. Each row begins at bx; key rectangle width=w*keyU and height=keyU; x advances w*keyU+gap, row y advances keyU+gap. OverlayCompositor.cs:242–296.

Each key first fills idle, then active fill with opacity from the tracker when >0, then outline stroke 1*unit. Draw nonempty label using active dark text only when opacity>.5; otherwise use idle text, DrawTextOptions.None and MeasuringMode.Natural. Mouse body radius=.42*mouseW; button split y=my+.42*mouseH. Clip rounded body, fill idle, fill left/right top halves separately for LMB/RMB opacities, draw vertical middle line and horizontal split with outline 1*unit, pop. Wheel/MMB width=9*unit, height=24*unit, starts my+12*unit and is centered horizontally; radius=wheelW/2; idle/active fills and outline 1*unit. Body final outline=1.5*unit. Restore active brush opacity=1 after all input drawing. OverlayCompositor.cs:297–366.

Stats text by Detail: 0 → "{Fps} FPS"; 1 → "{Fps} FPS   CPU {CpuPercent}%   GPU {GpuPercent}%"; every other value → "{Fps} FPS   CPU {CpuPercent}%   GPU {GpuPercent}%   RAM {RamUsedGb:0.0}/{RamTotalGb:0.0} GB", invariant culture for RAM. panelH=StatsScale*frameH; fontPx=max(6,panelH*.52); Segoe UI SemiBold Normal; text layout max width=frameW, height=panelH, recreated only if absent, text changes or font differs by >.5. padX=.45*panelH, panelW=layout.Metrics.Width+2*padX; x/y=StatsX/Y*max(0,frameDimension-panelDimension); radius=.22*panelH; idle fill, outline stroke exactly 1 (not scaled by unit), idle text at x+padX and vertically centered using layout metrics. OverlayCompositor.cs:369–406.

Only recorded keystroke release is animated: linear opacity fade for 450 ms; held opacity is 1. No overlay placement animation is implemented. KeystrokeTracker.cs:19–22,81–105. Overlay setup failure records plain video; per-frame Compose catches errors, logs the first error only, and returns original source, continuing to attempt composition on later frames. OverlayCompositor.cs:102–114,155–162; VfrReplayEngine.cs:258–283.

Cross-area UI uses CurrentTarget=null for idle, CaptureTarget(false,"",null) for desktop and game friendly name/exe for a game; the caller localizes desktop naming. Screenshot caller captures on Task.Run and uses Toast_ScreenshotSaved; recording status keys used by the caller include Status_Starting, Status_NoMonitor, Status_Recording, Status_RecordError, Status_Stopping, Status_Stopped, Status_Paused, Status_Saving, Status_NotRecording, Status_SaveError, Status_ReplaySaved and Toast_ReplaySaved/Toast_RecordingStarted/Toast_RecordingStopped. These keys are not resolved inside the engine. IReplayRecorder.cs:45–66; MainViewModel.cs:196–227,414–428,471–588,615–690.

## 4. State & logic

### Contracts, configuration and mapping

IReplayRecorder is IDisposable; required members are IsRecording, ReplaySaved(sender,absolutePath), Initialize(options), StartBuffer(), SaveReplay(), Teardown(), SetMicMuted(bool). Default interface implementations: IsPaused=false, SetPaused no-op, CurrentTarget=null, LastSavedGameTag=null, CaptureTargetChanged add/remove no-op. Native adapter overrides all. CaptureTarget is readonly record struct(bool IsGame,string Name,string? Exe). AppAudioCapture is sealed record(string ExeName,float Volume). IReplayRecorder.cs:13–66; RecorderOptions.cs:114.

RecorderOptions fields/defaults (unset nullable references=null; unset numeric/bool=0/false):

| Group | Exact properties/defaults |
| --- | --- |
| Video | BufferSeconds:int=60; FrameRate:int=30; Width:uint=0; Height:uint=0; OutputWidth:uint=0; OutputHeight:uint=0; VideoBitrateKbps:int=20000; AdapterIndex:int=0; PreferredEncoder:string?; MonitorId:string?; LibraryPath:string?; FileFormat:string="mp4" |
| Mic | MicrophoneId:string?; MicVolume:float=1; MicForceMono:bool=false; InputGateAuto:bool=true; InputGateThreshold:float=.08; NoiseSuppression:bool=false; MicStartMuted:bool=false |
| System/apps | AudioCaptureMode:string="all"; AudioModeByTarget:bool=false; SystemAudioDeviceId:string?; AudioApps:IReadOnlyList<AppAudioCapture>=[]; SeparateAudioTracks:bool=false; SystemAudioEnabled:bool=true; SystemAudioVolume:int=100; GameAudioEnabled:bool=true; GameAudioVolume:int=100 |
| Webcam | WebcamOverlay=false; WebcamDeviceId:string?; WebcamX:double=.97; WebcamY:double=.95; WebcamScale:double=.24 |
| Keys | KeysOverlay=false; KeysX:double=.03; KeysY:double=.95; KeysScale:double=.20 |
| Stats | StatsOverlay=false; StatsX:double=.5; StatsY:double=.02; StatsScale:double=.045; StatsDetail:int=1 |

All are init-only. Native capture dimensions are described as never cropped; only OutputHeight actually controls the native engine. RecorderOptions.cs:10–110; VfrRecorderAdapter.cs:60–105.

VfrEngineOptions differs from the UI snapshot defaults: LibraryPath="", BufferSeconds:double=120, PreferredCodec=null, BitrateKbps=30000, FileFormat="mp4", TargetHeight=0, MaxFps=120, Preset=4, CaptureCursor=true, MonitorDeviceId=null, Mode=CaptureMode.AutoGame (enum order AutoGame,Display), AdapterIndex=-1; MicrophoneId=null, MicVolume=1, MicStartMuted=false, SeparateAudioTracks=false, AppsAudioMode=false, AudioModeByTarget=false, AudioApps=Array.Empty<(string Exe,float Volume)>(), SystemAudioEnabled=true, SystemAudioDeviceId=null, SystemVolume=1, MicMono=false, InputGateAuto=true, InputGateThreshold=.08, NoiseSuppression=false, GameAudioEnabled=true, GameVolume=1, AudioEnabled=true, ForcedWindow=IntPtr.Zero, DuplicationOnly=false. Overlay defaults match section 3. VfrReplayEngine.cs:14–73.

Adapter owns one engine from construction, forwards ReplaySaved with adapter as sender, forwards TargetChanged as CaptureTargetChanged with EventArgs.Empty. IsRecording maps to IsRunning; CurrentTarget requires TargetActive; FramesCaptured forwards cumulative counter; teardown=Stop, dispose=Dispose; mic/pause/save forward. Initialize only caches mapped options; StartBuffer does nothing before Initialize. FfmpegDir is GetFullPath(BaseDirectory/"ffmpeg"); IsAvailable calls engine availability and returns false on exceptions. VfrRecorderAdapter.cs:16–58.

IsRunning/adapter IsRecording means armed, even before a target is acquired or during a lock cooldown. FriendlyEncoder returns last selected FriendlyName or the em dash "—" when no choice; choice is not cleared by Stop. BufferedSeconds under _gate sums live ring.BufferedSeconds and carry.DurationSeconds with nulls as0; it is not wall time since arming. AvgStageMs returns(0,0) when encoder absent or TimedFrames==0; otherwise(ConvertUs/1000.0/TimedFrames,EncodeUs/1000.0/TimedFrames), without a snapshot lock across those counter reads. FramesCaptured/FramesEncoded are public long counters. TargetActive/TargetIsGame/TargetName/TargetExe have private setters, initial false/false/""/null. VfrReplayEngine.cs:195–224.

Map: null LibraryPath→""; copy buffer, bitrate, format (null/empty/whitespace→"mp4"), (int)OutputHeight→TargetHeight; positive FrameRate else 120→MaxFps; MonitorId→MonitorDeviceId; AdapterIndex copied; mic fields copied (MicForceMono→MicMono); audio mode equals "apps" ordinal-ignore-case (no trim); null AudioApps→empty list, mapped tuples; percentage SystemAudioVolume/GameAudioVolume divided by 100f without clamping; all overlays copied; CaptureCursor forced true. Width/Height/OutputWidth do not map. Mode/Preset/AudioEnabled/ForcedWindow/DuplicationOnly remain engine defaults. VfrRecorderAdapter.cs:60–105.

MapCodec lowercases invariantly but does not trim: empty/null→null; contains "av1"→"av1"; contains "hevc"/"h265"/"265"→"hevc"; contains "264"→"h264"; only then contains "x264"/"cpu"→"x264"; anything else→null. Thus an explicit "x264"/"libx264" string passed through this adapter actually becomes h264; "cpu" reaches force-CPU; direct engine preference x264 works. VfrRecorderAdapter.cs:111–119.

### Threads, synchronization and lifecycle

| Work | Actual execution context and guards |
| --- | --- |
| UI initialization/start/stop/save requests | Normal MainViewModel shifts Initialize+StartBuffer and save onto Task.Run; teardown runs its own Task.Run and next start awaits it. The service methods themselves are synchronous, with no private save thread. MainViewModel.cs:465–467,542–547,583–588,676–692. |
| Encoder discovery | Dedicated background thread named EncoderProbe, explicitly MTA where possible; caller starts and joins it. Result cached process-wide. EncoderSelector.cs:46–62. |
| Lock evaluation/session creation/audio start/periodic stats/carry aging | System.Threading.Timer, due 500 ms, period 2000 ms; SafeEvaluateLock catches each evaluation/trim/stats operation separately. It is a ThreadPool timer; there is no overall timer non-reentrancy guard. VfrReplayEngine.cs:285–301. |
| Capture/overlay/conversion/video submit/drain/one-time retune | Free-threaded WGC MTA worker callback, synchronously before pool texture/frame are released; no video worker queue. WgcCaptureSource.cs:26–28,132–162; VfrReplayEngine.cs:484–546. |
| Full-system/mic PCM | NAudio per-stream WASAPI callback threads; callbacks directly call AAC resampling/encoding. WasapiAudioSource.cs:16–17,249–308; VfrReplayEngine.cs:723–765. |
| Each per-process loopback | Dedicated background MTA named ProcLoopback{pid}; setup, capture, COM Stop/Release all on that thread. PCM callback only queues gain-applied app floats. ProcessLoopbackCapture.cs:63–95; WasapiAudioSource.cs:213–223. |
| Selected-app sum and AAC master feed | 20 ms System.Threading.Timer from WasapiAudioSource; mix emits GameDataReady synchronously on that callback. WasapiAudioSource.cs:194,228–246. |
| AAC silence advance | 100 ms System.Threading.Timer, also called once synchronously by save; individual encoders serialize it against PCM/teardown. VfrReplayEngine.cs:771–795; AacAudioEncoder.cs:30–40,138–148. |
| Webcam | WinRT MediaFrameReader callback normalizes/copies CPU frames under its gate; video callback polls/copies the latest sequence, uploads and composes. WebcamCaptureSource.cs:97–146; OverlayCompositor.cs:166–180. |
| Input | Existing GlobalHotkeyManager SharpHook hook feeds key/button state; capture thread snapshots under the tracker gate. No new hook or motion handler. GlobalHotkeyManager.cs:80–87,95–116,153–161; KeystrokeTracker.cs:57–105. |
| Stats sampling | System.Threading.Timer once per 1000 ms; atomically publishes immutable snapshot; capture thread only increments frame count/reads snapshot. SystemStatsSampler.cs:18–46,48–76. |
| Endpoint notifications | WASAPI system callback thread; subscriber posts to UI and debounces 300 ms. AudioDeviceWatcher.cs:8–12,28–34; SettingsViewModel.AudioMeter.cs:33–67. |
| Mux/transcode | Calling save thread; ring snapshots copied under individual gates; mux and any child ffmpeg run after those gates release. VfrReplayEngine.cs:817–937; Mp4Muxer.cs:18–21. |

Engine _gate guards session fields, audio-track list and carry handoff, not an entire capture or save. Video _opLock serializes Encode/Flush/GetStreamSpec/Dispose; shared D3D device enables driver's multithread protection. AAC always takes _opLock before finer _writeLock (master FIFO/timeline) or _micLock. Packet rings have independent locks; a snapshot makes new packet records sharing Data arrays, which this pipeline treats as immutable although byte[] is not intrinsically immutable. Stats logger alone uses Monitor.TryEnter(_statsGate), skips when already logging. VfrReplayEngine.cs:90–115,558–599,823–827; D3D11Context.cs:62–66; NvencVfrEncoder.cs:35–39,258–264,410–445; AacAudioEncoder.cs:30–40,85–91,117–124,188–225; PacketRingBuffer.cs:50–73,100–178.

Event invocations are synchronous on the thread executing their call site: normal TargetChanged on lock timer, but clearing target on Stop/SimulateTargetLoss on that caller; ReplaySaved on save thread; video PacketReady on capture thread or teardown Flush caller; AAC PacketReady on system/mic/app-mix/audio-pump/save-pump or teardown Flush caller as appropriate. Neither engine nor adapter creates a dispatcher handoff. The engine's _lastEncodedUs is assigned/reset but never read for additional gating; CadenceGrid is the actual monotonic/cap guard when enabled. VfrReplayEngine.cs:411,443,516,723–765,793,924–935,1011,1093,1103–1113; NvencVfrEncoder.cs:369,424–430; AacAudioEncoder.cs:317,337–342; VfrRecorderAdapter.cs:26–29.

Availability is WGC.IsSupported && FfmpegInterop.Initialize(dir) && EncoderSelector.Available.Count>0 (short-circuit). Start locks _gate, no-ops if disposed, overwrites _options, makes CadenceGrid(maxFps if >0 else 0), lazily creates/reuses D3D11 device with AdapterIndex, selects encoder or throws "No usable video encoder.", logs Armed, replaces overlay, marks running and creates lock timer only if absent. It has no running/idempotent-start guard and does not rebuild a retained D3D device for a later adapter selection. VfrReplayEngine.cs:231–287.

Lock state enum is None/Game/Desktop; initial cooldown DateTime.MinValue. Each evaluation:

1. If Game and both IsWindow(lockedHwnd) and IsProcessAlive(lockedPid), return immediately, irrespective of foreground, visibility, minimization, monitor movement or new candidates. If either fails, log window ended and teardown preserving carry.
2. If UtcNow<cooldownUntil return.
3. ForcedWindow nonzero bypasses mode/detector: only acquire with lock None, valid HWND, nonzero PID, successful process-name query; use ProcessName+".exe". It is held as Game.
4. Display mode ensures Desktop, tearing down a different lock if needed.
5. AutoGame asks FindBestGame(MonitorDeviceId). A candidate while Desktop tears down desktop; None then logs score/fullscreen and locks candidate. No candidate and None locks monitor.

No subscription connects WgcCaptureSource.Closed to teardown: current implementation relies on the 2 s poll. No startup capture occurs until timer evaluation. VfrReplayEngine.cs:303–388,407–411; WgcCaptureSource.cs:46–47,83.

Game dimensions come from GetWindowRect, not WGC ContentSize: fail/width<2/height<2 abort that lock silently. Monitor resolver EnumDisplayMonitors/GetMonitorInfoW uses primary if null OR empty, else case-insensitive device name; computes physical rcMonitor dimensions; no fallback if an explicit ID is missing. Enumeration continues but first found is retained; GC.KeepAlive(cb). Missing/invalid monitor logs and returns without cooldown. OutputDims: outH=TargetHeight if 0<TargetHeight<srcH, else srcH; outW=(int)Math.Round((double)srcW*outH/srcH), .NET midpoint-to-even; result=(outW & ~1,outH & ~1). Never upscale, crop or recompute encoder output after WGC resize. VfrReplayEngine.cs:366–396,453–481.

StartSession creates video encoder (fpsHint=positive MaxFps else 120), ring(BufferSeconds), wires encoder.PacketReady→ring.Add and capture.FrameReady→OnFrame; under _gate publishes encoder/ring/source/lock, resets lastEncodedUs, requests timeline reset, clears one-shot fps adaptation, records startQpc, replaces pause state with unpaused zero accumulator, sets target. Then capture.Start(cursor,maxFps), process High priority, SustainedLowLatency GC, StartAudio, locked log, TargetChanged. Failure logs "Failed to lock onto {label}: {message} — backing off 15s (no retry-thrash).", sets UTC cooldown +15s, teardown. TargetChanged can therefore briefly report/clear a failed session. VfrReplayEngine.cs:402–450.

Pause snapshot is one volatile reference containing AccumUs, PauseStartQpc, Paused; read once per engine-clock call. Let ts=Stopwatch.GetTimestamp(), f=Stopwatch.Frequency, p=captured pause:

    pausedUs = p.AccumUs + (p.Paused ? (ts-p.PauseStartQpc)*1_000_000L/f : 0)
    EngineNowUs = (ts-startQpc)*1_000_000L/f - pausedUs

Integer division truncates toward zero, including the separate divisions above. SetPaused holds _gate; no-op disposed/not running/same state. Pause stores current QPC with old AccumUs. Resume adds (resumeQpc-pauseStartQpc)*1e6/f to accumulated paused time, clears PauseStartQpc, requests capture-thread timeline reset. Both log "[VfrReplayEngine] paused/resumed (total paused {AccumUs/1000}ms)."; pause log excludes the still-current span. Audio pump is not explicitly disabled while paused; it sees the frozen clock. New session/teardown clears pause. VfrReplayEngine.cs:167–182,658–665,986–1005,1089.

OnFrame first captures encoder reference, returns if absent or paused; increments FramesCaptured atomically only for unpaused frames with an encoder. If reset pending, clear it, grid.Repin(), stalls.Reset(). nowUs=EngineNowUs; stalls.FrameArrived(nowUs,nowUs); ignore WGC sourceTicks entirely. Initialize stage durations zero; in try, grid.Snap(nowUs), return on Drop; set lastEncodedUs; synchronously compose and measure overlay; sample encoder ConvertUs/EncodeUs counters before/after Encode; increment FramesEncoded regardless of Encode's returned bool; maybe adapt fixed-bitrate FPS hint. finally always FrameDone, including cadence-dropped frames. Frame counters are fields initialized only with the engine object and are not reset on new sessions or Start/Stop. VfrReplayEngine.cs:133–161,207–224,484–546.

Stop sets running=false, disposes/nulls lock timer, TeardownCapture(keepCarry:false), then disposes/nulls overlay. Dispose is idempotent via disposed flag; sets it, calls Stop, disposes/nulls D3D device. TeardownCapture first disposes/nulls audio pump; under gate takes references and clears published source/encoder/ring/audio/tracks/lock/target/pause; emits TargetChanged only if previously active. Then begins timed teardown, detaches FrameReady, Flush video, read video StreamSpec while still valid if keeping carry, Dispose video BEFORE WGC pool, Dispose capture, Dispose WASAPI, Flush all AAC encoders, capture carry while AAC specs still readable (or clear carry on full stop), Dispose AAC, restore process Normal and GC Interactive. This ordering is essential to avoid freeing pool textures or codec contexts under in-flight conversion/PCM. Many audio/spec operations are individually caught; top-level video Flush/Dispose/capture/audio Dispose are not all wrapped by engine-level catch. Timer.Dispose itself does not join pending callbacks. VfrReplayEngine.cs:1071–1147.

### Shared D3D11 device and WGC

D3D11Context default adapterIndex=-1 uses DriverType.Hardware; index>=0 attempts Factory1.GetAdapter1(index) then new Device(adapter,flags), catches and logs unavailable→default hardware. Flags are exactly BgraSupport|VideoSupport; no Debug or SingleThreaded flag and no explicit feature-level list. Query DXGI Device.Adapter for name/vendor; QueryInterface<Multithread>().SetMultithreadProtected(true). Log "[D3D11Context] Device created on adapter: {AdapterName}". Dispose owns device only. Vendor IDs NVIDIA=0x10DE, AMD=0x1002, Intel=0x8086. D3D11Context.cs:27–71.

WgcCaptureSource owns one HWND or HMONITOR for life. Wrap shared IDXGIDevice using CreateDirect3D11DeviceFromDXGIDevice, reject nonzero hr or null pointer. GraphicsCaptureItem comes from WinRT class "Windows.Graphics.Capture.GraphicsCaptureItem", IGraphicsCaptureItemInterop IID 3628E81B-3CAC-4C60-B7F4-23CE0E0C3356, requested item IID 79C3F95B-31F7-4EC2-A464-632EF5D30760; vtable slot 3 CreateForWindow or 4 CreateForMonitor. Reject absent factory/failed item creation; release factory and ABI item pointer; source does not delete its HSTRING. WgcCaptureSource.cs:71–88,213–273.

Use Direct3D11CaptureFramePool.CreateFreeThreaded(winrtDevice,B8G8R8A8UIntNormalized,3,item.Size). Save dimensions, subscribe item.Closed (invoke Closed if not disposed), subscribe pool FrameArrived, create session, best-effort set IsCursorCaptureEnabled. Request GraphicsCaptureAccess.RequestAccessAsync(Borderless) synchronously first; catch/log unavailable; then independently best-effort set IsBorderRequired=false without API capability precheck. Log either "[WgcCaptureSource] disabled WinRT capture border." or "[WgcCaptureSource] capture-border toggle not available on this OS build.". Request result itself is ignored. WgcCaptureSource.cs:81–116.

Only if maxFps>0 AND Windows version>=10.0.26100: ms=1000.0/(maxFps*2.0), TrySet MinUpdateInterval=TimeSpan.FromMilliseconds(ms), log "[WgcCaptureSource] MinUpdateInterval set to {ms:F2}ms (floor {maxFps*2}fps)." even if the setter throws internally. This is a generous WGC delivery floor, distinct from exact output grid cap. StartCapture then log "[WgcCaptureSource] Capturing monitor/hwnd 0x{handle:X} at {w}x{h}.". WgcCaptureSource.cs:118–129.

Frame callback pulls only one TryGetNextFrame, no queue draining. If ContentSize changes (no positive-size validation), update lastSize and Recreate on the same device/BGRA format with TWO buffers. Still process that frame. Extract ID3D11Texture2D through IDirect3DDxgiInterfaceAccess IID A9B3D012-3DF2-4EE3-B8D1-8695F457D3C1, vtable slot 3 GetInterface, texture IID 6f15aaf2-d208-4e89-9ab4-489535d34f9c. Call FrameReady(texture,frame.SystemRelativeTime.Ticks) synchronously; dispose texture and finally frame. Failed interface query gives no frame; errors log "[WgcCaptureSource] frame error: {message}" without killing loop. Dispose marks disposed, detaches pool handler best effort, disposes session then pool best effort, nulls them/item. IsSupported returns GraphicsCaptureSession.IsSupported() or false on exception. WgcCaptureSource.cs:132–211,276–280.

### BGRA → NV12 and scaling

Converter masks output width/height even, queries ID3D11VideoDevice and immediate-context ID3D11VideoContext. Pipeline lazily recreates on INPUT dimension change only, disposing scratch view/texture, staging, processor, enumerator and input copy. VideoProcessorContentDescription: Progressive, source width/height, output even width/height, InputFrameRate=120/1 and OutputFrameRate=120/1 regardless of recording cap, Usage=PlaybackNormal; processor index=0. Set stream source rect entire input, stream dest rect entire output, output target rect entire output, all enabled. This stretches the current source into the session's fixed output dimensions after a source aspect change. Bgra2Nv12Converter.cs:51–58,183–211.

Color settings: input VideoProcessorColorSpace has RgbRange=false (full RGB), every other field default; output has YCbCrMatrix=true and NominalRange=1 (BT.709 limited 16–235), other fields default. WGC SDR BGRA path is 8-bit; there is no HDR tonemap/P010/BT.2020 path. Comments elsewhere calling output full-range are stale. Bgra2Nv12Converter.cs:213–226; NvencVfrEncoder.cs:153–158.

Input copy texture: input W/H, mip=1,array=1,B8G8R8A8_UNorm,sample(1,0),Usage.Default,RenderTarget|ShaderResource,CPU.None,Options.None. CopyResource from pool/composed input first. Per frame create VideoProcessorInputView: FourCC=0,Texture2D,mip=0,arraySlice=0; one VideoProcessorStream with Enable=true,OutputIndex=0,InputFrameOrField=0,PastFrames=0,FutureFrames=0,PInputSurface=view; VideoProcessorBlt(processor,scratchView,0,1,[stream]); dispose input view in finally. Bgra2Nv12Converter.cs:62–88,228–240.

Scratch texture: output W/H, mip=1,array=1,NV12,sample(1,0),Default,RenderTarget,CPU.None,Options.None; output view Dimension.Texture2D,mip=0. This single non-array target is required: direct VideoProcessorBlt to encoder array slices silently left green/unwritten frames on NVIDIA. Hardware ConvertInto then CopySubresourceRegion(scratch,subresource0,whole,dstArray,slice,0,0,0); no normal per-frame CPU fence, same immediate context GPU-orders encoder work. Bgra2Nv12Converter.cs:33–39,95–101,242–259.

Staging NV12 is always allocated when pipeline is created, even for hardware path: output W/H,mip=1,array=1,sample(1,0),Staging,Bind.None,CPU.Read,Options.None. System-memory MapSystemNv12 converts to scratch, CopyResource to staging and MapSubresource(0,Read,None); caller copies Y rows Width bytes each over Height, UV rows Width bytes each over Height/2, source UV starts RowPitch*Height; respects FFmpeg destination linesizes; always unmaps in finally. Bgra2Nv12Converter.cs:110–118,261–273; NvencVfrEncoder.cs:322–346.

### Discovery, encoder context and packet output

FFmpeg initialization/discovery rules and native bindings are in section 6. Encoder probe candidate order and data:

| FfmpegName | CodecId/Tier | FriendlyName | IsHardware |
| --- | --- | --- | --- |
| av1_nvenc | AV_CODEC_ID_AV1 / Av1 | AV1 (NVIDIA) | true |
| av1_amf | AV_CODEC_ID_AV1 / Av1 | AV1 (AMD) | true |
| av1_qsv | AV_CODEC_ID_AV1 / Av1 | AV1 (Intel) | true |
| hevc_nvenc | AV_CODEC_ID_HEVC / Hevc | HEVC (NVIDIA) | true |
| hevc_amf | AV_CODEC_ID_HEVC / Hevc | HEVC (AMD) | true |
| hevc_qsv | AV_CODEC_ID_HEVC / Hevc | HEVC (Intel) | true |
| h264_nvenc | AV_CODEC_ID_H264 / H264 | H.264 (NVIDIA) | true |
| h264_amf | AV_CODEC_ID_H264 / H264 | H.264 (AMD) | true |
| h264_qsv | AV_CODEC_ID_H264 / H264 | H.264 (Intel) | true |
| libx264 | AV_CODEC_ID_H264 / H264 | H.264 (CPU / x264) | false |

CodecTier enum order Av1,Hevc,H264. EncoderChoice contains FfmpegName,CodecId,Tier,FriendlyName,IsHardware. Each candidate requires find_encoder_by_name, alloc_context3, width=1280,height=720,time_base=1/1000,framerate=60/1,pix_fmt=NV12,bit_rate=4_000_000,gop_size=120; avcodec_open2 with no dictionary succeeds iff r>=0; always free context. No hw_device_ctx or special options are passed to probes. Exceptions skip candidate, log; no FFmpeg availability→empty list; no successes logs missing even libx264. Available uses lazy static cache, no cache lock. EncoderSelector.cs:9–12,27–62,123–183.

Select trims/lowercases direct engine preference. Empty available→null. Explicit x264/libx264/cpu picks libx264 if present, otherwise continues auto. Explicit nvenc/amf/qsv picks that vendor's first H264, else that vendor's first entry, else logs and returns avail[0]. av1→Av1,hevc/h265→Hevc,h264/avc→H264 selects first matching family, missing logs and returns avail[0] (not a stepwise lower-tier walk). All other preferences, including empty/auto, pick first hardware H264; absent hardware H264 returns avail[0]. No capture-GPU matching changes this selection order. EncoderSelector.cs:71–120.

NvencVfrEncoder constructor even-masks dimensions; fpsHint<=0→120; bitrateKbps<=0→auto quality. _qsvMode=name contains "qsv"; _hwMode=(nvenc+0x10DE OR amf+0x1002 OR qsv+0x8086) AND not qsv. Thus QSV always gets system NV12 and self-initializes MFX; cross-vendor NVENC/AMF and libx264 also get system NV12. Log path with vendor hex X4; make converter; hardware setup if needed, open encoder, allocate reusable packet/frame. On constructor failure Dispose and rethrow. NvencVfrEncoder.cs:69–111.

Hardware: av_hwdevice_ctx_alloc(D3D11VA); set AVD3D11VADeviceContext.device and device_context to shared device/immediate context, AddRef each for FFmpeg's teardown ownership; av_hwdevice_ctx_init. av_hwframe_ctx_alloc; format=D3D11,sw_format=NV12,width/height=output,initial_pool_size=16; AVD3D11VAFramesContext.BindFlags=RenderTarget|ShaderResource; av_hwframe_ctx_init. Check null/negative results and throw descriptive errors. NvencVfrEncoder.cs:113–141.

Every explicitly assigned AVCodecContext video field:

| Field | Value |
| --- | --- |
| width,height | Even-masked session dimensions |
| pix_fmt | D3D11 in _hwMode, otherwise NV12 |
| colorspace | AVCOL_SPC_BT709 |
| color_range | AVCOL_RANGE_MPEG (limited/tv) |
| color_primaries | AVCOL_PRI_BT709 |
| color_trc | AVCOL_TRC_BT709 |
| time_base | 1 / 1_000_000 |
| framerate | fpsHint / 1, RC hint only |
| bit_rate in Auto | 0 |
| global_quality in QSV Auto | 23 (direct integer, not multiplied by FF_QP2LAMBDA) |
| fixed bit_rate and rc_max_rate | max(1,bitrateKbps)*1000L |
| fixed rc_buffer_size | (int)(bps*2), no overflow guard |
| gop_size | 240 |
| max_b_frames | 0 |
| flags | OR AV_CODEC_FLAG_GLOBAL_HEADER |
| hw_frames_ctx | av_buffer_ref(hwFrames) only in _hwMode |

Other fields remain FFmpeg defaults; no profile, level, thread count, fps numerator correction, rc_min_rate or frame duration is assigned. Global header supplies SPS/PPS/VPS/AV1 config for MP4/MOV/MKV. NvencVfrEncoder.cs:144–195.

Per-encoder private options are applied by av_opt_set(ctx->priv_data,key,value,0); failure logs "[NvencVfrEncoder] opt '{key}={value}' rejected: {Err}" and continues to open. All values are literal strings:

| Backend | Always | Auto (bitrate<=0) | Fixed bitrate |
| --- | --- | --- | --- |
| name contains nvenc | preset=p{clamp(preset,1,7)}, tune=hq, rc-lookahead=0, spatial-aq=1, forced-idr=1 | rc=constqp; qp=22 if name contains hevc, otherwise 20 (AV1 therefore 20) | rc=cbr; multipass=qres |
| name contains amf | quality=speed, preanalysis=false, forced_idr=1 | rc=cqp, qp_i=23, qp_p=23 | rc=vbr_peak |
| name contains qsv | preset=veryfast, forced_idr=1 | no extra option; global_quality=23 above | no explicit rc option |
| exactly libx264 | preset=veryfast, tune=zerolatency, forced-idr=1 | crf=21 | no explicit rc option |

Default p4 is chosen for game-load capture throughput; it is not p7 or low-latency tune ll. Exact option spelling differs forced-idr versus forced_idr as shown. NvencVfrEncoder.cs:69–76,200–250; VfrReplayEngine.cs:26–31.

Encode locks _opLock, returns false if disposed. Start timer; hardware av_hwframe_get_buffer failure logs and skips frame with EncodeLocked returning true; array texture is frame.data[0], slice=(int)frame.data[1], AddRef temporary SharpDX wrapper, convert. System path unrefs frame, sets format NV12,width,height, av_frame_get_buffer(alignment=32), returns skip on failure, maps/copies as above. Set frame.pts=ptsUs. First frame sets lastForcedKfUs (first codec frame assumed IDR); whenever ptsUs-lastForcedKfUs>=2_000_000 set pict_type=I and update lastForced timestamp. No separate wall-clock keyframe scheduler; gop_size 240 is an additional frame-count backstop. Send frame then unref even on send error. Negative send except EAGAIN logs and returns false; EAGAIN is not retried. Drain all currently ready packets until EAGAIN/EOF; negative receive logs/breaks. ConvertUs=(t1-t0)*1e6/frequency and EncodeUs=(t2-t1)*1e6/frequency accumulate only the completed path, TimedFrames++; exceptions log and return false. NvencVfrEncoder.cs:258–370.

Packet record is EncodedPacket(byte[] Data,long PtsUs,long DtsUs,bool IsKeyframe), with new private copy of each AVPacket's payload. Video packet PTS is used directly in microseconds; DTS=PTS when AV_NOPTS_VALUE, otherwise packet DTS. KEY flag OR bitstream parser determines keyframe. Parser only H264/HEVC and payload length>=5. H264 NAL type=(hdr & 0x1F)==5; HEVC type=((hdr>>1)&0x3F) in [16,21]. If starts 00 00 01 or 00 00 00 01, scan i while i+4<n for 00 00 01 and test byte i+3; after a nonkey start increment i by extra 2. Otherwise parse 4-byte unsigned big-endian lengths using long; stop len<=0 or pos+4+len>n; test byte pos+4; advance 4+len. AV1 trusts packet KEY only. Event then av_packet_unref. No packet duration is retained in record. NvencVfrEncoder.cs:8–10,349–406.

GetStreamSpec copies current codec extradata under _opLock, returns codec ID,IsVideo=true,width,height,SampleRate=0,Channels=0,ExtraData. Flush under lock sends null and drains, catches exceptions. Dispose under lock idempotently frees packet, frame, codec context, hwFrames ref, hwDevice ref and converter; Dispose does not independently Flush. NvencVfrEncoder.cs:409–445.

One-time fixed-bitrate fps retune: only BitrateKbps>0, after current frame Encode and EngineNowUs>=3_000_000. Set done=true before further decisions; measured=(int)(FramesEncoded*1e6/max(1,elapsedUs)), floor by integer arithmetic; clamp measured to [5,enc.FpsHint]. If enc.FpsHint-measured < enc.FpsHint*.15, keep current. Otherwise under _gate create a fresh encoder on same D3D/choice/size/bitrate/preset with new hint while old still alive; failure logs/keeps old. On success wire new ring, replace encoder/ring, reset lastEncodedUs, Dispose old (without flushing here). Does not reset audio, engine clock, cadence or cumulative counters; logs reopening/new/dispose/done. Adapt exceptions mark done and log. Captured frame continues in old callback until that return. VfrReplayEngine.cs:531–541,602–652.

### Exact cadence grid

CadenceGrid has fps:long; nominal stepUs=fps>0 ? 1_000_000/fps : 0 (reporting only). Enabled=fps>0; Fps reports fps as double; Drop=-1. Repin sets pinned=false,lastSlot=long.MinValue; it does not clear collisions. TakeCollisions=Interlocked.Exchange(collisions,0). Snap passes input through if fps<=0; first snap pins originUs=ptsUs and lastSlot=min. Thereafter, including first frame:

    delta = ptsUs - originUs
    slot = delta>=0 ? (delta*fps + 500_000L)/1_000_000L
                    : (delta*fps - 500_000L)/1_000_000L
    if lastSlot != long.MinValue AND slot<=lastSlot:
        Interlocked.Increment(collisions); return -1
    lastSlot = slot
    return originUs + (slot*1_000_000L)/fps

All divisions signed integer truncation toward zero, not floor; use 64-bit multiplication with the same normal source-range assumptions. Positions derive from fps, not rounded step, preventing accumulating drift at e.g. 165 Hz. Empty slots are left empty; no duplicate frame is made. Nearest-slot quantization can move time by approximately half a period; collisions/backward slots drop. Only capture thread mutates grid except atomic counter drain. CadenceGrid.cs:27–97.

### Audio capture and process loopback

StartAudio skips entirely if AudioEnabled=false and logs video-only. Otherwise create WasapiAudioSource; gamePid is locked PID only for Game. Effective apps mode=AudioModeByTarget ? lockKind==Game : AppsAudioMode. Start source with mic ID, apps tuples, system enable/gain, mono, game PID/gain/enable, selected output ID; then set mic gain, initial mute, gate and suppression. Capture starts before engine callbacks attach and before these post-Start controls apply; early PCM may be lost. Failures log "[VfrReplayEngine] audio capture unavailable — video-only: {message}" and leave video running. VfrReplayEngine.cs:676–696,777–780.

WasapiAudioSource clamps only negative mic/system gain to zero, no upper bound; app/game gain is stored directly. Effective _appsMode=appsMode && (apps.Count>0 OR gameEnabled&&gamePid!=0). If true start app captures; else if systemEnabled start system loopback; else log system disabled. Consequently requested empty apps mode with no eligible game can fall back to full-system audio. GameFormat is fixed 48000/2/IEEE float in effective apps mode, otherwise endpoint WaveFormat if present, otherwise same fixed format for mic-only/silence. WasapiAudioSource.cs:24–49,61–85,90–105.

Whole-system loopback chooses default Render/Multimedia for null/empty SystemAudioDeviceId; a specified GetDevice failure logs and falls back to default. Create WasapiLoopbackCapture(render), subscribe DataAvailable, StartRecording, log endpoint/sample-rate/channels/bits/encoding. Device wrapper is disposed after setup. OnGameData drops disposed/empty; for IEEE float and systemGain!=1 copy bytes, multiply every float, clamp [-1,1]; other formats ignore gain. Emit GameDataReady synchronously with endpoint format. WasapiAudioSource.cs:124–150,249–257,349–360.

Per-app setup: HashSet<uint> capturedPids. First attempt enabled detected-game PID with GameVolume; add PID only on successful Start. For each selected tuple strip terminal ".exe" case-insensitively, Process.GetProcessesByName; none logs/skips. Prefer first process with MainWindowHandle!=0 else first; only that one process tree is captured. If already captured PID, log duplicate and skip; otherwise create ProcessLoopbackCapture, queue callback, synchronous Start, append AppMix with raw Gain, log pid/gain:F2. Catch per app/capture, dispose all Process wrappers. If no successful app captures, set _appsMode=false and call StartSystemLoopback unconditionally, even when SystemAudioEnabled=false. No live retry/re-enumeration as apps start/exit later. WasapiAudioSource.cs:155–210.

Each app callback views bytes/4 floats, ignores trailing incomplete sample, locks its queue; if current queue.Count>48000*2, discard the entire incoming buffer; otherwise enqueue each sample*gain (so one incoming buffer can exceed the nominal 1 s limit). 20 ms timer sum uses 1920 floats (960 stereo frames), fresh zero array, dequeue up to chunk length from each source at its head, sum index-aligned, clamp all floats to [-1,1], block-copy to fresh 7680-byte array and emit GameDataReady even when every queue empty. Callback catches everything; no timestamp alignment/resampling per app (all are fixed format), no SystemVolume gain on this app sum. WasapiAudioSource.cs:46–49,213–246.

ProcessLoopbackCapture forced WAVEFORMATEX: tag=3,channels=2,rate=48000,bits=32,blockAlign=8,avgBytes=384000,cbSize=0. Activate virtual "VAD\Process_Loopback" for IAudioClient with AUDIOCLIENT_ACTIVATION_PARAMS {ActivationType=1,TargetProcessId=pid,ProcessLoopbackMode=0 INCLUDE_TARGET_PROCESS_TREE}, passed as PROPVARIANT VT_BLOB=0x0041. Completion handler GUID 41D949AB-9862-444A-80F6-C261334DA5EB sets ManualResetEvent and returns 0. Wait activation 5000 ms or throw "ActivateAudioInterfaceAsync did not complete (process loopback)."; GetActivateResult nonzero activationHr throws. All setup/use on dedicated MTA. Public Start waits _ready up to 6000 ms but does not test the wait's boolean; only rethrows _startError if set. ProcessLoopbackCapture.cs:21–34,57–88,97–131,163–190,216–240.

IAudioClient.Initialize(shareMode=0 shared,flags=LOOPBACK 0x00020000|EVENTCALLBACK 0x00040000,bufferDuration=2_000_000 ticks=200ms,periodicity=0,format,sessionGuid=null). CreateEventW(manualReset=false,initial=false,name=null); SetEventHandle; GetService IAudioCaptureClient; Start; any checked hr!=0 throws. Capture loop WaitForSingleObject(event,100) (return ignored), then drain while GetNextPacketSize returns 0 and frames>0. GetBuffer returns positions but both ignored; error/zero frames breaks (successful zero still ReleaseBuffer(0)). For each normal packet allocate frames*8 bytes; SILENT flag 0x2→zeros else Marshal.Copy; ReleaseBuffer before raising DataAvailable in catch-all. Run catches capture-loop failures, stops client and releases capture/client COM on the same MTA. Dispose sets running=false, signals event, joins thread up to 2000ms, nulls thread and closes kernel event; it never releases COM from caller thread. No runtime build gate, no MMCSS explicit setup, no format query. ProcessLoopbackCapture.cs:97–159,193–211,233–310.

Mic ID exactly "" disables mic and logs; null chooses Capture/Communications default; nonempty ID uses GetDevice (no fallback if invalid). Create WasapiCapture, subscribe, StartRecording, log; mic setup exceptions are nonfatal game-audio-only. Per-buffer disposed/empty/muted returns without advancing gate/denoiser. Float format only: apply linear gain on copy and clamp if gain!=1; compute gate level from ORIGINAL raw buffer peak, clamp peak to1, perceptual Curve=peak^.6; dt=bytes/AverageBytesPerSecond if positive else .01; compute gate; if newGain<.999 OR lastGain<.999, copy if needed and ramp each interleaved float sample i by lastGain+(newGain-lastGain)*(i/(float)n), n=count/4. Last sample does not reach exact target; ramp crosses interleaved channels, not frame pairs. Store lastGain (initially 1). Next optional denoiser, then optional mono, then MicDataReady. Integer PCM bypasses float gain/gate/denoise/mono; still captured/encoded. WasapiAudioSource.cs:105–121,259–308,322–360.

Mono only for exactly 2 channels IEEE float: frames=count/4/2; new mono float bytes; dst[i]=(L+R)*.5; format same rate/1ch. Does not change eventual AAC stereo output. Mic suppression Process true with zero output→drop buffer; Process false→pass current gain/gated input unchanged. WasapiAudioSource.cs:288–319.

Source Dispose marks disposed, disposes mix timer then each process capture, clears app list, stops mic callback before denoiser, stops loopback, disposes denoiser. StopOne unsubscribes, StopRecording, Dispose in one best-effort try. WasapiAudioSource.cs:363–378.

### Noise gate and RNNoise

NoiseGate starts Auto=true,ManualThreshold=.08,Threshold=0,IsOpen=false,noiseFloor=.02,gain=0,hold=0. Process t=(float)clamp(dt,.0001,.5). Auto rate=2.5 if level<noiseFloor else .03; noiseFloor+=(level-noiseFloor)*min(1,rate*t); clamp floor [.004,.5]; Threshold=clamp(noiseFloor*2.3,.012,.9). Manual Threshold=clamp(ManualThreshold,0,1). above=level>=Threshold; hold=above?.28:max(0,hold-t); IsOpen=above||hold>0; target=IsOpen?1:0; gain+=(target-gain)*min(1,(target>gain?70:7)*t); if gain<.0005 then 0; return gain. Reset clears gain/hold and resets floor=.02 but DOES NOT reset Threshold/IsOpen. NoiseGate.cs:18–38,42–76.

MicDenoiser is single-threaded per microphone, no extra native RNNoise DLL: uses avfilter-10 and BaseDirectory/rnnoise/std.rnnn. Enabled default false; Process false/pass-through if disabled, permanently failed, byteCount<=0 or invalid sample count. Rebuild graph when not ready or sample rate/channels changed; failed Setup marks _failed=true for instance lifetime. Graph:

    abuffer instance "in":
      time_base=1/{rate}:sample_rate={rate}:sample_fmt=flt:channel_layout={layout}
    arnndn instance "denoise":
      allocate filter, av_opt_set(ctx,"model",absoluteModelPath,AV_OPT_SEARCH_CHILDREN)
      avfilter_init_str(ctx,null)
    aformat instance "fmt":
      sample_fmts=flt:sample_rates={rate}:channel_layouts={layout}
    abuffersink instance "out"
    links 0→0 between each; avfilter_graph_config(graph,null)

Using av_opt_set for model avoids splitting drive-letter colon. LayoutString uses default channel layout→describe into 128-byte buffer→uninit; if absent/error fallback mono for channels=1 else stereo. Missing file/filter or negative create/link/config returns false with available diagnostic; setup catches/logs. Success allocates reusable input/output frames and logs "[MicDenoiser] arnndn (RNNoise) graph ready ({rate}Hz {channels}ch).". MicDenoiser.cs:19–53,98–188.

For each input, unref frame; format=FLT,rate,nb_samples=byteCount/(channels*4),default layout; get_buffer(0), copy complete interleaved sample bytes; av_buffersrc_add_frame with no explicit PTS. Drain sink until EAGAIN/EOF/error; append each returned nb_samples*channels*4 bytes to reusable output accumulator, unref output frames; return true with total possibly 0 (RNNoise buffers 10ms). Accumulator starts 8192 on first growth, doubles until enough, copies previous capacity bytes; consumer must synchronously use the valid count. Frame allocation/source failure returns false without latching failure; exceptions log and latch _failed=true. Teardown frees input/output frames and graph, ready=false, nulls src/sink; does not clear _failed, Enabled or output capacity. Dispose invokes teardown. MicDenoiser.cs:55–96,190–210.

### AAC, mixing, silence and track creation

Audio sample-format mapper: Encoding==IeeeFloat→FLT; else bits=16→S16; else bits=32→S32; else FLT (no special WaveFormatExtensible/24-bit handling). VfrReplayEngine.cs:807–811.

Separate mode creates All first with game format, bitrate=192, ring; System next same format bitrate=160; Microphone placeholder last with null encoder/ring. Game callback (if not paused) gets EngineNowUs once and encodes into All then System. Mic callback gets same-clock t, queues EncodeMic into All; lazily opens mono/input-format-aware microphone AAC encoder on first delivered mic buffer, bitrate=160, ring, fills placeholder, then Encode mic-only at t. Mic placeholder is in track list immediately but omitted from save until it has encoder/ring/packets. Single mode creates one Audio track at160; game callback Encode at now, mic callback EncodeMic only. All AAC outputs are stereo48k regardless of input. Constants AllTrackKbps=192,SourceTrackKbps=160; reason for larger All is measured Windows media auto-selection by highest bitrate rather than MP4 default alone. VfrReplayEngine.cs:672–766.

After source/track callback wiring publish _audio/tracks under _gate and create timer due=100ms,period=100ms. PumpAudio no-ops disposed/not running; obtains now once; copies tracks under gate and calls each encoder.PumpSilenceTo(now), catches/logs each encoder error. Does not require video frames or system sound, so audio may begin before first video and continue during a video-source stall. Called again by save before snapshots. VfrReplayEngine.cs:769–795,833–837.

AAC constructor finds AV_CODEC_ID_AAC and allocates context: sample_rate=48000,bit_rate=max(64,requestedKbps)*1000L,sample_fmt=FLTP,ch_layout=default stereo,time_base=1/48000,flags OR GLOBAL_HEADER; no other options/profile/thread fields assigned; open2(null options). Create master swresampler from input default layout/rate/format to FLTP stereo48k using swr_alloc_set_opts2(...,0,null), swr_init; allocate master and mic AVAudioFifo FLTP/2ch/initial size1; allocate frame/packet. Log original requested kbps. Constructor has no catch/dispose rollback. AacAudioEncoder.cs:19–40,46–79.

Encode under _opLock ignores disposed/byteCount<=0; inSamples=byteCount/(inBytesPerSample*inChannels), return <=0. swr_get_out_samples; allocate two float planes outMax*sizeof(float); swr_convert from one interleaved input pointer using initially configured master resampler; under _writeLock, if got>0 PadTo(chunkEngineUs), MixMicInto(real loopback planes,got), FIFO write got, writtenSamples+=got, DrainFrames; always free planes. Input metadata supplied on later calls does not rebuild master resampler. AacAudioEncoder.cs:85–128.

Timeline is actually anchored at session sample index 0, NOT the constructor comment's first-sample-offset implementation. PadTo: expected=targetUs*48000/1_000_000L; gap=expected-writtenSamples; only if 0<gap<=48000*5 write that many silence samples with mic, then writtenSamples+=gap. Gaps >5 seconds are ignored in their entirety; no seek/reanchor. Incoming real chunk is then appended, even if writtenSamples already exceeds expected. PumpSilenceTo takes _opLock then _writeLock, PadTo(now), DrainFrames. Silence helper uses reusable-per-call two 4096-float allocations, clears each chunk, mixes queued mic, writes into master FIFO, repeats; no wall-clock sleeps. AacAudioEncoder.cs:94–123,138–183.

EncodeMic under _opLock; derive complete input samples; lazy InitMicSwr on first buffer from mic rate/layout/format to FLTP stereo48k; no later format-change rebuild once _micReady. Alloc/swr init failure drops mic; successful init logs input format. swr_convert to two new planes; under _micLock sz=micFifoSize; if sz+got>48000 drain min(sz,sz+got-48000) oldest then append got. This caps normal backlog to1s but one oversize got can exceed cap. MixMicInto under mic lock reads min(requested,available), allocates two mic planes, sums into master planes, clamps [-1,1], frees; no ducking or averaging. Queue alignment is arrival/FIFO-based, not timestamp-based. Mic-only source track does not use EncodeMic and receives its own master silence padding. AacAudioEncoder.cs:188–275.

DrainFrames uses ctx.frame_size if >0 else1024. While master FIFO>=frameSize: unref frame; nb_samples=frameSize,format=FLTP,copy channel layout,sample_rate=48000,get_buffer(0); read FIFO into frame planes; pts=readSamples; readSamples+=frameSize; Submit. Submit sends, ignores EAGAIN as nonfatal without retry; drains receive until EAGAIN/EOF/error. Copy payload, ptsUs=packet.pts==NOPTS ? 0 : packet.pts*1e6/48000; DTS=PTS; IsKeyframe=true for EVERY AAC packet; event then unref. Codec priming may produce negative PTS and is not separately offset; rebasing drops negative-before-video packets. StreamSpec=AAC,false,0,0,48000,2,copied ASC extradata. Flush just Submit(null); partial FIFO, mic FIFO and resampler delay are not padded/flushed first. Dispose frees packet/frame/FIFOs/resamplers/context under _opLock, no automatic Flush. AacAudioEncoder.cs:277–358.

### Packet rings, snapshots and save

PacketRingBuffer constructor bufferUs=(long)(max(5,bufferSeconds)*1e6). Storage linked packet list plus FIFO queue of keyframe nodes. Add under lock appends; enqueue if key; cutoff=newPacket.PtsUs-bufferUs. While >=2 keyframes: find second queue node by iteration; if missing OR second.PtsUs>cutoff stop; dequeue first keyframe, remove packets before second (retain second). This preserves latest keyframe at/before cutoff; no byte-size cap; without recurring recognized keyframes memory is unbounded. Audio marks all packets key so trims at packet cadence. BufferedSeconds=(last.PTS-first.PTS)/1e6, 0 empty/single; OldestPtsUs nullable first PTS. PacketRingBuffer.cs:20–80.

Snapshot(out absBaseUs,minStartUs=long.MinValue): absBase initially0; under gate empty list if no packets/no keyframe. cutoff=last.PTS-bufferUs; choose latest queued keyframe<=cutoff, else earliest. If minStart supplied AND minStart-start.PTS>500_000, advance to first keyframe>=minStart if one exists, log advancement seconds:0.0; if none, keep original start. From selected start through end return new packet records with PTS and DTS each minus start.PTS; absBase=start.PTS. Snapshot() just discards returned base. SnapshotRebasedFrom(base) returns EVERY buffered packet with PTS>=base, both times minus base, no keyframe constraint and no upper bound. SnapshotAbsolute copies list references unchanged absolute times; Clear clears list and queue under gate. PacketRingBuffer.cs:88–185.

Save takes local ring,encoder,carry and copy of track-state references under _gate, then releases it. If live ring/encoder present, PumpAudio, compute minStart as MAX OldestPtsUs over all existing audio rings, including a young mic track; video Snapshot(out base,minStart). Only if video nonempty get current video spec and rebase each eligible audio ring from SAME video base, include nonempty tracks in their existing order. No live video spec and no carry→log "[VfrReplayEngine] SaveReplay: no active capture and nothing carried over.", return null; no ReplaySaved event. A Save with ring but no keyframe also takes this branch. VfrReplayEngine.cs:817–865.

Create local-time output path (section 5), clear LastSavedGameTag at this point (not on earlier no-data return). No carry→Mp4Muxer.Write live. With carry: rebase head video/audio from carry.StartUs, set LastSavedGameTag if carry.WasGame; if liveSpec null OR headVideo.Count==0 attempt head-only write (head must nonempty). Otherwise offset=headVideo.last.PTS+16_000; compare exact compatibility (below). Same stream→head video + Shift(liveVideo,offset), MergeAudio, mux with carry's VideoSpec. Different→TranscodeStitch using options bitrate/preset. Failure→log halves separately, write live to primary path; write carried to same stem+"-Game"+ext; if head write succeeds emit ReplaySaved(headPath) even if live fails. Return null if primary write false; otherwise emit ReplaySaved(primary), return path. Never clear ring or carry after successful save. VfrReplayEngine.cs:867–936.

MergeAudio's track list is defined by LIVE tracks, in their order. Shift each live PTS/DTS by same seam offset. Find first head track with case-insensitive equal title AND codec/sampleRate/channels/extradata equality; if found prepend head packets, use live Spec/title. If none, only shifted live packets (implicit silent head, no synthetic packets). Head-only tracks are discarded; no channel/codec conversion in direct merge. VfrReplayEngine.cs:948–970.

### Carry-over and ReplayStitcher

CaptureCarryOver after teardown flush: if ring/spec missing or absolute video empty, keep existing older carry. Copy nonempty absolute audio lists and specs best-effort; create CarryOverSegment(VideoSpec,Video,Audio,WasGame,TargetName,TargetExe), TrimmedToLast(options.BufferSeconds), replace _carry under gate (can become null). Holds only ONE previous session; if an intervening session records any footage it replaces older carry rather than recursively merging history. TargetName/Exe are preserved for later clip tagging. VfrReplayEngine.cs:1019–1048.

Carry uses absolute timestamps in that dead session's own clock. Start=firstVideo.PTS or0, End=lastVideo.PTS or0, Duration=(End-Start)/1e6, IsEmpty=count==0; duration excludes final packet duration. TrimmedToLast(seconds): return null if no video or seconds<=0; cutoff=End-(long)(seconds*1e6); scan keyframes in order, retain latest<=cutoff; if first later keyframe occurs before any earlier candidate, take that one; break at first later. No keyframe→null; chosen index0→return same object, even if companion audio predates first video; otherwise copy video suffix, cut each audio to PTS>=newVideoStart, omit empty audio tracks, preserve specs/tag. Audio/video payload arrays are shared, not copied. VideoRebased subtracts Start from PTS and DTS; AudioRebased drops PTS<Start then subtracts Start likewise. CarryOverSegment.cs:27–107.

Every lock tick TrimCarryOver reads carry and live ring span; room=options.BufferSeconds-live. If room>.5, carry.TrimmedToLast(room), else null. If same reference do nothing; otherwise replace only if _carry still references captured carry. Null logs release. Keyframe alignment can retain up to a GOP beyond requested room; timer granularity and packet spans mean comments promising exactly <=window are approximate. Carry does not expire with wall time while live footage is not accumulating. SimulateTargetLoss() simply TeardownCapture(true) for tests. VfrReplayEngine.cs:1008–1014,1051–1066.

ReplayStitcher.SeamGapUs=16_000, exactly (not 16_667). Shift adds offset to both times preserving payload/key. CanConcatDirect compares CodecId,Width,Height and byte-for-byte ExtraData; ignores IsVideo and other fields. CanConcatAudio compares CodecId,SampleRate,Channels,ExtraData; null extras equal only null, reference equality shortcut, otherwise length/content equality. ReplayStitcher.cs:25–56.

TranscodeStitch locates BaseDirectory/ffmpeg/ffmpeg.exe; missing logs/returns false. Creates temp Lag-stitch-{first8GuidN}, head.mp4/live.mp4. aCount=min(headAudio.Count,liveAudio.Count); keep first aCount tracks by INDEX (not title), log differing count. Mux each half to MP4; failure returns false. Output size w/h=live spec. Exact filter assembly:

    [0:v]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,format=yuv420p[v0];
    [1:v]scale={w}:{h},setsar=1,format=yuv420p[v1];
    [v0][0:a:0]...[0:a:N-1][v1][1:a:0]...[1:a:N-1]concat=n=2:v=1:a={N}[v][a0]...[aN-1]

Assemble without whitespace/newlines between filter fragments. Head letterboxes, live scales directly; no setpts/asetpts, aresample, explicit silence-fill, explicit VFR mode or color tags. No 16ms seam option here: concat filter chooses timing. ReplayStitcher.cs:65–108.

Exact subprocess arguments, with audio clauses only when N>0:

    -y -hide_banner -loglevel error -i "{headTmp}" -i "{liveTmp}" -filter_complex "{filter}" -map "[v]" -map "[a0]" ... -c:v {VideoEncoderFor(liveCodec)} -preset p{clamp(preset,1,7)} -b:v {max(1000,bitrateKbps)}k -c:a aac -b:a 160k -b:a:0 192k -metadata:s:a:0 title="{liveTitle0}" ... "{outPath}"

VideoEncoderFor is AV1→av1_nvenc,HEVC→hevc_nvenc,anything else→h264_nvenc regardless of capture GPU. Strip double quotes from audio titles, retain remaining characters; audio metadata titles are live-half titles. No faststart or disposition clause. If first Run fails, log hardware fallback and replace only "-c:v {hardware} -preset pN " with "-c:v libx264 -preset veryfast ", retaining all other arguments (therefore may change output codec to H264). Even Auto bitrate uses at least1000k here. Run uses UseShellExecute=false,CreateNoWindow=true,redirect stderr AND stdout; synchronously ReadToEnd stderr then WaitForExit with NO timeout; stdout never drained. Nonzero exit logs last400 trimmed chars of stderr; success logs elapsed ms. Finally try delete temp directory recursively; return false on caught transcode exception. Temp directory creation precedes try. ReplayStitcher.cs:110–194.

### Detection and auxiliary services

GameCandidate holds Hwnd,Pid,Exe,GameName,Score,Fullscreen. FindBestGame and RunningGameExes get current foreground, refresh GPU sample and Steam running boolean ONCE per scan, EnumWindows all top-level windows. FindBest keeps first winner on ties (strict >); RunningGameExes returns case-insensitive exe set for same qualifiers. Public Evaluate does not sample signals itself. GameDetector.cs:9,120–161.

Reject HWND0, invisible, iconic, PID0, own PID, process-name lookup failure; exe=ProcessName+".exe". Reject case-insensitive shell and GPU-nongame sets BEFORE positive signals. Known name comparisons strip trailing .exe. Fullscreen requires GetWindowRect, MonitorFromWindow(hwnd,2 NEAREST), monitor info; selected monitor must case-insensitive match explicit nonempty device ID, otherwise primary flag1. Rect must cover all four monitor edges (<=left/top,>=right/bottom); no tolerance, overlap percentage, client rect or DWM cloaking test. Known-name games can qualify outside selected monitor and windowed; engine-class alone cannot. GameDetector.cs:161–203,223–236.

Exact qualifying formula:

    knownByName OR (fullscreen AND (gpu3D OR steamGameRunning OR engineClass))
    gpu3D := last summed process 3D utilization >= 30.0
    score = (knownByName?46000:0) + (fullscreen?30000:0)
          + (hwnd==foreground?18000:0) + (gpu3D?46000:0)
          + (steamGameRunning?30000:0)

Engine class carries no direct score bonus. Steam bonus applies globally to every otherwise qualifying candidate, not to a PID belonging to that Steam game. Steam reads HKCU\Software\Valve\Steam\RunningAppID, only nonzero int qualifies; exceptions=false. Friendly name is table value else trimmed GetWindowTextW(buffer256), else process name without extension; window class buffer256. GameDetector.cs:31–38,176–218,239–249.

Complete known-name map (case-insensitive; aliases listed together):

| Process names without .exe | Friendly name |
| --- | --- |
| cs2 | Counter-Strike 2 |
| csgo | CS:GO |
| dota2 | Dota 2 |
| deadlock | Deadlock |
| valorant-win64-shipping, valorant | VALORANT |
| r5apex, r5apexdx12 | Apex Legends |
| fortniteclient-win64-shipping | Fortnite |
| gta5, gtav, gta-v | GTA V |
| rocketleague | Rocket League |
| league of legends, leagueoflegends | League of Legends |
| overwatch | Overwatch 2 |
| tslgame | PUBG: BATTLEGROUNDS |
| rainbowsix, rainbowsix_vulkan | Rainbow Six Siege |
| destiny2 | Destiny 2 |
| bf2042 | Battlefield 2042 |
| bf1 | Battlefield 1 |
| bfv | Battlefield V |
| eldenring | Elden Ring |
| marvel-win64-shipping | Marvel Rivals |
| discovery | THE FINALS |
| helldivers2 | Helldivers 2 |
| palworld-win64-shipping, palworld | Palworld |
| rust | Rust |
| dayz | DayZ |
| warthunder, aces | War Thunder |
| modernwarfare, cod | Call of Duty |
| minecraft.windows | Minecraft |
| hl2 | Half-Life 2 |
| portal2 | Portal 2 |

GameDetector.cs:46–71. Complete case-insensitive engine classes: UnrealWindow, SDL_app, riotWindowClass, CryENGINE, Valve001, Sledgehammer Oasis. No Unity class despite summary prose. GameDetector.cs:75–83.

Complete ShellExclude: explorer.exe,dwm.exe,applicationframehost.exe,searchhost.exe,shellexperiencehost.exe,startmenuexperiencehost.exe,textinputhost.exe,lockapp.exe,sihost.exe,ctfmon.exe,lag.exe. Complete NonGameGpuExclude: chrome.exe,msedge.exe,firefox.exe,librewolf.exe,brave.exe,opera.exe,opera_gx.exe,vivaldi.exe,browser.exe,arc.exe,discord.exe,discordcanary.exe,discordptb.exe,slack.exe,ms-teams.exe,teams.exe,spotify.exe,code.exe,claude.exe,notion.exe,whatsapp.exe,telegram.exe,obs64.exe,obs32.exe,obs.exe,streamlabs obs.exe,xsplit.core.exe,vlc.exe,mpc-hc64.exe,mpc-hc.exe,mpc-be64.exe,potplayermini64.exe,potplayer64.exe,mpv.exe,wmplayer.exe,steam.exe,steamwebhelper.exe,epicgameslauncher.exe,battle.net.exe,galaxyclient.exe,eadesktop.exe,ealauncher.exe,riotclientux.exe,riotclientservices.exe. GameDetector.cs:88–115.

GpuUsageProbe lazily opens PDH once, latches _opened even on failure, adds English wildcard "\GPU Engine(*engtype_3D)\Utilization Percentage", primes collect then each Sample collects again. PdhGetFormattedCounterArrayW first query must return PDH_MORE_DATA=0x800007D2,size>0; allocate requested buffer; second status0; parse entries where CStatus==0 and name nonnull, parse first ordinal "pid_" then consecutive char.IsDigit into uint; ignore PID0; SUM all 3D instances per PID, no100 clamp. PDH_FMT_DOUBLE=0x200; native item layout pointer,uint,padded double (x64 alignment). Successful complete sample replaces map; errors/early returns leave previous map (stale data), unknown PID returns0. Dispose closes query, nulls handles; GameDetector does not implement IDisposable and never disposes its probe in current engine. GpuUsageProbe.cs:21–124; GameDetector.cs:27–41.

AudioSessionService uses default Render/Multimedia endpoint sessions, only AudioSessionStateActive, excludes PID0/self, deduplicates ExeName=ProcessName+".exe" ordinal-ignore-case (one per executable, not PID). Display initially process name; OpenProcess(0x1000 QUERY_LIMITED_INFORMATION,false,pid), QueryFullProcessImageNameW(flags0,buffer1024), CloseHandle in finally; path optional. Nonblank FileVersionInfo.FileDescription trimmed replaces display; icon=ExtractAssociatedIcon→ToBitmap→PNG byte array (failure=null). Per-process failure skips; global failure Debug.WriteLine; return sorted DisplayName ordinal-ignore-case. Not all render endpoints and not persistent session IDs. AudioSessionService.cs:22–123.

AudioDeviceWatcher constructs MMDeviceEnumerator and registers IMMNotificationClient in constructor, records _registered only on success; registration exception Console log. StateChanged,Added,Removed,DefaultDeviceChanged invoke parameterless DevicesChanged synchronously with no filtering/debounce; PropertyValueChanged ignored. Dispose best-effort unregister if registered then enumerator Dispose. Consumer marshals/debounces and refreshes picks/meters, not active recorder automatically. AudioDeviceWatcher.cs:14–39; SettingsViewModel.AudioMeter.cs:27–67.

ScreenshotService EnumDisplayMonitors/GetMonitorInfo returns physical rcMonitor for case-insensitive exact GDI name, or primary flag1 if name is null; empty string is NOT the same as null but falls back via second primary search. Return null if no rect or dimensions<=0. Directory.CreateDirectory output; Bitmap(w,h), Graphics.FromImage.CopyFromScreen(rect.Left,rect.Top,0,0,Size(w,h)), save PNG, return path; no recorder overlay/cursor rendering, WGC, HDR handling or region selection. Catch all: Debug.WriteLine("[Screenshot] capture failed: {message}"), return null. ScreenshotService.cs:14–94.

WebcamCaptureSource.ListDevicesAsync uses DeviceInformation.FindAllAsync(DeviceClass.VideoCapture), preserves returned order, DeviceInfo(Id,Name).ToString()=Name; errors log and empty list. Start: MediaCaptureInitializationSettings Video-only and MemoryPreference.Cpu, set VideoDeviceId only nonempty; initialize synchronously; first FrameSource Color else throw "camera has no color stream". Choose SupportedFormats with VideoFormat!=null and fps>=15, order abs(width-640), THEN FPS ascending (prefers lower qualifying FPS on width tie), first; SetFormat failure ignored, stay default. Fps numerator/denominator if denominator>0 else0. CreateFrameReader, AcquisitionMode.Realtime, subscribe, StartAsync status must Success or throw. Logs chosen mode even if setting it failed. WebcamCaptureSource.cs:31–95.

Callback gets latest frame, skips absent SoftwareBitmap; if not BGRA8 AND premultiplied already, SoftwareBitmap.Convert to both desired attributes. Under gate maintain exact w*h*4 byte array, CopyToBuffer, store dimensions, seq++. Dispose converted bitmap only when distinct, frame via using; errors log each frame. TryCopyLatest initializes output dimensions0, locks, false if no frame or same sequence (caller buffer/sequence unchanged); allocate caller buffer matching size, Array.Copy full bytes, output dimensions and update lastSeq. Despite "double-buffer" summary, actual code uses one internal array and a copying caller buffer, protected by lock. Dispose marks disposed, unsubscribes, synchronous StopAsync, reader Dispose in try; capture Dispose separately; null reader/capture. WebcamCaptureSource.cs:97–167.

SystemStatsSampler opens/prime same English PDH 3D wildcard (no retry), GetSystemTimes baseline, timer1000/1000. Snapshot initially all zero. CountFrame Interlocked.Increment; tick FPS=(int)Interlocked.Exchange(count,0), not normalized by actual elapsed timer interval. CPU=(int)clamp(100.0*(dKernel+dUser-dIdle)/(dKernel+dUser),0,100) when total>0, kernel includes idle. GPU sum all valid CStatus0 values, clamp to0..100 then truncate int; failure0. RAM GlobalMemoryStatusEx physical total and total-available divided by1073741824.0 (GiB displayed as GB). Publish volatile new record; failure keeps previous snapshot. Dispose timer/query. SystemStatsSampler.cs:16–113.

### Keystroke state and label rules

KeystrokeTracker singleton has volatile Active, gate-protected insertion-ordered List<Entry>,max10 entries. Entry.Label immutable; DownAtTicks,UpAtTicks mutable; UpAtTicks=0 means held. Active=false clears all entries under gate, Active=true does not clear. OnKey/OnMouse are a single active-check when disabled. Push ignores null labels; now=DateTime.UtcNow.Ticks; down finds equal label, if absent and count>=10 remove index0 even if held, then append with DownAt=now; existing label down refreshes DownAt and UpAt=0 without moving its insertion order. Up only sets UpAt on an existing entry, does not create one. Snapshot scans backwards removing released entries ONLY when upMs>450 (exactly450 remains opacity0), then emits insertion order with held opacity1 or clamp(1-upMs/450,0,1). Wall-clock changes affect fade; this is not engine pause time. KeystrokeTracker.cs:17–105.

Mouse mapping button1=LMB,2=RMB,3=MMB,4=M4,5=M5,other="M{button}". Keyboard mapping starts KeyCode.ToString(); require prefix "Vc", strip two chars. Exact switch: LeftShift/RightShift→Shift; LeftControl/RightControl→Ctrl; LeftAlt/RightAlt→Alt; LeftMeta/RightMeta→Win; Space→Space; Enter→Enter; Backspace→Bksp; Escape→Esc; Tab→Tab; CapsLock→Caps; Up/Down/Left/Right→↑/↓/←/→; Comma→","; Period→"."; Slash→"/"; Backslash→"\"; Semicolon→";"; Quote→"'"; Minus→"-"; Equals→"="; OpenBracket→"["; CloseBracket→"]"; Backquote→U+0060 grave accent. Remaining switch guards, in order: name.Length==1→name; Length==2 && name[0]=='1'→name; StartsWith("F") && Length<=3→name; Length==1 OR Length==2 && char.IsDigit(name[1])→name; explicit strings0..9→name; else null. Preserve these permissive guards rather than inventing a narrower key list. Both left/right modifiers merge into one label, so releasing either can fade a still-held other modifier. Tracker accepts many labels the fixed keyboard compositor does not display. KeystrokeTracker.cs:48–55,108–148.

### Unused implementations and dead legacy cluster

These behaviors are documented to distinguish them from the acceptance target, not to enable them in Rust:

- HybridGameCaptureSource: starts monitor DXGI duplication at center of GetWindowRect, output finder searches only shared adapter bounds, fallback output0; accepts monitor frames only when foreground HWND equals game (or duplicationOnly), lazily starts WGC on first background check, then accepts WGC only in background. _emitGate rejects pts<=last; WGC callback's long is actually 100ns ticks passed through a handler named ptsUs, unlike duplication microseconds. Lazy-start flag is set before Start and never retried after failure. Its alleged seamless hybrid path is unreachable from current engine. HybridGameCaptureSource.cs:49–119; WgcCaptureSource.cs:152.
- DxgiDuplicationCaptureSource: DuplicateOutput, background "DxgiDuplication" thread AboveNormal, AcquireNextFrame timeout100ms; emit only LastPresentTime!=0 and distinct, convert QPC ticks*1e6/frequency. Release resource/frame in finally. AccessLost disposes and waits50ms then recreate; errors sleep2ms; null duplication sleeps5ms forever if recreation failed. Dispose join500ms then release. This is not the independent DuplicationProbe, which is reachable. DxgiDuplicationCaptureSource.cs:48–149.
- RecordingEngine dead defaults: monitor0,fps30,h264_nvenc,segment5s,buffer5min,temp %TEMP%\Lag_Segments,library Videos\Lag. Task.Run initializes old capture and deletes leftover temp folder, disk watcher "segment_*.mp4" adds after500ms, LongRunning Task pipes BGRA synchronously at integer1000/fps sleeps; save selects segment paths, concat stream-copy, thumbnail JPG, "Replay_{local timestamp}.mp4"; stop cancels, awaits capture, stops encoder, shuts source/watcher. None is called by current DI/UI. RecordingEngine.cs:30–50,78–140,152–209,222–304.
- WindowsCaptureService dead: enumerates DXGI adapters/outputs, primary by left/top==0, staging BGRA readback per row, AccessLost marks uninitialized (does NOT recreate despite summary), non-Windows stub. Its MonitorInfo/CapturedFrame/IScreenCaptureService are isolated from HardwareDetector.MonitorInfo used by settings. WindowsCaptureService.cs:42–126,139–231; IScreenCaptureService.cs:6–77.
- RingBufferManager dead: disk segments default max60/duration5s; lock-protected linked list; add stamps UTC and deletes oldest over capacity; reverse walk sums durations then reverses path list; Clear deletes files, only IOExceptions swallowed. Do not replace live packet buffer with this design. RingBufferManager.cs:20–148.
- FFmpegService dead: default executable "ffmpeg", old rawvideo pipe/segment process; NVENC quality "-preset p4 -tune ll -rc vbr -cq 28 -b:v 20M", AMF "-quality speed -rc vbr_peak -b:v 20M", QSV "-preset veryfast -global_quality 28 -b:v 20M", x264/default "-preset ultrafast -crf 23"; yuv420p, reset_timestamps1; concat list text and -c copy; thumbnail at1s scaled320:-1; ffprobe duration; StopEncoder sends 'q', waits5s, kill tree; FindFFmpegExecutable searches PATH/common/Downloads/WinGet locations. None of these options/searches applies to the native engine, which binds its own DLLs and only uses ffmpeg.exe for incompatible carry stitching. FFmpegService.cs:35–49,64–88,102–189,197–238,245–286,296–379.

## 5. Persistence & formats

### Replay paths and files

The live rolling buffers/carry are RAM-only packet collections; they have no disk persistence, schema version, migration, recovery file, segment cache or native settings file. Start receives a complete caller-owned snapshot. The normal save path creates LibraryPath and writes "{DateTime.Now:yyyy-MM-dd_HH-mm-ss}-Replay.{normalizedExtension}". Extension trim/lowercase maps only mkv→mkv,mov→mov; every other value, including avi and blank,→mp4. Missing/null UI library path maps to empty string; the engine does not choose a default directory. Same-second names are not made unique; no collision check, reservation, atomic rename or cleanup of partial mux outputs. VfrRecorderAdapter.cs:62–67; VfrReplayEngine.cs:800–805,939–945; PacketRingBuffer.cs:20–28; CarryOverSegment.cs:27–41.

Direct save writes one video stream followed by nonempty audio tracks in order supplied. Live separate mode: video stream0; audio All(default),System,Microphone when mic exists. Single mode: video,Audio(default). Placeholder/empty audio tracks are omitted rather than written empty. The first included audio stream has disposition AV_DISPOSITION_DEFAULT; every later audio disposition is assigned0. Titles set metadata key "title" only for nonempty strings. No language, handler_name, game tag, encoder-friendly name or replay metadata is written by this muxer. Game tags are a separate caller sidecar action. Mp4Muxer.cs:53–65,97–103; VfrReplayEngine.cs:699–766,852–857; MainViewModel.cs:201–212.

StreamSpec is the exact shared schema: CodecId:AVCodecID,IsVideo:bool,Width:int,Height:int,SampleRate:int,Channels:int,ExtraData:byte[]. It contains no bitrate, color fields, profile, framerate, duration or packet side-data. EncodedPacket contains Data:byte[],PtsUs:long,DtsUs:long,IsKeyframe:bool only; copied packet records share the payload array. CarryAudioTrack contains Spec,List<EncodedPacket>,Title; CarryOverSegment adds video/audio lists,WasGame,TargetName,TargetExe. Mp4Muxer.cs:8–15,30; NvencVfrEncoder.cs:8–10; CarryOverSegment.cs:7,27–41.

### Mux algorithm and metadata

Mp4Muxer.Write first rejects videoPackets.Count==0 with "[Mp4Muxer] no video packets — nothing to save.", returns false. Container name based on case-insensitive path suffix: .mkv→"matroska",.mov→"mov",otherwise"mp4"; avformat_alloc_output_context2(formatName,path). Add video stream then audio streams; avio_open(AVIO_FLAG_WRITE), avformat_write_header(fmt,null), merge packets, av_write_trailer; close pb/free format in finally. No muxer dictionary, no "+faststart", no fragmented-MP4 flags, no explicit video timescale option. Catch exception logs "[Mp4Muxer] error: {message}" and false; allocation/avio/header negative results log their own errors and false. Mp4Muxer.cs:37–94.

AddStream avformat_new_stream(fmt,null), time_base=1/1_000_000; codecpar.codec_id=spec.CodecId. Video sets codec_type=VIDEO,width,height. Audio sets codec_type=AUDIO,sample_rate,default channel layout(channels). Deliberately no assignment of codecpar.format. If extradata nonempty, av_malloc(extra.Length+AV_INPUT_BUFFER_PADDING_SIZE), copy only length bytes, set extradata_size; padding memory is allocated but NOT explicitly zeroed. No bit_rate or color-space/range/primaries/TRC fields are assigned to codecpar. Encoder video color tags are set in AVCodecContext and potentially codec headers, but the muxer does NOT propagate them into a colr box through codecpar. Do not claim independent MP4 color tags that source never writes. Mp4Muxer.cs:105–136; NvencVfrEncoder.cs:155–158.

MergeAll copies video entries first and all audio entries next, then List.Sort by PTS only; equal-PTS order has no stable tie-break or DTS ordering promise. Each WritePacket allocates packet, av_new_packet(Data.Length), copies payload, sets stream_index,pts,dts,key flag if needed; does NOT set duration; av_packet_rescale_ts from microsecond base into ACTUAL stream.time_base after write_header (AAC often changed to1/sampleRate); av_interleaved_write_frame; free packet in finally. Return codes for av_new_packet, interleaved write and trailer are not checked, so Write can report true and ReplaySaved after a low-level packet/trailer failure. MP4 normally retains trailing moov; same no-faststart behavior for MOV. Mp4Muxer.cs:73–80,139–169.

All AAC stream specs say48k stereo; source bitrate itself is not transferred in StreamSpec/codecpar, and the muxed measured bitrate depends on packet content. The All192 versus source160 encoding request is deliberate; verify actual Windows track selection using MF probe rather than relying solely on disposition. AacAudioEncoder.cs:52–58,323–333; VfrReplayEngine.cs:672–718; PlaybackTrackProbe.cs:14–44.

### Other reads/writes

| Artifact | Exact location/format/lifetime |
| --- | --- |
| Incompatible stitch inputs | %TEMP%\Lag-stitch-{Guid.NewGuid().ToString("N").Substring(0,8)}\head.mp4 and live.mp4, muxed from original packet snapshots; directory best-effort recursively deleted in finally, no startup scavenger. ReplayStitcher.cs:79–94,143–146. |
| Stitch fallback carried half | Primary replay stem+"-Game"+original extension; independently raises ReplaySaved if written. VfrReplayEngine.cs:919–924. |
| Screenshot | outputDir\Screenshot_{local yyyy-MM-dd_HH-mm-ss}.png; full selected/primary physical monitor dimensions, GDI PNG, no sidecar in this service; same-second path can collide. ScreenshotService.cs:45–61. |
| FFmpeg runtime | Absolute BaseDirectory\ffmpeg directory, shared FFmpeg7.1 DLLs; no system installation discovery. App.axaml.cs:34–50; FfmpegInterop.cs:8–11,34–49. |
| Stitch executable | BaseDirectory\ffmpeg\ffmpeg.exe; not FFmpegService.FFmpegPath. ReplayStitcher.cs:72–77. |
| RNNoise model | BaseDirectory\rnnoise\std.rnnn, model read by arnndn, no persisted filter state. MicDenoiser.cs:33–36,103–117,153–157. |
| Steam signal | Read-only registry HKCU\Software\Valve\Steam, value RunningAppID, nonzero signed int; no writes. GameDetector.cs:239–249. |
| Executable description/icon | Process image path via QueryFullProcessImageNameW; FileVersionInfo and associated icon→in-memory PNG only. AudioSessionService.cs:59–69,86–111. |
| GC packet-size fixture | Optional LAG_GCTEST_PACKETS path, comma-separated ffprobe rows size,flags; positive integer first field, second contains 'K'→key, malformed/nonpositive lines skipped; no header/schema/version. GcSelfTest.cs:130–140. |
| Headless capture output | LAG_VFR_OUT or Path.GetTempPath()\lag_vfr_test; normal replay path syntax inside it. VfrEngineTest.cs:37–38,60–69; VfrReplayEngine.cs:945. |
| Diagnostic log | Actual FileLog path is Documents\Lag\Logs\Lag-{local yyyy-MM-dd_HH-mm-ss}.txt (Program's AppData comment is stale), UTF8 writer, overwrite, AutoFlush=true, keep15 old matching logs before creating new, console/error tee. Each WriteLine gets prefix "[HH:mm:ss.fff] "; an embedded newline in the stall report stays embedded in that one string. FileLog.cs:24–59,78–91,139–146. |
| Crash log | Documents\Lag\Logs\crash.log, append across launches; native/managed crash blocks include local date/version. Program installs handlers only after headless dispatches. FileLog.cs:29–34,110–136; Program.cs:137–146. |
| Clip game metadata | Engine only exposes LastSavedGameTag. MainViewModel ReplaySaved writes ClipMetadata{Game=carry.Name or current game Name,Exe=carry.Exe or current.Exe} through ClipMetadataStore before library refresh. Sidecar schema belongs to library spec. MainViewModel.cs:196–215. |

The legacy segment_*.mp4 files, concat_*.txt, Replay_*.mp4 and JPG thumbnail flow are dead and are not outputs of the live engine. RecordingEngine.cs:191–245; FFmpegService.cs:109–151.

## 6. Native/external dependencies

Use the same bundled FFmpeg7.1 ABI, not a system FFmpeg or reimplemented media bitstream. The C# project targets net8.0-windows10.0.26100.0, SupportedOSPlatformVersion=10.0.19041.0; comments saying compile22621/run1903 do not override these actual project values. It bundles ffmpeg/** and rnnoise/** with PreserveNewest; relevant packages NAudio.Core/Wasapi2.3.0, SharpDX.DXGI/Direct3D11/Direct2D1 4.2.0, FFmpeg.AutoGen7.0.0, SharpHook5.3.9, System.Drawing.Common8.0.10. Rust should replace managed wrappers with windows-rs and bindings matching the shipped C ABI, preserving the same formats/options. InstantReplay.csproj:5–15,35–44,72–73,86–102.

### Binding initialization

App initialization makes ffmpegDir=GetFullPath(BaseDirectory/"ffmpeg"), sets Environment.CurrentDirectory, SetDllDirectory, AddDllDirectory, prepends PATH, configures FFMpegCore.BinaryFolder, then FfmpegInterop.Initialize BEFORE constructing settings/probing codecs. Normal app does not call SetDefaultDllDirectories here; headless VfrEngineTest additionally calls SetDefaultDllDirectories(0x1000 LOAD_LIBRARY_SEARCH_DEFAULT_DIRS). App.axaml.cs:32–50; VfrEngineTest.cs:24–32.

FfmpegInterop.Initialize locks static Gate, returns cached Available if already attempted, sets initialized=true BEFORE binding so failures cannot retry in-process. Sets ffmpeg.RootPath, DynamicallyLoadedBindings.Initialize(), touches avcodec_version; major=version>>16 must be61, otherwise InitError="libavcodec major {major} != expected 61 (FFmpeg 7.x ABI)." and false. Require avcodec_find_encoder_by_name("libx264")!=null as universal sentinel; absence InitError="libx264 encoder not present in the bundled avcodec.". Success sets Available=true, error string remains null; exception stores ex.Message and logs native bind failure. Encoder candidates are probed separately. Err(code) stackalloc1024, av_strerror, returns "{ANSI error text} ({code})". FfmpegInterop.cs:20–102.

Runtime DLL families used directly: avcodec-61 (video/AAC),avformat-61 (containers),avutil-59 (frames, packets/buffers, options, channel layouts, FIFO),swresample-5 (audio),avfilter-10 (arnndn); external ffmpeg.exe also uses its bundled dependencies for stitch scale/pad/concat. The checked major sentinel is libavcodec only; source does not explicitly validate all other DLL majors. FfmpegInterop.cs:8–11,43–78; MicDenoiser.cs:9–12; ReplayStitcher.cs:72–115.

### Windows/native mapping

| C# dependency/API | Purpose and Rust equivalent suggestion |
| --- | --- |
| SharpDX D3D11 Device,DeviceContext,Multithread | windows-rs D3D11CreateDevice on chosen IDXGIAdapter/default hardware, ID3D11Device/ID3D11DeviceContext and ID3D11Multithread.SetMultithreadProtected. Preserve BGRA_SUPPORT|VIDEO_SUPPORT and context ownership. D3D11Context.cs:33–66. |
| SharpDX video processor | windows-rs ID3D11VideoDevice/VideoContext, processor enumerator/input/output views, BGRA input copy/single NV12 scratch/staging resources; preserve enum/color-bitfield values. Bgra2Nv12Converter.cs:51–115,195–273. |
| WinRT Windows.Graphics.Capture | windows-rs GraphicsCaptureItem interop/CreateForWindow/CreateForMonitor, CreateDirect3D11DeviceFromDXGIDevice, Direct3D11CaptureFramePool.CreateFreeThreaded, GraphicsCaptureSession, access/border/cursor/update interval. Surface IID interop as section4. WgcCaptureSource.cs:71–195,213–273. |
| Win32 user32 monitor/window APIs | EnumWindows,GetForegroundWindow,IsWindowVisible,IsIconic,GetWindowThreadProcessId,GetWindowRect,MonitorFromWindow,EnumDisplayMonitors,GetMonitorInfoW,GetWindowTextW,GetClassNameW,IsWindow; windows-rs equivalents with physical monitor rectangles/device names. GameDetector.cs:254–276; VfrReplayEngine.cs:1158–1176; ScreenshotService.cs:28–34. |
| Stopwatch/QPC | QueryPerformanceCounter/Frequency shared session clock, preserving integer microsecond arithmetic and pause accumulation; WGC raw timestamp is for diagnostics only. VfrReplayEngine.cs:658–665; WgcTimestampProbe.cs:69–75. |
| NAudio WASAPI | windows-rs MMDeviceEnumerator/IMMDeviceEnumerator, default Render Multimedia/Capture Communications, IAudioClient/IAudioCaptureClient for endpoint/mic; same event-thread shape and callback formats. Whole-system path's device mix format is dynamic. WasapiAudioSource.cs:105–149. |
| Custom process WASAPI COM | windows-rs ActivateAudioInterfaceAsync on VAD\Process_Loopback, IActivateAudioInterfaceCompletionHandler, PROPVARIANT BLOB, IAudioClient/IAudioCaptureClient; CoInitializeEx MTA per worker and release COM there. IAudioClient IID1CB9AD4C-DBFA-4c32-B178-C2F568A703B2; IAudioCaptureClient IIDC8ADBD64-E71E-48a0-A4DE-185C395CD317; async operation IID72A22D78-CDE4-431D-B8CC-843A71199B6D. ProcessLoopbackCapture.cs:163–190,216–310. |
| Kernel events/threads | CreateEventW,SetEvent,WaitForSingleObject,CloseHandle; preserve100ms wait,5s activation,6s ready,2s teardown limits. ProcessLoopbackCapture.cs:63–69,123–125,140,183–184,207–211. |
| IMMNotificationClient | windows-rs endpoint notification registration; no custom UI event loop in engine. AudioDeviceWatcher.cs:14–39. |
| WASAPI session enumeration | IAudioSessionManager/Sessions equivalents, active state/PID; OpenProcess query-limited,QueryFullProcessImageNameW; executable version/icon→PNG via Windows equivalents. AudioSessionService.cs:37–81,86–123. |
| PDH | windows-rs PdhOpenQueryW/PdhAddEnglishCounterW/PdhCollectQueryData/PdhGetFormattedCounterArrayW/PdhCloseQuery; exact wildcard/flags as above, x64 struct alignment. GpuUsageProbe.cs:26–60,65–117; SystemStatsSampler.cs:79–107. |
| Resource stats | GetSystemTimes,GlobalMemoryStatusEx; no per-game CPU metric. SystemStatsSampler.cs:54–74,139–157. |
| WinRT webcam | DeviceInformation,MediaCapture,MediaFrameReader,SoftwareBitmap; CPU memory, real-time latest frame, BGRA8 premultiplied. WebcamCaptureSource.cs:37–146. |
| Direct2D/DirectWrite | windows-rs D2D1 Factory1/Device/DeviceContext/Bitmap1/rounded geometry/layers/brushes and DWrite shared factory/text formats/layout; no shader-based replacement needed. OverlayCompositor.cs:91–100,183–238,242–406. |
| SharpHook | Reuse the application's single global input hook; Windows input hook can feed equivalent label/down/up events, no second mouse-movement hook for overlay. GlobalHotkeyManager.cs:80–87,95–116,153–161; KeystrokeTracker.cs:8–13. |
| GDI screenshot | Desktop DC BitBlt equivalent to Graphics.CopyFromScreen and PNG encoder equivalent to System.Drawing; use selected monitor physical rcMonitor. ScreenshotService.cs:45–59. |
| Windows registry | windows-rs registry query for HKCU Steam RunningAppID. GameDetector.cs:242–249. |
| Windows media probe | WinRT MediaSource,MediaPlaybackItem.AudioTracks; no VLC dependency for this probe. PlaybackTrackProbe.cs:22–40. |
| Process FFmpeg stitch | std::process child with hidden Windows window and argument construction/parsing compatible with exact source command. ReplayStitcher.cs:110–188. |

PerformanceGuard.Apply is called once in normal app before resolving MainViewModel. SetProcessInformation(currentProcess,ProcessPowerThrottling class4,stateSize) with Version=1,ControlMask=EXECUTION_SPEED0x1,StateMask=0 to opt out EcoQoS. Boolean return is ignored; only exceptions log failure. Then D3DKMTSetProcessSchedulingPriorityClass(gdi32) tries5 Realtime,4 High,3 AboveNormal in order, success iff NTSTATUS==0, logs chosen class and returns; all denied log unchanged; exceptions log. CPU priority is deliberately not changed by this helper, but actual capture StartSession raises it High and teardown Normal. No automatic elevation request. PerformanceGuard.cs:18–44,47–84; App.axaml.cs:213–216; VfrReplayEngine.cs:435–439,1118–1125.

Process-wide timeBeginPeriod(1) is called in Program before any diagnostic dispatch and held for process lifetime (no paired timeEndPeriod). Capturing sets .NET SustainedLowLatency GC; teardown Interactive rather than restoring previous GC mode. Rust has no .NET GC; equivalent acceptance is avoiding long capture-path allocator/ownership stalls while preserving diagnostics meaning where possible. Do not introduce a forced no-GC region: source does not use one. Program.cs:65–68; VfrReplayEngine.cs:436–439,1118–1119.

## 7. Cross-area interfaces

| Caller/callee boundary | Contract |
| --- | --- |
| App→engine | Singleton VfrRecorderAdapter, singleton IReplayRecorder alias; native binding before codec/settings discovery. App.axaml.cs:46–50,353–359. |
| MainViewModel→recorder | Builds fresh RecorderOptions from selected monitor/GPU/video/audio/overlay settings; disabled mic="",percent mic/app volume divided100,only enabled app tuples. Width/output width included but native adapter ignores them; engine calculates window-specific aspect from OutputHeight. Start Task.Run; stop Task.Run; next start awaits teardown. MainViewModel.cs:465–547,583–588; VfrRecorderAdapter.cs:60–105. |
| UI mode mapping | Normal adapter never sets engine.Mode, always AutoGame; direct Display/ForcedWindow reachable from test/client only. Normal UI also never sets Preset, so4. VfrRecorderAdapter.cs:60–105; VfrReplayEngine.cs:31–35,53. |
| Recorder→UI | TargetChanged forwarded; UI explicitly posts UpdateCaptureTarget to dispatcher. ReplaySaved forwards on save caller's thread; MainViewModel event directly changes save/status, tags sidecar, refreshes library, shows toast. Adapter adds no UI marshaling. VfrRecorderAdapter.cs:26–29; MainViewModel.cs:196–227. |
| Save metadata | LastSavedGameTag supplies game from carried head when current target desktop; fallback separate saves share the property at event time. Null for pure live saves after property reset. VfrReplayEngine.cs:868,887,924–935; MainViewModel.cs:204–212. |
| Hotkey→mic | SetMicMuted acts immediately on live _audio or no-op; MicStartMuted supports PTT. GlobalHotkeyManager raises PttPressed/PttReleased on configured key without codec restart; tracker simultaneously receives all supported hook key/button events. IReplayRecorder.cs:33–43; VfrReplayEngine.cs:974–979; GlobalHotkeyManager.cs:139–161. |
| Pause→UI | SetPaused no-op while idle; caller's TogglePauseRecording requires recording and stops/restarts its own buffer ticker/status. Engine keeps native sessions alive. MainViewModel.cs:615–632; VfrReplayEngine.cs:986–1005. |
| Save feedback | UI command refuses if idle/already saving, begins toast/chime immediately and runs save Task.Run; completion ReplaySaved silent toast. 20s watchdog exists in caller because false/null save emits no completion. MainViewModel.cs:635–705. |
| Live FPS HUD | FramesCaptured cumulative unpaused WGC count feeds StatsHudWindow, distinct from encoded FPS; baked stats option left unset by normal UI. MainViewModel.cs:440–452,538–539; VfrReplayEngine.cs:491,529. |
| Audio app picker | AudioSessionService enumeration Task.Run; same GameDetector.RunningGameExes filters live game exes; UI merges names/icons and keeps disappeared enabled selections, drops disappeared disabled rows. No shared detector state with engine instance. SettingsViewModel.cs:800–848. |
| Audio devices→settings | DeviceWatcher events→UI post→300ms debounce→RefreshMicrophones/RefreshOutputDevices→restart meter; no automatic active recorder reset. SettingsViewModel.AudioMeter.cs:27–67. |
| Noise helpers→settings monitor | NoiseGate and MicDenoiser are reusable helpers; settings owns independent monitor instances and live gate/denoise changes; recorder receives snapshot at cold start. WasapiAudioSource.cs:31–38; MicDenoiser.cs:16–17; SettingsViewModel.AudioMeter.cs:102–116. |
| Screenshot command→service | Selected monitor DeviceName and LibraryPath captured on UI thread then Task.Run ScreenshotService.Capture; success toast + screenshot sound; independent of recording state. MainViewModel.cs:414–428. |
| Overlay→webcam/input/stats | Camera owns CPU latest frame; compositor owns GPU upload/target/D2D brushes/text; singleton tracker fed by existing hook; stats sampler independent timer. Dispose disables tracker globally, disposes stats/text/webcam/brush/bitmap/target/factories. OverlayCompositor.cs:102–121,166–180,409–431. |
| Source→video encoder | Texture borrowed only inside WGC callback; converter/encode must synchronously consume it. Never queue or retain pool texture beyond callback. WgcCaptureSource.cs:26–28,150–162. |
| Encoder→ring→mux | Encoders emit private payload copies; rings/snapshots hold these after encoder teardown; StreamSpec extradata must be copied before context free. NvencVfrEncoder.cs:357–370,410–420; VfrReplayEngine.cs:1103–1115. |

No current recording path calls dead FFmpegService, RecordingEngine, WindowsCaptureService, IScreenCaptureService or RingBufferManager. The editor's FFMpegCore use and playback's VLC/mpv replacement are separate; a similar FFmpeg executable name does not make the legacy service live. App.axaml.cs:44,353–359; ReplayStitcher.cs:72.

## 8. Self-tests / diagnostics

No diagnostics or builds were executed during extraction. Expected outputs below are derived from source; live counts/timings/device strings and floating-point statistics are observations, not guaranteed numeric goldens. Literal prefixes, property names, option defaults, pass conditions and formatting are the acceptance target.

### Dispatch and complete environment inventory

Program.Main is STA. It initializes FileLog, requests1ms timer period, then selects FIRST matching headless entry in this exact order and returns after it: LAG_VFR_TEST=="1"; nonempty LAG_MFPROBE; LAG_WGCPROBE=="1"; LAG_DUPPROBE=="1"; LAG_STALLTEST=="1"; LAG_GCTEST=="1"; LAG_CADENCETEST=="1"; LAG_CARRYTEST=="1"; int.TryParse(LAG_PROCTEST). These bypass UI, single-instance mutex, PerformanceGuard.Apply and crash-handler installation. Boolean flags require exact "1", not true/yes. Program.cs:58–146,157–175.

| Variable | Parsing/default/effect |
| --- | --- |
| LAG_VFR_TEST | Exact1 runs VfrEngineTest. Program.cs:71–74. |
| LAG_VFR_OUT | Null→%TEMP%\lag_vfr_test; empty string is used, not defaulted. VfrEngineTest.cs:37–38. |
| LAG_VFR_PID | int.TryParse; Process.GetProcessById.MainWindowHandle forced target; failures log; no parse→no forced window. VfrEngineTest.cs:40–51. |
| LAG_VFR_DESKTOP | Exact1 makes Mode.Display and clears forced HWND, overriding PID. VfrEngineTest.cs:53–56,71–72. |
| LAG_VFR_CODEC | Raw string passed to EncoderSelector; null/empty auto. VfrEngineTest.cs:64. |
| LAG_VFR_BITRATE | int.TryParse else30000; <=0 Auto. VfrEngineTest.cs:65; NvencVfrEncoder.cs:85–86. |
| LAG_VFR_FORMAT | Null→"mp4"; normalize at save. VfrEngineTest.cs:66; VfrReplayEngine.cs:800–805. |
| LAG_VFR_HEIGHT | int.TryParse else1080;<=0 native/no downscale. VfrEngineTest.cs:67; VfrReplayEngine.cs:394. |
| LAG_VFR_MAXFPS | int.TryParse else120;<=0 grid off, encoder hint120, no MinUpdateInterval setter. VfrEngineTest.cs:68; VfrReplayEngine.cs:251,408; WgcCaptureSource.cs:121. |
| LAG_VFR_PRESET | int.TryParse else4; NVENC clamps1..7. VfrEngineTest.cs:69; NvencVfrEncoder.cs:211. |
| LAG_VFR_DUPONLY | Exact1 sets DuplicationOnly but active engine never reads it; NO capture-path change. VfrEngineTest.cs:73; VfrReplayEngine.cs:54,372,387. |
| LAG_VFR_SEPAUDIO | Exact1 enables All/System/Microphone layout. VfrEngineTest.cs:74. |
| LAG_VFR_NOAUDIO | Exact1 disables all audio. VfrEngineTest.cs:75. |
| LAG_VFR_APPS | Nonempty string enables requested apps mode; comma split RemoveEmptyEntries|TrimEntries, every tuple gain1; game audio defaults enabled too. VfrEngineTest.cs:76–80. |
| LAG_VFR_WEBCAM | Exact1 enables default camera PiP with normal defaults. VfrEngineTest.cs:81–84. |
| LAG_VFR_KEYS | Exact1 enables keys and synthetic feed. VfrEngineTest.cs:84,90–101. |
| LAG_VFR_STATS | Exact1 enables baked stats with test's StatsDetail=2. VfrEngineTest.cs:85–86. |
| LAG_VFR_SECONDS | int.TryParse else15; seconds<0 makes buffering loop empty, no clamp. VfrEngineTest.cs:89–92. |
| LAG_VFR_CARRY | Exact1 simulates loss after buffering then waits6s and saves both sessions. VfrEngineTest.cs:110–123. |
| LAG_VFR_DIAG | Static readonly flag at Bgra2Nv12Converter type initialization, exact1 enables expensive green probes on hardware ConvertInto only. Bgra2Nv12Converter.cs:46,101. |
| LAG_MFPROBE | Nonempty→split ';' RemoveEmptyEntries, no trim; probe file URIs. PlaybackTrackProbe.cs:16–23. |
| LAG_WGCPROBE | Exact1 timestamp research on foreground window. Program.cs:87–90. |
| LAG_WGCPROBE_SECONDS | int.TryParse else10, no positive clamp. WgcTimestampProbe.cs:33. |
| LAG_WGCPROBE_FPS | int.TryParse else165. WgcTimestampProbe.cs:34. |
| LAG_DUPPROBE | Exact1 raw desktop-duplication counting (no encoding). Program.cs:95–98. |
| LAG_DUPPROBE_OUTPUT | int.TryParse else0, no range clamp. DuplicationProbe.cs:29. |
| LAG_DUPPROBE_SECONDS | int.TryParse else8. DuplicationProbe.cs:28. |
| LAG_STALLTEST | Exact1 pure stall attribution tests. Program.cs:102–105. |
| LAG_GCTEST | Exact1 real ring allocation/GC workload. Program.cs:110–113. |
| LAG_GCTEST_PACKETS | Optional CSV fixture path; missing/invalid-empty fixture→synthetic distribution. GcSelfTest.cs:37,130–140. |
| LAG_GCTEST_MODE | OrdinalIgnoreCase "interactive"→Interactive, otherwise SustainedLowLatency; restore old mode in finally. GcSelfTest.cs:43–50. |
| LAG_CADENCETEST | Exact1 deterministic synthetic cadence check. Program.cs:117–120. |
| LAG_CARRYTEST | Exact1 pure carry bookkeeping checks. Program.cs:124–127. |
| LAG_PROCTEST | Any parseable int activates process-loopback selftest, cast to uint without validation (including negative). Program.cs:131–134; ProcessLoopbackCapture.cs:38–42. |

These are all LAG_* strings found in handwritten source. LAG_VFR_DIAG modifies normal capture as well as tests; all other diagnostic options are read only in the entry/helper paths indicated.

### VFR end-to-end test

Prepend bundled DLL search/PATH as section6, print:

    [VfrTest] WGC supported: {bool}
    [VfrTest] Engine available: {bool}
    [VfrTest] Forced target pid {pid} '{ProcessName}' hwnd 0x{forced:X}
    [VfrTest] forced pid lookup failed: {message}

The last two are conditional alternatives. Availability bool is printed but not used to abort. Start engine with buffer30s, bitrate30000,formatmp4,targetHeight1080,maxFps120,preset4,cursortrue by default; audio enabled, default mic, no split track unless flagged. Test does not use adapter or normal UI codec mapping. VfrEngineTest.cs:24–87.

Print "[VfrTest] Buffering for {seconds}s (start/keep a game in the foreground)...". Each iteration, if fake keys: W down always; LeftShift down when i%2==0 else up; LMB down when i%3!=0 else up; sleep1000ms. Every fifth iteration prints "[VfrTest] ...{i+1}s, captured={FramesCaptured}, encoded={FramesEncoded}, buffered={BufferedSeconds:F1}s, convert={ConvertMs:F1}ms encode={EncodeMs:F1}ms". If carry flag print simulation line, call SimulateTargetLoss (does not change ForcedWindow/Mode/detection, so it may reacquire SAME game rather than desktop), print carried span, sleep/print each of6 post-switch seconds. Finally:

    [VfrTest] Saving replay...
    [VfrTest] SAVED: {path}
    [VfrTest] DONE → {path}

SAVED is ReplaySaved event and may appear twice on separate-half fallback. Failure tail instead "[VfrTest] SaveReplay returned null (no game locked?)." (message is stale: desktop also records). Explicit Stop then using-dispose can trigger another empty teardown log. Acceptance: playable chosen-container clip with expected enabled sources/overlays and, in carry rehearsal, earlier footage available after simulated loss. No PASS boolean is emitted. VfrEngineTest.cs:89–130.

### Cadence self-test

LAG_CADENCETEST uses System.Random(seed1234),3000 frames,nominal165fps,jitter ±1500us: t starts0 double, increment1e6/165+(NextDouble()*2-1)*1500; every400th frame (i%400==399) add45000; append (long)t. Compare raw intervals to real CadenceGrid(120), count Drop. Intervals include only positive differences ms. Median=sorted[count/2] (upper middle), standard deviation population, distinct=Math.Round(ms,2) (.NET to-even), within10%=|x-med|<=med*.10. Success distinct<=5 AND within>=85%. CadenceSelfTest.cs:21–39,46–102.

Expected literal output/format, numeric measurements generated by that sequence:

    [CadenceTest] stabiliser check — synthetic 165fps source, 120fps cap

      RAW present times (what we wrote before)
        median {med:F2}ms (= {1000/med:F1} fps) | stdev {sd:F2}ms | max {max:F2}ms
        distinct intervals {distinct,5} | within +/-10% of median {within:F1}%
      SNAPPED to the 120fps grid
        median {med:F2}ms (= {1000/med:F1} fps) | stdev {sd:F2}ms | max {max:F2}ms
        distinct intervals {distinct,5} | within +/-10% of median {within:F1}%
      frames dropped into filled slots: {dropped} of 3000

    [CadenceTest] distinct intervals {distinct} (target <= 5, Medal = 3)
    [CadenceTest] within +/-10% of median {within:F1}% (target >= 85%, Medal = 92.4%)
    [CadenceTest] PASSED

On violated threshold prints FAILED instead. No nonzero process exit is set by this test. CadenceSelfTest.cs:29–39,105–114.

### Carry-over self-test

LAG_CARRYTEST does no native/GPU calls. Synthetic video step=1_000_000/fps integer,16-byte payload; optional AAC50Hz/8-byte payload, fake specs H2641920x1080 extradata[1,2,3],AAC48k2ch[1,2],tag TestGame/test.exe. Expected successful output (dynamic duration rounding shown):

    [CarryTest] carry-over stitching checks
      [PASS] short tail is returned untouched
      [PASS] trim returned a segment
      [PASS] trim opens on a keyframe
      [PASS] trim keeps >= 3s (got 3.98s)
      [PASS] trim keeps <= 5s (got 3.98s)
      [PASS] original is unchanged
      [PASS] keyframe-less tail is dropped
      [PASS] zero window is dropped
      [PASS] trimmed segment has audio
      [PASS] no audio packet predates the video start
      [PASS] re-based video starts at 0
      [PASS] re-based video keeps its length
      [PASS] re-based audio starts at/after 0
      [PASS] shift offsets pts and dts alike
      [PASS] same params concat directly
      [PASS] different size needs a re-encode
      [PASS] different extradata needs a re-encode
      [PASS] same audio config concats
      [PASS] different sample rate does not
      [PASS] different channel count does not
    [CarryTest] ALL PASSED

Locale can use comma instead of decimal point for interpolated :0.00. Check cases:1s tail ask5 same reference;10s60fps keyevery120 ask3 keeps >=3<=5 and original600 untouched; no keys/zero window→null; trimmed audio no PTS before video; 7s-offset120frames rebase duration error<2us; Shift+5_000_000 affects both PTS/DTS; video dimension/extradata and audio rate/channels compatibility. Failures change individual PASS→FAIL and final "[CarryTest] {n} CHECK(S) FAILED"; static failed counter not reset by Run. CarryOverSelfTest.cs:16–107,119–147.

### Stall attribution and exact live log

Default threshold25_000us, recent history8 frames, max queued64 events, max shown6 per drain. Monitor reset clears recent,write index,previous-present and GC-base flag, but not queued events/counters. FrameArrived gets GC total pause ticks, idleGC=max(0,(gcNow-lastFrameEndGC)/10) if base exists, records current QPC/GC start; gap=present-prev; classify only STRICTLY >threshold. Engine passes arrival time for both arguments. FrameDone measures total with (long)(deltaQpc*(1e6/frequency double)); stores stage durations,GcUs=max(0,(gcEnd-gcStart)/10),IdleGcUs; advance circular slot, save GC end. CaptureStallMonitor.cs:32–78,82–117; VfrReplayEngine.cs:505–506.

Work.OtherUs=max(0,TotalUs-OverlayUs-ConvertUs-EncodeUs-RetuneUs); BlockingUs=TotalUs+IdleGcUs (in-frame GC is already within Total). Classify chooses largest BlockingUs of previous8, strict > tie; if current idleGC>worst.BlockingUs choose Work{IdleGcUs=current}. Ours iff worst.BlockingUs*2>=gapUs; else Source. Dominant first returns gc if (GcUs+IdleGcUs)*2>=BlockingUs; otherwise start other, compare overlay,convert,encode,retune in that order using strict > (ties keep earlier). Increment atomic source/ours even if queue full; queue includes GapUs,AtUs,UTC wall ticks,Ours,Cause,Worst. AtUs is stored but not printed. CaptureStallMonitor.cs:55–68,120–151.

Drain exchanges counters to0; if zero, empties stale queue and returns null. Otherwise summary followed by first6 queue events; each suppressed QUEUED event increments more; events discarded when64-full are only represented in summary counts, not "...more". Time local HH:mm:ss.fff. Exact output format:

    [stall] {total} gap(s) >{thresholdUs/1000}ms: {src} SOURCE (game/DWM presented nothing), {ours} OURS
        {gapUs/1000.0,6:F1}ms gap at {HH:mm:ss.fff} -> OURS: {cause} | frame {totalMs:F1}ms = overlay {overlayMs:F1} + convert {convertMs:F1} + encode {encodeMs:F1} + retune {retuneMs:F1} + other {otherMs:F1} | gc in-frame {gcMs:F1} idle {idleGcMs:F1}
        {gapUs/1000.0,6:F1}ms gap at {HH:mm:ss.fff} -> SOURCE (heaviest recent frame of ours: {blockingMs:F1}ms)
        ... and {more} more

Detail alternatives are independent events, with exactly four leading spaces then width6 gap number; final more line only when>0. CaptureStallMonitor.cs:156–184.

Engine LogCaptureStats called on lock timer, Monitor.TryEnter nonreentrant; encoder null clears diagnostic baseline. New encoder reference seeds timing/capture counters, clears collisions/drains stalls then returns. Otherwise secs=(EngineNowUs-windowStart)/1e6; secs<1 return; df=TimedFrames-baseline,dcap=FramesCaptured-baseline,drops=TakeCollisions; if df>0 log:

    [capture] encoded {df/secs:F0} fps (wgc {dcap/secs:F0}) | convert {convertDelta/1000.0/df:F1}ms + encode {encodeDelta/1000.0/df:F1}ms/frame | preset p{Preset} | grid {grid.Fps:F0}fps, {drops} off-slot

Then refresh baselines and drain/log stalls even if df0. Normal per-frame routine avoids synchronous capture stats/stall disk logs; other diagnostics/errors/retune still can log from capture. VfrReplayEngine.cs:558–599.

LAG_STALLTEST Step6061us. Feed ten cheap frames then +48000 gap→source; ten cheap, one actual sleep45ms declaring encode45000us, two cheap frames then +40000 gap→ours encode;5 cheap, sleep40ms declaring overlay40000 then46000 gap→ours overlay;500 ordinary1/2-step frames no report; reset between0 and5_000_000 produces no gap. Expected:

    [StallTest] stall attribution checks
      [PASS] a game hitch is reported
      [PASS] ...as SOURCE
      [PASS] our stall is reported
      [PASS] ...as OURS
      [PASS] ...naming the encoder
      [PASS] a slow overlay is named
      [PASS] ordinary 1-2 step cadence is not a stall
      [PASS] a gap across a reset is not a stall
    [StallTest] ALL PASSED

It actually sleeps for attribution timings, so extreme scheduler/GC interference can affect result; no process failure code set; failed static counter accumulates, final alternative "[StallTest] {n} CHECK(S) FAILED". StallMonitorSelfTest.cs:19–104.

### GC stress test

Defaults: fps165,buffer120s,sim240s,speed2.0 (about120 real seconds),3 audio tracks,47 audio packets/s,434 bytes each. Fixture optional CSV as section5; synthetic Random7 sequence60*165 entries,keyevery330: key size520000+Next(200000),otherwise(int)(12000*exp(NextDouble()*2.9)). For39600 video frames, cycle sequence; pts=i*1e6/165; allocate byte[size], BlockCopy min(size,1<<20) from reusable1MiB source, real PacketRingBuffer.Add. Measure just alloc+copy+Add against QPC and GC pause before/after; retain spikes>5ms. Audio accumulator+=47/165.0,while>=1 emit3 all-key AAC-size fake packets at current pts. Snapshot video every30 simulated sec when i>0. Pace Thread.Sleep((int)(targetMs-wallElapsed)) if>0. GcSelfTest.cs:27–33,53–102,145–156.

Output has no PASS/FAIL golden. Expected header/shape:

    [GcTest] {seqCount} packet sizes loaded; simulating 240s at 165fps (2x speed), buffer 120s, 3 audio tracks.
    [GcTest] GC latency mode: SustainedLowLatency
    [GcTest] ran {wallSeconds:F0}s real for 240s simulated, 39600 video packets
    [GcTest] per-packet cost on the capture thread (alloc + copy + buffer add):
               p50 {p50:F3}ms | p99 {p99:F3}ms | p99.9 {p999:F2}ms | max {max:F2}ms
               spikes >5ms: {n5} | >16ms: {n16} | >30ms: {n30}
    [GcTest] GC: gen0 {n0} | gen1 {n1} | gen2 {n2} | total pause {pauseMs:F0}ms ({pct:F2}% of runtime)
    [GcTest] heap: {liveBytes/1048576} MB live, {fragmentedBytes/1048576} MB fragmented, promoted last GC {promotedBytes/1048576} MB
    [GcTest] worst spikes (was a GC pause inside them?):
               {spikeMs,7:F2}ms @ {frame/165.0,6:F1}s sim  -> GC PAUSE
    [GcTest] spikes >5ms caused by GC: {gcSpikes} of {spikeCount}

MODE interactive changes mode label to Interactive. Worst section only if spikes exist, largest8 descending; each row ends "GC PAUSE" or "not GC". Percentile uses sorted[min(n-1,(int)(n*q))], not interpolation. Total-pause percent denominator=(SimSeconds/Speed*10). Restore original GC mode in finally. These measurements diagnose pauses; source imposes no numeric pass limit. GcSelfTest.cs:35–50,104–127.

### Process-loopback self-test

LAG_PROCTEST prints "[ProcLoopbackTest] capturing pid {pid} for 5s (play sound in that app)..."; capture ProcessLoopbackCapture((uint)pid), accumulate callback count,bytes,max abs float. Start failure prints "[ProcLoopbackTest] Start FAILED: {message}" then returns. Otherwise sleep5000ms and print:

    [ProcLoopbackTest] done: {callbacks} callbacks, {totalBytes} bytes (~{totalBytes/8.0/48000:F2}s), peak amplitude {peak:F4} (REAL AUDIO ✓).

If peak<=.0001f final parenthesized text is "silent/none". Duration counts delivered stereo samples including silent packets; valid PID playing nonquiet audio should yield callbacks/bytes>0 and REAL AUDIO. It is not an isolation assertion or fixed byte-count test. ProcessLoopbackCapture.cs:36–54.

### Green/unwritten-slice diagnostic

LAG_VFR_DIAG hardware path only: after convert/copy use staging16x16 source-format probe and16x16 NV12 probe, top-left box(0,0,0,16,16,1). Copy source patch, Map Read (blocking GPU completion), max over16 rows*64 bytes; if max<4 srcBlack++. Alpha bytes are INCLUDED in max, so opaque BGRA black (alpha255) does not classify srcBlack (preserve actual algorithm, not prose). Copy output array slice to NV12 probe, max luma16rows*16bytes<8→nv12Green++. Every120 probed frames:

    [DIAG] frames={diagFrames} srcBlack={srcBlack} nv12Green={nv12Green}

Errors "[DIAG] probe error: {message}". Staging textures mip1,array1,sample1,Usage.Staging,Bind.None,CPU.Read,Options.None; source probe reallocated when format changes; NV12 probe once. Expected healthy output nv12Green=0; positive green means unwritten/near-zero luma, while real limited-range black is near16. No automatic assert/recovery. Per-frame maps deliberately make this diagnostic expensive; no probe in MapSystemNv12. Bgra2Nv12Converter.cs:123–180.

### WGC timestamp research probe

LAG_WGCPROBE targets current foreground window, title buffer256,PID/process or"?"; defaults10s/165fps. Preallocate max(1000,seconds*600) arrival/source longs; WGC callback records Stopwatch timestamp and supplied source ticks with no allocation/lock, stops counting at capacity. Start cursorfalse,maxFps=fps; sleep seconds*1000; Dispose. Print target and frames/s; if<20 print "[WgcProbe] too few frames — is the game in the foreground and presenting?" and return. WgcTimestampProbe.cs:31–67.

ToQpc(srt)=srt if frequency10_000_000 else(long)(srt*(frequency/10_000_000.0)); UsFromQpc(q)=q*1e6/frequency double. Lag=arrival-source in µs; sorted percentile index=min(n-1,(int)(n*q)); future=lag<=0; fallback=lag<-5000 OR >500000; nonMono=source[i]<=source[i-1]. Report shape:

    [WgcProbe] target: '{title}' ({proc}, pid {pid}), {seconds}s, MinUpdateInterval for {fps}fps
    [WgcProbe] frames: {count} ({count/(double)seconds:F1}/s)
    [WgcProbe] QPC frequency {freq} Hz (100ns units: {freq==10000000})
    [WgcProbe] arrival − frame timestamp (ms; negative = timestamp is AHEAD of arrival):
                 min {P(0)/1000,7:F2} | p1 {P(.01)/1000,7:F2} | p10 {P(.10)/1000,7:F2} | median {P(.5)/1000,7:F2} | p90 {P(.90)/1000,7:F2} | p99 {P(.99)/1000,7:F2} | max {P(1)/1000,7:F2}
    [WgcProbe] timestamp at/after arrival: {future} of {count} ({future*100.0/count:F1}%)
    [WgcProbe] frames the engine would REJECT (lag < −5ms or > 500ms -> arrival-time fallback): {fallback} ({fallback*100.0/count:F1}%)
    [WgcProbe] non-increasing frame timestamps: {nonMono}

"engine would REJECT" and later "today's engine" labels refer to an older mixed-clock experiment; CURRENT live engine uses arrival for all frames. WgcTimestampProbe.cs:69–112; VfrReplayEngine.cs:135–147,502–506.

Report positive intervals for both "frame timestamps (SystemRelativeTime)" and "arrival times (when our callback ran)": upper median,population SD,within10%,max; for165,144,60Hz compute period1000/hz, percentage with rounded k>=1 and |interval-k*period|<=.25ms; top8 bins grouped Math.Round(ms*4)/4 descending count, strings "{bin:F2}ms x{count}". Then compare three strategies through CadenceGrid(fps): A arrival all, B source all, C mixed where lag outside[-5000,500000] uses arrival else source. Us arrays cast to long; kept/dropped count; backwards computed but not printed. Output:

    [WgcProbe] {label}:
                 median {med:F2}ms | stdev {sd:F2}ms | within +/-10% of median {within:F1}% | max {max:F2}ms
                 multiples of {hz}Hz ({period:F2}ms, +/-0.25ms): {pct:F1}%
                 most common: {bins}
    [WgcProbe] output cadence after CadenceGrid({fps}) — what each strategy would record:
       {strategyLabel,-38} kept {kept,4} dropped {dropped,3} | within +/-10% {within,5:F1}% | distinct {distinct,2} | stdev {sd:F2}ms | max {max:F2}ms

Labels are exactly "A  arrival time for every frame", "B  WGC timestamp for every frame", "C  today's engine (mixed per frame)". Numeric counts are machine/source-dependent; no PASSED line. WgcTimestampProbe.cs:94–172.

### Desktop duplication research probe

LAG_DUPPROBE constructs default shared D3D device, enumerates outputs on that adapter and prints name,size,origin,selected "<- probing"; DuplicateOutput selected index (invalid index is not caught). Acquire loop timeout100ms; count all successful acquired resources, sum AccumulatedFrames, count unique nonzero LastPresentTime values; always resource Dispose/ReleaseFrame. Failure AccessLost prints "[DupProbe] access lost (mode change / secure desktop) — stopping." and breaks; other acquisition failures continue. Every elapsed second boundary print current counters and reset; advances nextMark by1 only once per iteration; final elapsed denominator max(.001,elapsedSec). Expected:

    [DupProbe] adapter: {adapterDescription}
    [DupProbe] output {i}: {deviceName} {width}x{height} at ({left},{top})  <- probing
    [DupProbe] counting for {seconds}s — keep the game in the foreground...
    [DupProbe]   {nextMark,3:F0}s: acquired {secAcquired,4}/s | presents {secAccumulated,4}/s | distinct {secDistinct,4}/s
    [DupProbe] TOTAL over {t:F1}s: acquired {acquired/t:F1}/s | presents to output {accumulated/t:F1}/s | distinct frames {distinct/t:F1}/s

Only selected output has marker; no file/encoding. Intended comparison is whether generated/display frames reach output above WGC's rendered rate; there is no fixed FPS or PASS threshold. DuplicationProbe.cs:26–97.

### Windows audio-track probe

LAG_MFPROBE semicolon paths: print filename, MediaSource.CreateFromUri(new Uri(path)), synchronous OpenAsync, MediaPlaybackItem, AudioTracks. For each track get encoding props best-effort; output:

    [MfProbe] {filename}
    [MfProbe]   audio tracks: {count}, selected by default: index {selectedIndex}
    [MfProbe]   [{i}] SELECTED label='{Label}' lang='{Language}' id='{Id}' {Subtype} {SampleRate}Hz {ChannelCount}ch {Bitrate/1000}kbps

Unselected uses eight spaces in place of SELECTED; encoding errors leave suffix empty. Per-file error "[MfProbe]   failed: {message}". Dispose source on success. Expected for successfully encoded separate tracks: default selected index0 corresponds to All, with bitrate above source tracks; the probe only reports, never asserts or changes selection. PlaybackTrackProbe.cs:14–44; VfrReplayEngine.cs:699–718.

### Additional operational logs

Preserve component prefixes and meaningful arguments for troubleshooting; numbers use current .NET culture unless explicitly invariant above:

| Trigger | Literal message format |
| --- | --- |
| FFmpeg bind | "[Ffmpeg] Bound libavcodec {major}.{minor}.{patch} from {dir}"; "[Ffmpeg] Native VFR engine can start (libx264 baseline present)."; failure "[Ffmpeg] Native bind failed: {message}" or "[Ffmpeg] {InitError}". FfmpegInterop.cs:55–87. |
| Discovery | "[EncoderSelector] Usable: {friendly} ({name})"; "{name} probe threw, skipping: {message}"; backend/preferred unavailable messages; "No usable video encoder found (not even libx264).". EncoderSelector.cs:93–110,135–146. |
| Arm | "[VfrReplayEngine] Armed. Encoder: {friendly}, buffer {seconds}s.". VfrReplayEngine.cs:256. |
| Target | "[VfrReplayEngine] Locked window ended — releasing capture."; "Game appeared ({exe}) — switching from desktop to game."; "Game detected: {name} ({exe}) score={score} fullscreen={bool}"; "no monitor to capture (id='{id}')."; "Locked onto {label} @ {w}x{h} (WGC monitor/window capture).". VfrReplayEngine.cs:310,349–355,383,442. |
| Converter | "[Bgra2Nv12] Pipeline {inW}x{inH} → {outW}x{outH} NV12.". Bgra2Nv12Converter.cs:275. |
| Encoder | "[NvencVfrEncoder] {friendly}: D3D11 zero-copy/system-memory feed path (capture GPU vendor 0x{vendor:X4})."; "[NvencVfrEncoder] {friendly} open @ {w}x{h}, Auto (constant-quality)/CBR {kbps}kbps, fpsHint={hint}, VFR.". NvencVfrEncoder.cs:95–96,194–195. |
| Retune | "[fps-adapt] reopening encoder {old}→{hint} ..."; "new encoder live, disposing old ..."; "done — fixed-bitrate fps-hint {old}→{hint}, buffer reset."; exceptions "reopen failed, keeping current encoder: {message}" / "error, keeping encoder: {message}". VfrReplayEngine.cs:538,632–651. |
| Carry | "[CarryOver] holding {seconds:0.0}s of game '{name}'/desktop @ {w}x{h} — still saveable."; "[CarryOver] released — the live buffer now covers the full window."; "[VfrReplayEngine] stitching {headSeconds:0.0}s carried ({headW}x{headH}) + live ({liveW}x{liveH}).". VfrReplayEngine.cs:897–899,1044–1048,1066. |
| Teardown | "[Teardown] begin"; "[Teardown] encoder flushed @ {elapsedMs}ms"; "encoder disposed @ ..."; "capture disposed @ ..."; "audio disposed @ ..."; "complete @ ...", every line carries [Teardown]. VfrReplayEngine.cs:1097–1117. |
| Mux success | "[Mp4Muxer] wrote {videoCount} video + {audioStreamCount} audio track(s) → {path}". Mp4Muxer.cs:78–79. |
| AAC | "[AacAudioEncoder] AAC open: in {rate}Hz {channels}ch {fmt} → 48kHz stereo, {requestedKbps}kbps."; "[AacAudioEncoder] mic mix on: {rate}Hz {channels}ch {fmt} → 48kHz stereo.". AacAudioEncoder.cs:79,242. |

Operational error paths also preserve ring alignment message, WASAPI selected-device fallback/app-not-running/duplicate/mic-unavailable logs, overlay single-error pass-through and webcam enumeration/frame errors as specified in section4. These are best-effort diagnostics, not user-facing localized notifications.

## 9. Known bugs / quirks to preserve or fix

**Policy:** preserve the following current observable rules for the 1:1 acceptance target; proposed fixes need a separate change in requirements. Native lifetime races/leaks are implementation defects to prevent with Rust ownership and synchronization, not crashes to intentionally reproduce.

1. **Stale comments are not behavior. Preserve executable implementation.** Engine says "video-only for now" despite active audio; converter/encoder comments sometimes say full-range despite limited output; source comments mention hybrid/full present behavior while active engine only uses WGC; QSV is system-memory; normal UI baked stats are unused. VfrReplayEngine.cs:85–86,667–780; NvencVfrEncoder.cs:89–94,153–158; MainViewModel.cs:538–539.
2. **Arrival timestamps only. Preserve.** WGC SystemRelativeTime is deliberately ignored; old ±5ms/500ms mixed-source fallback exists only as a probe comparison and must not be restored into capture. The measured source note reports timestamps3.8–10.6ms in the future on one rig, not an invariant for every machine. VfrReplayEngine.cs:135–147,502–506; WgcTimestampProbe.cs:79–112.
3. **Adapter mapping shadows x264. Preserve or explicitly fix later.** MapCodec checks "264" before "x264", so UI adapter x264 strings select h264 family. Raw backend IDs nvenc/amf/qsv map to null/Auto, whereas direct engine Select supports them. VfrRecorderAdapter.cs:111–119; EncoderSelector.cs:78–120.
4. **Auto selects hardware H264, not candidate-list first AV1. Preserve.** Missing requested family/backend returns avail[0] and can choose a different family/vendor than a "next-best lower tier" description implies. EncoderSelector.cs:89–120.
5. **Device reused across cold restarts. Preserve observable selection unless fixed.** Stop keeps _d3d; later AdapterIndex change does not create a new device until engine Dispose. Encoder selection remains independent of device vendor and may fall back to readback. VfrReplayEngine.cs:253–254,1128–1146; NvencVfrEncoder.cs:93–94.
6. **Retune uses lifetime counters. Preserve current calculation.** FramesCaptured/FramesEncoded are never reset per session; one-time3s fixed-bitrate retune can overestimate after a lock switch/restart, making clamp keep old hint. Fresh encoder open happens before old free, so session-limited hardware may reject reopen. Hints<5 cause Math.Clamp(measured,5,hint) to throw, caught once and adaptation disabled. VfrReplayEngine.cs:208–209,423–425,534–539,606–650.
7. **Counters are attempts, not all successful packets. Preserve definitions.** FramesCaptured excludes paused frames; FramesEncoded increments even when Encode returnsfalse or internally skips, while TimedFrames can differ. VfrReplayEngine.cs:486–491,525–529; NvencVfrEncoder.cs:272,289–302.
8. **Hold game until HWND/process dies. Preserve.** Minimize/invisibility/focus loss does not release; no Closed subscription or stall timeout triggers capture recovery. A live but nonpresenting game can hold indefinitely. VfrReplayEngine.cs:303–312,410–411.
9. **Explicit missing monitor does not fall back in video. Preserve.** Screenshot selected-monitor failure does fall back primary; audio selected-output failure does default fallback; mic specific-device failure disables mic. VfrReplayEngine.cs:380–384,465–481; ScreenshotService.cs:45–46; WasapiAudioSource.cs:109–121,136–141.
10. **Fixed encoder dimensions survive source resize. Preserve.** WGC pool3→2 and converter rebuilds input, but output dimensions remain those from initial GetWindowRect/monitor. Changed aspect stretches; no session restart/letterbox for ordinary live resize. WgcCaptureSource.cs:141–148; Bgra2Nv12Converter.cs:195–211; VfrReplayEngine.cs:368–373.
11. **8-bit SDR only. Preserve scope.** No HDR/tone mapping, wide-gamut,10-bit or camera rotation/mirroring implementation. Bgra2Nv12Converter.cs:213–239; WebcamCaptureSource.cs:106–120.
12. **Approximate buffer length. Preserve keyframe logic.** Ring minimum5s; keyframe-start snapshots/carry can exceed configured window by up to a GOP; carried memory ages every2s by live PTS span, not wall time. Comments promising exact bound are stronger than code. PacketRingBuffer.cs:25–27,57–70; CarryOverSegment.cs:61–85; VfrReplayEngine.cs:1054–1064.
13. **Single carry only. Preserve.** A new nonempty ending session replaces old carry; repeated rapid transitions can lose older footage. Failed/empty ending sessions retain older carry. Save does not consume it; no wall-time expiry. VfrReplayEngine.cs:1024–1042,1056–1064,932–935.
14. **Save is not globally atomic. Prevent unsafe races; preserve snapshot semantics.** It releases _gate before PumpAudio/spec/snapshots; audio/video snapshots can contain slightly different tails and target teardown/retune can race spec reads. No engine-level simultaneous-save exclusion; caller normally guards IsSaving but direct callers can overlap. VfrReplayEngine.cs:823–857; MainViewModel.cs:658–676.
15. **Empty/failure save does not notify. Preserve.** No ReplaySaved on null/false primary; caller20s watchdog clears saving. LastSavedGameTag is not reset for early no-data return, only after NewClipPath. VfrReplayEngine.cs:861–868,930–935; MainViewModel.cs:695–705.
16. **No unique filenames/atomic completion. Preserve names; fixes optional later.** Same-second replay/screenshot can overwrite/fail at existing path; mux can leave partial files; stitch fallback can emit head event while primary save returnsnull, and both events observe carried game tag. VfrReplayEngine.cs:919–945; ScreenshotService.cs:53–59.
17. **Audio-app fallback can capture more than requested. Preserve actual conditions.** Empty requested apps mode or all captures failing can become full-system; all-failed fallback ignores SystemAudioEnabled; no per-app rediscovery until restart. WasapiAudioSource.cs:99–103,155–194.
18. **Mic/loopback controls are float-only. Preserve.** Integer/Extensible formats can bypass intended gain/gate/denoise/mono; sample-format mapping has no explicit24bit handling. WasapiAudioSource.cs:254–255,263–305; VfrReplayEngine.cs:807–811.
19. **Mic processing startup and toggle state. Preserve.** Mic starts before final gain/mute/gate/denoise setters and engine callback subscription. Muted mic drops buffers without advancing processors; existing mixed mic FIFO can still play queued samples after mute. Gate Reset leaves Threshold/IsOpen; recorder initial previous gateGain1 gives its first gate transition a ramp. WasapiAudioSource.cs:34,105–118,261–285; VfrReplayEngine.cs:688–695,723–765; AacAudioEncoder.cs:247–269; NoiseGate.cs:71–76.
20. **AAC timing/trim limitations. Preserve.** Master PadTo uses callback-arrival time before appending full chunk; it does not subtract chunk duration. Pump and real callbacks may put write timeline ahead of clock. Gap>5s ignored entirely; Flush drops incomplete FIFO/mic/resampler remainder. Mic mixing queues do not use PTS; already48k stereo AAC output even for forced mono input. AacAudioEncoder.cs:119–123,153–157,197–275,337–342.
21. **RNNoise permanent failure latch. Preserve.** Missing model/filter/setup exception disables that instance until recreated, toggling Enabled does not retry. Output variable/zero; no filter flush tail on stop. MicDenoiser.cs:44–53,90–95,200–210.
22. **Track list versus comments. Preserve All,System,Microphone.** Older Mp4Muxer docs claim two tracks system/mic, but current separate path deliberately creates a mixed All default first at higher bitrate. Missing mic track may be omitted entirely rather than always creating a silent placeholder stream. Mp4Muxer.cs:33–35; VfrReplayEngine.cs:699–748,852–856.
23. **Mux color metadata omissions and unchecked returns. Preserve current successful-file behavior; proposed robustness fix separate.** Encoder tags BT709 limited but StreamSpec/mux codecpar omit explicit color tags/bitrate; packet duration/side-data omitted, extradata padding not zeroed, equal-PTS sort unstable, packet/trailer errors unchecked. Mp4Muxer.cs:10–15,73–80,105–168.
24. **Transcode stitch differs from live codec/RC. Preserve.** Always tries NVIDIA family encoder, falls back libx264 even from HEVC/AV1; bitrate floor1000k even Auto; pairs audio by first common count, not title/config; no explicit gap/silence fixes/default flag/color metadata or faststart; stderr read+wait has no timeout and undrained stdout can hang. ReplayStitcher.cs:85–136,158–188.
25. **Steam/GPU classifier false positives. Preserve rules.** Steam signal global, known game can qualify off-monitor/windowed, GPU stale on failure, direct Evaluate doesn't refresh; fixed lists include explicit nongame excludes despite "not a denylist" prose; Unity absent despite summary. GameDetector.cs:19–25,75–115,161–203,239–249; GpuUsageProbe.cs:65–103.
26. **Scheduling contradiction. Preserve actual calls.** PerformanceGuard comment promises Normal CPU, but capture raises High, teardown unconditionally Normal, not previous priority. GC restored Interactive, not previous; headless paths skip PerformanceGuard but still get capture High/GC changes. PerformanceGuard.cs:66–68; VfrReplayEngine.cs:435–439,1118–1125; Program.cs:70–135.
27. **GC/probe lifetime leaks and callback joins. Fix safely without changing output.** GameDetector never disposes GpuUsageProbe; HSTRING creation not deleted; process-loopback ready/handler events and async COM op not explicitly disposed; timed Start wait ignores timeout boolean; join timeout can close event while worker lingers; Timer.Dispose doesn't await callbacks. Use Rust RAII/worker shutdown safely, retaining normal timing/defaults. GameDetector.cs:27–41; WgcCaptureSource.cs:227–246,254–273; ProcessLoopbackCapture.cs:57–69,179–190,198–228; VfrReplayEngine.cs:1073–1075.
28. **Tracker lossy modifiers/max10. Preserve.** One label for left/right; max10 can evict held input; faded insertion order not recent-press order; unsupported drawn keys don't magically appear, wall time controls fade through pause. KeystrokeTracker.cs:57–105,115–147; OverlayCompositor.cs:82–89.
29. **Webcam comments versus implementation. Preserve pixels/mode.** Single locked array plus copies rather than a lock-free double buffer; lower FPS tie-break; log may show failed selected format instead of actual; TryCopyLatest false sets out dimensions0 despite "leaves outputs untouched". WebcamCaptureSource.cs:71–91,113–146.
30. **Diagnostics are not all assertions. Preserve reporting semantics.** GC/WGC/DXGI/MF/process probes mostly print observations, cadence/carry/stall print status but do not set process failure code; static failed counters aren't reset. DIAG includes source alpha in black classifier. Program.cs:101–135; CadenceSelfTest.cs:35–39; CarryOverSelfTest.cs:14–29; StallMonitorSelfTest.cs:17–27; Bgra2Nv12Converter.cs:140–146.
31. **Unused DUPONLY/hybrid. Do not revive as normal feature.** LAG_VFR_DUPONLY is ineffectual in active engine. Dormant hybrid also mixes raw WGC100ns values with duplicationµs, another reason not to infer a working switch path from its prose. VfrEngineTest.cs:73; HybridGameCaptureSource.cs:73–87; WgcCaptureSource.cs:152.

## 10. Parity checklist

Each item is independently verifiable against the preceding exact algorithms. A test should exercise successful and stated failure/fallback cases; no extra CFR padding, altered defaults or undocumented automatic recovery should be inferred.

- [ ] Production composition resolves IReplayRecorder to VfrRecorderAdapter and never constructs the dead legacy cluster. App.axaml.cs:353–359.
- [ ] All 35 scoped VfrCapture sources and 11 shared/legacy sources are accounted for, including diagnostic-only and unreachable classifications in section1.
- [ ] Rust uses the bundled FFmpeg7.1 DLL ABI and refuses libavcodec major other than61. FfmpegInterop.cs:53–64.
- [ ] FFmpeg bind is attempted once process-wide, including failure caching, before encoder discovery. FfmpegInterop.cs:34–49; App.axaml.cs:46–50.
- [ ] Missing libx264 sentinel makes engine unavailable even if hardware encoder symbols exist. FfmpegInterop.cs:71–78.
- [ ] Availability also requires WGC support and at least one successfully opened candidate. VfrReplayEngine.cs:231–234.
- [ ] RecorderOptions defaults/types and VfrEngineOptions differing defaults match section4 exactly. RecorderOptions.cs:10–110; VfrReplayEngine.cs:17–73.
- [ ] Initialize caches options and StartBuffer before Initialize is a no-op. VfrRecorderAdapter.cs:41–46.
- [ ] Adapter ignores Width/Height/OutputWidth and maps only OutputHeight to target height. VfrRecorderAdapter.cs:60–105.
- [ ] Adapter maps nonpositive FrameRate to120 and forces cursor capture true. VfrRecorderAdapter.cs:68,90.
- [ ] Adapter audio percentage gains divide by100f without additional clamp. VfrRecorderAdapter.cs:83–85.
- [ ] Adapter MapCodec substring ordering, including x264 shadowing by264, matches source. VfrRecorderAdapter.cs:111–119.
- [ ] Candidate order/friendly names/codec IDs/hardware flags match all10 entries. EncoderSelector.cs:27–38.
- [ ] Probe runs on dedicated joined MTA EncoderProbe with1280x720,NV12,60fps,1/1000,4Mbps,GOP120 and no explicit hw context. EncoderSelector.cs:55–62,157–182.
- [ ] Probe retains only open2>=0 successes and caches results. EncoderSelector.cs:46–47,128–147.
- [ ] Auto chooses first hardware H264 before other available codecs. EncoderSelector.cs:114–120.
- [ ] Direct force CPU/backend/family selection and missing-preference fallback match exact Select branches. EncoderSelector.cs:71–120.
- [ ] D3D uses chosen adapter with default-hardware fallback and exactly BGRA_SUPPORT|VIDEO_SUPPORT. D3D11Context.cs:33–55.
- [ ] Shared immediate context is multithread-protected and FFmpeg shares the same device/context references. D3D11Context.cs:62–66; NvencVfrEncoder.cs:113–126.
- [ ] Device remains reused after Stop/Start and adapter change follows current reuse behavior. VfrReplayEngine.cs:253,1128–1146.
- [ ] Start sets running/arms a500ms initial,2000ms periodic lock timer. VfrReplayEngine.cs:285–286.
- [ ] No forced capture starts synchronously during Start before timer evaluation. VfrReplayEngine.cs:238–287.
- [ ] AutoGame detects game or starts desktop; Display bypasses detection. VfrReplayEngine.cs:335–361.
- [ ] ForcedWindow bypasses detection/mode and is held as Game after valid HWND/PID lookup. VfrReplayEngine.cs:320–333.
- [ ] Game lock survives focus,visibility,minimization and monitor changes until HWND/process invalid. VfrReplayEngine.cs:305–312.
- [ ] Game acquisition dimensions use GetWindowRect and reject<2 pixels. VfrReplayEngine.cs:368–373.
- [ ] Monitor capture resolves primary for null/empty, matches explicit GDI ID case-insensitively, and does not video-fallback for missing ID. VfrReplayEngine.cs:380–388,455–481.
- [ ] OutputDims downsizes height only, preserves initial aspect with midpoint-to-even rounding and even masks. VfrReplayEngine.cs:391–396.
- [ ] Failed session lock backs off15 UTC seconds rather than retrying every tick. VfrReplayEngine.cs:445–449.
- [ ] New session publishes target/clock/reset state before starting WGC, audio and target event. VfrReplayEngine.cs:413–443.
- [ ] WGC uses HWND or HMONITOR GraphicsCaptureItem interop without retargeting a live session. WgcCaptureSource.cs:54–88,225–273.
- [ ] WGC startup pool is free-threaded BGRA8 with3 buffers. WgcCaptureSource.cs:85–88.
- [ ] WGC cursor setter is best-effort. WgcCaptureSource.cs:93–94.
- [ ] Borderless access request precedes best-effort IsBorderRequired=false, with exact available/unavailable logs. WgcCaptureSource.cs:96–116.
- [ ] Only Windows>=26100 and positive maxFps sets MinUpdateInterval=1000/(2*maxFps) milliseconds. WgcCaptureSource.cs:118–125.
- [ ] WGC pulls one frame per event and invokes synchronous texture callback with raw SystemRelativeTime ticks. WgcCaptureSource.cs:132–152.
- [ ] Resize recreates WGC pool with2 buffers and BGRA8 while keeping encoder output dimensions fixed. WgcCaptureSource.cs:141–148.
- [ ] Texture/frame lifetime ends after callback; no queue retains pool texture. WgcCaptureSource.cs:150–162.
- [ ] Per-frame WGC failures log and permit later frames. WgcCaptureSource.cs:154–162.
- [ ] Closed is exposed by source but active engine uses polling rather than Closed-driven teardown. WgcCaptureSource.cs:83; VfrReplayEngine.cs:303–312,410–411.
- [ ] Capture,composition,conversion,video send/drain and FPS adaptation execute synchronously on the WGC thread. VfrReplayEngine.cs:484–545.
- [ ] Shared engine clock subtracts atomic pause snapshot using exact integer QPC formulas. VfrReplayEngine.cs:658–665.
- [ ] Video PTS always uses EngineNowUs arrival, ignoring sourceTicks. VfrReplayEngine.cs:502–506.
- [ ] Cadence origin pins on first frame; nonpositive FPS passes timestamps through. CadenceGrid.cs:72–81.
- [ ] Cadence positive/negative slot rounding and rational position formula match section4 exactly. CadenceGrid.cs:86–97.
- [ ] Filled/backward slots drop with sentinel-1 and atomic collision count; missing slots remain gaps. CadenceGrid.cs:90–97.
- [ ] Repin clears anchor/last slot but not collision count. CadenceGrid.cs:56–65.
- [ ] New-session/resume repin and stall reset are applied only by the next capture callback. VfrReplayEngine.cs:493–500,1002.
- [ ] Pause discards incoming video/mic/system callbacks and freezes engine timeline without closing codecs. VfrReplayEngine.cs:488–490,723–765,986–1005.
- [ ] Save during pause sees frozen clock and preserves pre-pause buffer. VfrReplayEngine.cs:658–665,817–857.
- [ ] Resume excludes paused duration and logs total folded paused milliseconds. VfrReplayEngine.cs:993–1004.
- [ ] FramesCaptured/FramesEncoded remain cumulative and match attempted-frame semantics. VfrReplayEngine.cs:208–209,491,529.
- [ ] Converter uses progressive video processor index0,120/1 input/output hints,PlaybackNormal and full source/destination rects. Bgra2Nv12Converter.cs:195–211.
- [ ] Converter input color is full RGB (RgbRange=false), output BT709 limited (YCbCrMatrix=true,NominalRange=1). Bgra2Nv12Converter.cs:220–226.
- [ ] Pool BGRA is copied into owned RenderTarget|ShaderResource input texture before video processing. Bgra2Nv12Converter.cs:67,228–240.
- [ ] Input view fields and stream Enable/indices/past/future values match source. Bgra2Nv12Converter.cs:69–86.
- [ ] VideoProcessorBlt targets a single non-array NV12 RenderTarget scratch, then CopySubresourceRegion into encoder array slice. Bgra2Nv12Converter.cs:95–101,242–259.
- [ ] Normal hardware path has no CPU staging map/fence and stays on shared GPU context. Bgra2Nv12Converter.cs:95–101.
- [ ] System path maps NV12 staging and copies Y/UV separately using source pitch and FFmpeg linesizes. NvencVfrEncoder.cs:322–346.
- [ ] Converter rebuilds input-dependent pipeline on size change and outputs8bit SDR only. Bgra2Nv12Converter.cs:183–239.
- [ ] NVENC/NVIDIA and AMF/AMD matched devices use D3D11 hwframes; QSV always system-memory; cross-vendor hardware uses readback. NvencVfrEncoder.cs:87–111.
- [ ] D3D11 FFmpeg hwdevice/frame pool format,D3D11/NV12,width,height,pool16 and bind flags match source. NvencVfrEncoder.cs:113–141.
- [ ] Every explicit video AVCodecContext field in section4 matches, including1/1e6 time base,GOP240,Bframes0,global header. NvencVfrEncoder.cs:144–188.
- [ ] Encoder color metadata is BT709 primaries/TRC/matrix and MPEG limited range. NvencVfrEncoder.cs:155–158.
- [ ] Auto bitrate<=0 sets bit_rate0 with NVENC constqp20/HEVC22,AMF23,QSV global_quality23 or x264 CRF21. NvencVfrEncoder.cs:69–76,164–172,209–249.
- [ ] Fixed bitrate sets bps,rc_max_rate=bps,rc_buffer_size=(int)(bps*2). NvencVfrEncoder.cs:175–178.
- [ ] NVENC always uses p1..p7 clamp,hq,lookahead0,spatial-aq1,forced-idr1; fixed cbr+qres. NvencVfrEncoder.cs:209–228.
- [ ] AMF quality speed/preanalysis false/forced_idr1,cqp QP I/P23 or vbr_peak exactly. NvencVfrEncoder.cs:230–236.
- [ ] QSV veryfast/forced_idr1 and x264 veryfast/zerolatency/forced-idr1 match exact spelling. NvencVfrEncoder.cs:238–249.
- [ ] Rejected private options log and do not abort before open2. NvencVfrEncoder.cs:203–207.
- [ ] First frame anchors forced-keyframe time and later frames force I at>=2_000_000us intervals. NvencVfrEncoder.cs:283–288.
- [ ] Hardware/software frame-buffer failure skips without propagating fatal engine failure; EAGAIN send is not retried. NvencVfrEncoder.cs:272,289–310,328.
- [ ] Video receive drains ready packets until EAGAIN/EOF without waiting for own-frame completion. NvencVfrEncoder.cs:349–371.
- [ ] Encoded payload is privately copied; PTSµs,DTS NOPTS fallback,key flag captured. NvencVfrEncoder.cs:357–370.
- [ ] H264 IDR/HEVC IRAP fallback parser supports Annex-B and4byte lengths; AV1 uses flags only. NvencVfrEncoder.cs:377–406.
- [ ] Packet model omits durations/side-data, stream extradata copied before encoder free. NvencVfrEncoder.cs:10,410–420.
- [ ] Retune only fixed bitrate,after3s,measured floor/clamp/15% test uses cumulative FramesEncoded. VfrReplayEngine.cs:604–618.
- [ ] Retune creates fresh encoder before freeing old, resets video ring only and preserves clock/audio/grid. VfrReplayEngine.cs:625–651.
- [ ] Retune failure logs/keeps old and is one-shot. VfrReplayEngine.cs:538,608,639–643.
- [ ] AudioModeByTarget overrides manual apps mode game→apps,desktop→wholePC. VfrReplayEngine.cs:683–691.
- [ ] AudioEnabled=false creates no capture/AAC/pump. VfrReplayEngine.cs:678.
- [ ] WholePC chooses specified render endpoint or default Multimedia with invalid-ID fallback. WasapiAudioSource.cs:124–150.
- [ ] Effective empty apps mode fallback follows exact _appsMode formula. WasapiAudioSource.cs:99–103.
- [ ] Detected game process capture starts first and owns duplicate selected-game PID. WasapiAudioSource.cs:160–181.
- [ ] Selected-app process lookup prefers a main window and otherwise first process; no late rediscovery. WasapiAudioSource.cs:166–184.
- [ ] All process-capture failures fall back to full-system even with SystemAudioEnabled=false. WasapiAudioSource.cs:187–194.
- [ ] Each process-loopback uses one named MTA thread for activation,use and COM release. ProcessLoopbackCapture.cs:63–95.
- [ ] Process activation uses VAD\Process_Loopback,VT_BLOB,ActivationType1,include-target-tree0 and correct IIDs. ProcessLoopbackCapture.cs:163–190,233–248.
- [ ] Process PCM is48k/2ch/float,shared LOOPBACK|EVENTCALLBACK,200ms buffer,periodicity0. ProcessLoopbackCapture.cs:101–118.
- [ ] Activation/ready/packet waits and teardown join use5000/6000/100/2000ms source limits. ProcessLoopbackCapture.cs:68,140,183,208.
- [ ] Silent process packets emit zero bytes of same sample count and release native buffer before callback. ProcessLoopbackCapture.cs:145–156.
- [ ] Process app queue gain/backlog drop and20ms1920float summing/clamp match exactly. WasapiAudioSource.cs:213–246.
- [ ] Full-system gain applies only IEEE float with copy/clamp; app sum does not apply system gain again. WasapiAudioSource.cs:228–256.
- [ ] Microphone null/default Communications,empty disabled,specific ID no-fallback semantics match. WasapiAudioSource.cs:105–121.
- [ ] Mic mute drops PCM before gate/denoise and volume clamps negative only at control level. WasapiAudioSource.cs:61–75,261.
- [ ] Mic path order is float gain→raw-input gate decision/ramp→RNNoise→optional mono→emit. WasapiAudioSource.cs:265–308.
- [ ] Gate peak perceptual curve,dt fallback and samplewise ramp denominator n match source. WasapiAudioSource.cs:273–285,323–346.
- [ ] NoiseGate all constants,floor/clamp formulas,hold,attack/release and .0005 gain cutoff match. NoiseGate.cs:29–67.
- [ ] NoiseGate.Reset does not reset Threshold/IsOpen. NoiseGate.cs:71–76.
- [ ] RNNoise graph/filter instance/options and model path match section4, including av_opt_set model path. MicDenoiser.cs:98–171.
- [ ] Denoiser returns pass-through on permanent failure and true/zero while buffering; variable output count respected. MicDenoiser.cs:44–95; WasapiAudioSource.cs:293–299.
- [ ] Denoiser buffer starts8192/doubles and no stop-time graph flush adds tail. MicDenoiser.cs:190–210.
- [ ] Mono averages stereo float pairs only and eventual AAC remains stereo. WasapiAudioSource.cs:301–319; AacAudioEncoder.cs:19–20.
- [ ] MapSampleFormat FLT/S16/S32/fallbackFLT semantics match, including unsupported-format quirk. VfrReplayEngine.cs:807–811.
- [ ] AAC context is48k,stereo FLTP,min64kbps,1/48000,GLOBAL_HEADER,with input resampler/FIFOs. AacAudioEncoder.cs:46–79.
- [ ] AAC master and mic operations serialize against disposal with correct lock order. AacAudioEncoder.cs:30–40,85–91,138–148,188–225.
- [ ] AAC PadTo exact expected sample math,positive<=5s condition and append-after-padding behavior match. AacAudioEncoder.cs:119–123,153–157.
- [ ] Silence writes4096-sample chunks,zeroes before mic mix and continues during no-loopback callbacks. AacAudioEncoder.cs:162–183.
- [ ] Mic swresampler lazy init,oldest FIFO drop at1s and clamped additive mix match. AacAudioEncoder.cs:197–275.
- [ ] AAC frame size comes from codec or1024,PTS counts read samples,packetµs conversion/key=true match. AacAudioEncoder.cs:277–318.
- [ ] AAC Flush sends null but does not flush partial FIFO/resampler/mic tail. AacAudioEncoder.cs:337–342.
- [ ] Single track title Audio at160kbps and separate All192,System160,Mic160 order match. VfrReplayEngine.cs:699–766.
- [ ] Mic separate track is lazily created on first delivered mic buffer and omitted if still empty. VfrReplayEngine.cs:731–748,854–856.
- [ ] Default All mixes system+mic, source System excludes mic, source Microphone excludes system. VfrReplayEngine.cs:723–746.
- [ ]100ms audio pump advances every active track and save pumps before snapshot. VfrReplayEngine.cs:771–795,837.
- [ ] PacketRingBuffer minimum window5s and newest-minus-oldest span match. PacketRingBuffer.cs:25–38.
- [ ] Ring evicts only complete oldest keyframe segments when second keyframe<=cutoff. PacketRingBuffer.cs:57–80.
- [ ] Video Snapshot begins latest keyframe<=cutoff else earliest; no keyframe means empty. PacketRingBuffer.cs:108–122.
- [ ] Audio companion alignment advances video only if lead>500ms and later covering keyframe exists. PacketRingBuffer.cs:124–138.
- [ ] Video/audio snapshots rebase both PTS and DTS from same video absolute base; early audio dropped. PacketRingBuffer.cs:140–164; VfrReplayEngine.cs:847–857.
- [ ] Absolute snapshots/Clear copy and clear proper collections under gate without payload recopy. PacketRingBuffer.cs:172–185.
- [ ] Save with no live/carry returnsnull and no completion event. VfrReplayEngine.cs:861–865.
- [ ] Normal save does not Flush video or consume video/audio rings. VfrReplayEngine.cs:833–873,932–935.
- [ ] Target teardown snapshots flushed video/spec/audio before respective contexts disappear. VfrReplayEngine.cs:1103–1116.
- [ ] Empty ending session retains previous carry; nonempty replaces one carry with captured tag. VfrReplayEngine.cs:1024–1042.
- [ ] Carry trim respects keyframe selection,returns same object index0,null no key/zero seconds and cuts audio to video origin. CarryOverSegment.cs:57–85.
- [ ] Carry and live have independent absolute clocks and rebase independently before stitching. CarryOverSegment.cs:88–107; VfrReplayEngine.cs:847–896.
- [ ] Carry aging uses room=buffer-liveSpan,cutoff>.5 and compare-reference replacement every lock tick. VfrReplayEngine.cs:295–297,1054–1066.
- [ ] Save after game exits retains playable prior-game footage,sets LastSavedGameTag and remains repeat-saveable. VfrReplayEngine.cs:875–936.
- [ ] Compatible streams concatenate untouched with offset=headLastPTS+16000 for PTS and DTS. ReplayStitcher.cs:28–56; VfrReplayEngine.cs:896–907.
- [ ] Direct audio merge uses live track order,title ignore-case and exact codec/layout/extradata equality; discards unmatched head-only tracks. VfrReplayEngine.cs:954–970.
- [ ] Incompatible stitch temp naming,first-common audio count,scale/pad/setsar/yuv420p/concat graph match. ReplayStitcher.cs:79–108.
- [ ] Stitch command uses correct NVENC codec,preset clamp,bitrate>=1000k,AAC160/default192 and live titles. ReplayStitcher.cs:110–126,159–164.
- [ ] Hardware stitch retries once with libx264 veryfast without silently changing other args. ReplayStitcher.cs:128–136.
- [ ] Stitch failure saves live primary and carried -Game half,with independent head completion event. VfrReplayEngine.cs:914–925.
- [ ] Stitch process stderr tail400,no timeout and best-effort temp cleanup match diagnostics contract. ReplayStitcher.cs:143–146,166–194.
- [ ] Replay filename local timestamp/container normalization match exactly; no new uniqueness suffix. VfrReplayEngine.cs:800–805,939–945.
- [ ] Mux container names,video-first stream order,title/default disposition and empty-audio omission match. Mp4Muxer.cs:45–65.
- [ ] No faststart or extra mux options are added to direct replay output. Mp4Muxer.cs:18–21,67–77.
- [ ] StreamSpec/codecpar/extradata fields match source without inventing muxer color/bitrate metadata. Mp4Muxer.cs:10–15,105–136.
- [ ] Mux rescales packet PTS/DTS into post-header stream time bases and uses PTS-only merge. Mp4Muxer.cs:139–169.
- [ ] GameDetector visibility/iconic/PID/exclusion rules and full-monitor test match exactly. GameDetector.cs:161–181,223–236.
- [ ] Complete known-game names,engine classes,ShellExclude and NonGameGpuExclude match section4. GameDetector.cs:46–115.
- [ ] Qualification formula and46000/30000/18000/30% weights match with global Steam bonus. GameDetector.cs:31–38,191–203.
- [ ] Candidate tie keeps first EnumWindows winner and fallback name trims title256. GameDetector.cs:129–134,206–218.
- [ ] Steam RunningAppID requires a nonzero int in exact HKCU path. GameDetector.cs:242–249.
- [ ] GPU probe uses English3D wildcard,sum-per-PID,stale-on-failure map and no re-open retry. GpuUsageProbe.cs:49–117.
- [ ] Audio picker enumerates active default-render sessions,dedups exe,skips self/PID0,sorts friendly descriptions and resolves icons with limited-query path. AudioSessionService.cs:31–123.
- [ ] Audio picker uses same detector to remove live game entries while preserving enabled disappeared selections. SettingsViewModel.cs:808–848.
- [ ] Audio endpoint notifications include state/add/remove/default but ignore property changes; subscriber debounce300ms. AudioDeviceWatcher.cs:28–34; SettingsViewModel.AudioMeter.cs:33–67.
- [ ] Screenshot exact monitor/null/primary fallback,GDI full physical rectangle,PNG naming andnull failure match. ScreenshotService.cs:41–94.
- [ ] Webcam enumeration/default device,CPU Color source,closest640>=15fps/lowerFPS tie and Realtime reader match. WebcamCaptureSource.cs:37–95.
- [ ] Webcam callback normalizes BGRA8 premultiplied and sequence-gated latest-frame copies. WebcamCaptureSource.cs:97–146.
- [ ] Overlay options/positions/sizes/draw order and exact brush float colors match section3. OverlayCompositor.cs:15–35,145–149,206–211.
- [ ] Overlay uses owned BGRA copy/Direct2D target and passes original on disabled/error paths. OverlayCompositor.cs:128–204.
- [ ] Webcam PiP aspect,radius14,position-free-space,border2 and linear interpolation scale with frameH/1080. OverlayCompositor.cs:214–238.
- [ ] Keyboard exact5row key IDs/text/widths and geometry/text thresholds match section3. OverlayCompositor.cs:82–89,242–313.
- [ ] Mouse dimensions/split/wheel/radii/strokes and LMB/RMB/MMB opacities match section3. OverlayCompositor.cs:315–366.
- [ ] Tracker uses existing hook only,cap10,merged modifier labels,insertion order and450ms linear wall-time fade. KeystrokeTracker.cs:17–105,108–148.
- [ ] Tracker complete label mapping and unsupported-code null behavior match source guards. KeystrokeTracker.cs:109–148.
- [ ] Stats sampler integer FPS/CPU/GPU/RAM formulas and1s snapshot timer match. SystemStatsSampler.cs:26–107.
- [ ] Baked stats0/1/other text,Segoe SemiBold font,layout/rebuild and panel geometry match. OverlayCompositor.cs:369–406.
- [ ] Normal UI keeps baked stats off and uses FramesCaptured for separate live HUD. MainViewModel.cs:440–452,538–539.
- [ ] Overlay failure is nonfatal and first compose failure logs once; camera failure degrades without blocking capture. OverlayCompositor.cs:102–114,155–162.
- [ ] Capture starts High CPU priority/SustainedLowLatency and teardown Normal/Interactive. VfrReplayEngine.cs:435–439,1118–1119.
- [ ] PerformanceGuard opts out execution throttling and tries GPU classes5,4,3 without automatic elevation. PerformanceGuard.cs:47–84.
- [ ] Process requests timeBeginPeriod(1) before headless/UI startup. Program.cs:65–68.
- [ ] Teardown detaches callback and disposes video encoder before WGC pool,then WASAPI,flush/snapshot/dispose AAC. VfrReplayEngine.cs:1098–1116.
- [ ] Stop clears carry and overlay; Dispose frees retained D3D device,with safe in-flight ownership. VfrReplayEngine.cs:1115,1128–1146.
- [ ] Lock evaluation,carry trim and stats errors are separately logged and do not kill timer. VfrReplayEngine.cs:290–300.
- [ ] Stall threshold strictly>25ms,history8,queue64,details6 and half-gap attribution match. CaptureStallMonitor.cs:32–36,82–151.
- [ ] Stall dominance GC-first then other/overlay/convert/encode/retune and exact report layout match. CaptureStallMonitor.cs:141–184.
- [ ] Capture stats log is timer-thread,nonreentrant,baseline-resetting and includes encoded/WGC FPS/stages/preset/grid/drops. VfrReplayEngine.cs:558–599.
- [ ] All 34 LAG_* variables in section8 are recognized with exact parsing/defaults and no added active meaning for DUPONLY.
- [ ] Headless dispatch priority matches Program and bypasses normal UI/mutex/crash setup. Program.cs:70–146.
- [ ] LAG_CADENCETEST source seed/math/statistics/thresholds produce PASSED and exact status formats. CadenceSelfTest.cs:21–114.
- [ ] LAG_CARRYTEST produces all20 PASS checks/ALL PASSED,including keyframe/audio/rebase/compatibility cases. CarryOverSelfTest.cs:16–147.
- [ ] LAG_STALLTEST produces all8 PASS checks/ALL PASSED with source/delayed encode/overlay/reset attribution. StallMonitorSelfTest.cs:19–104.
- [ ] LAG_GCTEST honors fixture/mode options,39600video/3audio workload,percentile/spike diagnostics and old-mode restore. GcSelfTest.cs:27–156.
- [ ] LAG_PROCTEST reports5s callback/byte/peak result and REAL AUDIO only above.0001. ProcessLoopbackCapture.cs:38–54.
- [ ] LAG_VFR_TEST honors every video/audio/overlay option and prints saved/null path/progress formats. VfrEngineTest.cs:24–130.
- [ ] LAG_VFR_CARRY rehearses target loss6s continuation and preserves footage without claiming forced target changes to desktop. VfrEngineTest.cs:110–123.
- [ ] LAG_VFR_DIAG samples16x16 source/NV12 patches with<4/<8 thresholds and reports every120hardware frames. Bgra2Nv12Converter.cs:123–180.
- [ ] LAG_WGCPROBE reports timing distributions/grid simulations,with historical mixed-clock labels kept distinct from live engine behavior. WgcTimestampProbe.cs:31–172.
- [ ] LAG_DUPPROBE counts acquired/accumulated/distinct monitor presents without recording and handles AccessLost stop. DuplicationProbe.cs:26–97.
- [ ] LAG_MFPROBE reports every audio track/default selection; separate-track clip selects All under tested Windows media stack. PlaybackTrackProbe.cs:14–44; VfrReplayEngine.cs:705–711.
- [ ] Diagnostic output includes exact component prefixes/log formats and actual Documents\Lag\Logs tee path. FileLog.cs:29–59,87–91.
- [ ] Unused Hybrid/DXGI source and dead disk-segment/FFmpegService behaviors are not exposed as replacement production capture features. App.axaml.cs:353–359; VfrReplayEngine.cs:366–388.
