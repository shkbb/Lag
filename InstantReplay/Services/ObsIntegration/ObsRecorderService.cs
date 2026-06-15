using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading.Tasks;

namespace Lag.Services.ObsIntegration;

/// <summary>
/// Core recording engine utilizing native libobs. Replaces the FFmpeg/DXGI external process pipeline
/// with a fully integrated, zero-copy architecture. Handles safe initialization, dynamic hardware 
/// adaptation, buffer management, and game capture hooking.
/// 
/// Cross-platform:
///   Windows: D3D11 graphics, game_capture/monitor_capture, WASAPI audio, file logging
///   Linux:   OpenGL graphics, PipeWire/XSHM capture, PulseAudio, stderr logging
/// </summary>
public sealed class ObsRecorderService : Lag.Services.IReplayRecorder
{
    private readonly HardwareDetector _hardwareDetector;
    private bool _isInitialized;
    private bool _isRecording;
    public bool IsRecording => _isRecording;

    private string? _libraryPath;

    /// <summary>Linear microphone volume applied to the mic source (1.0 = 100%). Set in Initialize.</summary>
    private float _micVolume = 1.0f;

    /// <summary>User-preferred encoder id (null/empty = fully automatic fallback chain).</summary>
    private string? _preferredEncoder;

    /// <summary>Video encoder bitrate in kbps (user-configurable; default 20 Mbps).</summary>
    private int _videoBitrateKbps = 20000;

    /// <summary>DXGI adapter index used for capture/render (multi-GPU systems).</summary>
    private int _adapterIndex;

    /// <summary>Route system audio → track 1, mic → track 2 as separate tracks in the file.</summary>
    private bool _separateTracks;

    /// <summary>Downmix the microphone to mono (OBS_SOURCE_FLAG_FORCE_MONO).</summary>
    private bool _micMono;

    /// <summary>Push-to-talk: create the mic muted; unmuted only while the PTT key is held.</summary>
    private bool _micStartMuted;

    /// <summary>"all" = desktop loopback; "apps" = per-application capture sources.</summary>
    private string _audioMode = "all";

    /// <summary>Output container extension (whitelisted in Initialize; default "mp4").</summary>
    private string _fileFormat = "mp4";

    /// <summary>Per-application audio capture entries (when <see cref="_audioMode"/> is "apps").</summary>
    private IReadOnlyList<AppAudioCapture> _audioApps = [];

    /// <summary>Live per-application capture sources (released on teardown).</summary>
    private readonly List<ObsSourceHandle> _appAudioSources = new();

    /// <summary>Second AAC encoder feeding audio track 2 (mic) when separate tracks are enabled.</summary>
    private ObsEncoderHandle? _audioEncoder2;

    /// <summary>OBS source flag: force the source's audio to mono (1 &lt;&lt; 1).</summary>
    private const uint ObsSourceFlagForceMono = 1u << 1;

    // SafeHandles for unmanaged resources to prevent memory leaks during GC
    private ObsSourceHandle? _captureSource;
    private ObsEncoderHandle? _videoEncoder;
    private ObsEncoderHandle? _audioEncoder;
    private ObsOutputHandle? _replayBuffer;

    /// <summary>
    /// OBS Scene pointer — scenes manage the activate/show/hide lifecycle that
    /// PipeWire portal sources depend on for proper rendering and D-Bus session triggers.
    /// Without a scene, the source's show() callback never fires and we get black frames.
    /// </summary>
    private IntPtr _scene = IntPtr.Zero;

    /// <summary>
    /// Tracks whether we manually called inc_active/inc_showing on the capture source.
    /// Must be decremented in Dispose() to prevent libobs shutdown assertion failures.
    /// </summary>
    private bool _forcedActiveShowing;

    // Strong reference to prevent GC from collecting the unmanaged callback delegate
    private ObsInterop.signal_callback_t? _savedCallback;

    // Process-once guards for the resident libobs core. These survive Stop→Start cycles and are only
    // reset by a full Dispose (real app exit), because obs_startup/module-load may run only once per
    // process — re-running them after obs_shutdown causes access violations.
    private static bool _isObsStarted = false;
    private static bool _modulesLoaded = false;

    /// <summary>Invoked asynchronously when a replay buffer is successfully saved to disk.</summary>
    public event EventHandler<string>? ReplaySaved;

    public ObsRecorderService(HardwareDetector hardwareDetector)
    {
        _hardwareDetector = hardwareDetector;
    }

    // ────────────────────── Native Log Handler ────────────────────── //

    /// <summary>
    /// Static log handler delegate — must be kept alive for the lifetime of the OBS session.
    /// Windows: writes to obs_native.log file (existing behavior).
    /// Linux:   writes to stderr (idiomatic Unix logging).
    /// </summary>
    private static readonly ObsInterop.log_handler_t _logHandler = OnObsLog;

    private static void OnObsLog(int log_level, IntPtr msg, IntPtr args, IntPtr param)
    {
        try
        {
            var sb = new System.Text.StringBuilder(4096);
            ObsInterop.vsnprintf(sb, new UIntPtr(4096), msg, args);
            string formattedMessage = sb.ToString();

            // Route all native logs directly to terminal for real-time Windows diagnostics
            Console.WriteLine($"[LIBOBS INTERNAL] {formattedMessage}");
            Console.Out.Flush();
        }
        catch
        {
            // Log handler must NEVER throw — swallow formatting/IO errors silently.
            // A crashing log handler would take down the entire OBS pipeline.
        }
    }

    // ────────────────────── Initialization ────────────────────── //

    /// <summary>
    /// Initializes OBS natively, loads modules, detects the optimal hardware encoder,
    /// sets up the scene, and configures the replay buffer.
    ///
    /// COLD-RESTART LIFECYCLE:
    /// Every call rebuilds the FULL capture pipeline (video/audio reset → sources → encoders →
    /// replay-buffer output) from scratch, so all current UI settings — monitor, microphone, FPS,
    /// codec and buffer duration — always take effect on the next Start. There is deliberately no
    /// "warm restart" fast-path anymore; that path was silently reusing stale objects (the original
    /// stuck-duration bug).
    ///
    /// The heavy libobs CORE (obs_startup + plugin modules + graphics subsystem) is started exactly
    /// ONCE per process and kept resident. This is intentional and important: re-running obs_startup()
    /// after obs_shutdown() within the same process is NOT supported by libobs and reliably produces
    /// access violations (duplicate source/encoder type registration, graphics re-init). The pipeline
    /// is torn down on every Stop via <see cref="Teardown"/>; the core is only shut down on real app
    /// exit via <see cref="Dispose"/> (invoked by the DI container on ShutdownRequested).
    /// </summary>
    public void Initialize(int bufferSeconds, int frameRate, uint width, uint height, string? microphoneId = null, string? monitorId = null, string? libraryPath = null, float micVolume = 1.0f, uint outputWidth = 0, uint outputHeight = 0, string? preferredEncoder = null)
    {
        // Legacy convenience overload — builds a RecorderOptions snapshot with defaults.
        Initialize(new RecorderOptions
        {
            BufferSeconds = bufferSeconds,
            FrameRate = frameRate,
            Width = width,
            Height = height,
            OutputWidth = outputWidth,
            OutputHeight = outputHeight,
            MicrophoneId = microphoneId,
            MonitorId = monitorId,
            LibraryPath = libraryPath,
            MicVolume = micVolume,
            PreferredEncoder = preferredEncoder
        });
    }

    /// <summary>Initializes a recording session from a full options snapshot (the primary entry point).</summary>
    public void Initialize(RecorderOptions options)
    {
        int bufferSeconds = options.BufferSeconds;
        int frameRate = options.FrameRate;
        uint width = options.Width;
        uint height = options.Height;
        uint outputWidth = options.OutputWidth;
        uint outputHeight = options.OutputHeight;
        string? microphoneId = options.MicrophoneId;
        string? monitorId = options.MonitorId;

        // Refresh the destination path so a changed library folder is honoured on every Start.
        _libraryPath = options.LibraryPath;
        _micVolume = options.MicVolume;
        _preferredEncoder = options.PreferredEncoder;
        _videoBitrateKbps = options.VideoBitrateKbps > 0 ? options.VideoBitrateKbps : 20000;
        _adapterIndex = Math.Max(0, options.AdapterIndex);
        _separateTracks = options.SeparateAudioTracks;
        _micMono = options.MicForceMono;
        _micStartMuted = options.MicStartMuted;
        _gameCaptureEnabled = options.GameCaptureEnabled;
        _audioMode = options.AudioCaptureMode == "apps" ? "apps" : "all";
        _audioApps = options.AudioApps;
        _fileFormat = options.FileFormat is "mkv" or "mov" or "avi" ? options.FileFormat : "mp4";
        // Remembered for the game-window overlay: only windows on the RECORDED monitor may
        // become capture targets (null = primary monitor).
        _monitorDeviceId = monitorId;

        // Defensive: if a previous session was not explicitly stopped, release its pipeline first
        // so we never leak or double-attach native objects. (Core stays resident.)
        if (_isInitialized)
        {
            Console.WriteLine("[ObsRecorderService] Initialize called while a pipeline is live — tearing it down first for a clean cold restart.");
            Console.Out.Flush();
            TeardownPipeline();
        }

        try
        {
            // ── Process-once heavy core (kept resident across Stop/Start) ──
            EnsureCoreStarted();

            // ── Per-Start pipeline (rebuilt every time so new settings apply) ──

            // Configure raw base audio and video (base = native capture, output = scaled render)
            ConfigureVideoAndAudio(width, height, outputWidth, outputHeight, (uint)frameRate, microphoneId);

            // Load libobs modules (plugins, encoders, sources) — process-once, must run after the
            // first video reset, matching the original working initialization order.
            EnsureModulesLoaded();

            // Create Capture Source (picks up the selected monitor)
            CreateCaptureSource(monitorId);

            // Create Audio Sources (picks up the selected microphone)
            CreateAudioSources(microphoneId);

            // Detect and Create Optimal Hardware Video Encoder (picks up codec/resolution)
            CreateVideoEncoder(width, height);

            // Create AAC Audio Encoder
            CreateAudioEncoder();

            // Create and Configure the Replay Buffer Output (picks up buffer duration)
            CreateReplayBufferOutput(bufferSeconds);

            _isInitialized = true;
            Debug.WriteLine("[ObsRecorderService] Initialization complete.");
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[ObsRecorderService] Initialization critically failed: {ex.Message}");
            // Roll back the partially-built pipeline; leave the resident core intact.
            TeardownPipeline();
            throw;
        }
    }

    /// <summary>
    /// Starts the libobs core exactly once per process: native log handler, OS path/env setup,
    /// <c>obs_startup</c> and the global plugin data path. Guarded by <see cref="_isObsStarted"/>
    /// so it is a no-op on every warm Start after the first.
    /// </summary>
    private void EnsureCoreStarted()
    {
        if (_isObsStarted) return;

        // Clear diagnostic log on the very first startup
        if (File.Exists("obs_native.log")) File.Delete("obs_native.log");

        // Attach native logger immediately to capture core initialization panics
        ObsInterop.base_set_log_handler(_logHandler, IntPtr.Zero);
        Console.WriteLine("[ObsRecorderService] Native log handler attached.");
        Console.Out.Flush();

        InitializeWindowsPaths();

        string originalDir = Environment.CurrentDirectory;
        string obsCorePath = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "obs-core"));

        // Hack: Temporarily set CurrentDirectory to obs-core to help relative paths resolve
        Environment.CurrentDirectory = obsCorePath;

        if (!ObsInterop.obs_startup("en-US", null!, IntPtr.Zero))
        {
            Environment.CurrentDirectory = originalDir;
            throw new InvalidOperationException("OBS Init Failed at obs_startup. Check locale and module_config_path.");
        }

        // Add plugin data path globally
        string pluginDataPath = Path.GetFullPath(Path.Combine(obsCorePath, "data", "obs-plugins")).Replace("\\", "/");
        ObsInterop.obs_add_data_path(pluginDataPath);

        // Restore original directory
        Environment.CurrentDirectory = originalDir;
        _isObsStarted = true;
    }

    /// <summary>
    /// Loads the libobs plugin modules exactly once per process. Re-opening already-resident modules
    /// would re-register source/encoder types and corrupt libobs state, so this is guarded by
    /// <see cref="_modulesLoaded"/> and is a no-op on subsequent warm Starts.
    /// </summary>
    private void EnsureModulesLoaded()
    {
        if (_modulesLoaded) return;
        LoadWindowsModulesManual();
        _modulesLoaded = true;
    }

    // ────────────────────── OS-Specific Path Setup ────────────────────── //

    /// <summary>
    /// Configure local obs-core directory paths, clone shader data to
    /// the physical relative path that libobs internally expects.
    /// </summary>
    private void InitializeWindowsPaths()
    {
        string basePath = AppContext.BaseDirectory;
        string obsCorePath = Path.GetFullPath(Path.Combine(basePath, "obs-core"));

        // ── THE BRUTE FORCE ROOT COPY HACK ──
        // OBS module_file() internal pathing on Windows defaults to searching AppDomain.CurrentDomain.BaseDirectory
        // for helper executables. We programmatically copy all .exe files from obs-core into the app root.
        try
        {
            if (Directory.Exists(obsCorePath))
            {
                var exeFiles = Directory.GetFiles(obsCorePath, "*.exe", SearchOption.AllDirectories);
                foreach (var exe in exeFiles)
                {
                    string destFile = Path.Combine(basePath, Path.GetFileName(exe));
                    File.Copy(exe, destFile, overwrite: true);
                    Console.WriteLine($"[Windows Init] Brute Force Copy: {Path.GetFileName(exe)} -> {basePath}");
                }
            }
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[Windows Init] WARNING: Brute Force Copy failed: {ex.Message}");
        }

        // ── Step 0: Inject PATH for DLL Resolution ──
        // obs-ffmpeg-mux.exe (and potentially plugins) needs to find FFmpeg DLLs
        // (avformat.dll, etc.) which may reside in the base or obs-core folder.
        // Windows only searches the process PATH if they aren't in the same folder.
        string currentPath = Environment.GetEnvironmentVariable("PATH") ?? string.Empty;
        string newPath = $"{basePath};{obsCorePath};{currentPath}";
        Environment.SetEnvironmentVariable("PATH", newPath);
        Console.WriteLine("[Windows Init] Injected Base and OBS-Core directories into process PATH for FFmpeg DLL resolution.");
        Console.Out.Flush();

        // ── Step 1: Automated Muxer Resolution ──
        // Programmatically locate obs-ffmpeg-mux.exe to ensure "Save" works 
        // regardless of whether the app is running in Debug, Release, or a standalone bundle.
        string[] muxerCandidates = 
        {
            Path.Combine(obsCorePath, "obs-ffmpeg-mux.exe"),
            Path.Combine(basePath, "obs-plugins", "64bit", "obs-ffmpeg-mux.exe"),
            Path.Combine(basePath, "obs-ffmpeg-mux.exe"),
            Path.Combine(basePath, "obs-core", "obs-plugins", "obs-ffmpeg-mux.exe")
        };

        string? foundMuxerFolder = null;
        foreach (var path in muxerCandidates)
        {
            if (File.Exists(path))
            {
                foundMuxerFolder = Path.GetFullPath(Path.GetDirectoryName(path)!);
                break;
            }
        }

        if (foundMuxerFolder != null)
        {
            // libobs uses this environment variable to locate the muxer executable
            Environment.SetEnvironmentVariable("OBS_EXEC_PATH", foundMuxerFolder);
            Console.WriteLine($"[Windows Init] SUCCESS: Found muxer at '{foundMuxerFolder}'. Setting OBS_EXEC_PATH.");
            Console.Out.Flush();
        }
        else
        {
            Console.WriteLine("[Windows Init] WARNING: obs-ffmpeg-mux.exe NOT FOUND. SaveReplay will hang!");
            Console.Out.Flush();
        }

        // ── Step 2: Shader and Plugin Data Paths ──
        // obs.dll searches for shaders in "../../data/libobs" relative to its own location.
        string expectedDataPath = Path.GetFullPath(Path.Combine(obsCorePath, "..", "..", "data"));
        if (!Directory.Exists(expectedDataPath))
        {
            CopyDirectory(Path.Combine(obsCorePath, "data"), expectedDataPath);
        }

        // Use absolute paths with forward slashes to ensure C-library compatibility
        string libobsData = Path.GetFullPath(Path.Combine(obsCorePath, "data", "libobs")).Replace("\\", "/");
        ObsInterop.obs_add_data_path(libobsData);

        string pluginBinPath = Path.GetFullPath(Path.Combine(obsCorePath, "obs-plugins", "64bit")).Replace("\\", "/");
        string pluginDataPath = Path.GetFullPath(Path.Combine(obsCorePath, "data", "obs-plugins", "%module%")).Replace("\\", "/");
    }
    private void LoadWindowsModulesManual()
    {
        string basePath = AppContext.BaseDirectory;
        string obsCorePath = Path.Combine(basePath, "obs-core");

        string[] plugins = 
        { 
            "obs-ffmpeg.dll", 
            "obs-x264.dll", 
            "obs-outputs.dll", 
            "obs-filters.dll", 
            "win-capture.dll", 
            "win-wasapi.dll", 
            "obs-qsv11.dll",
            "obs-text.dll",
            "obs-transitions.dll",
            "rtmp-services.dll",
            "obs-nvenc.dll" // Ensure nvenc is loaded
        };

        // Try both structures: flat obs-plugins and 64bit subfolder
        string[] binSearchPaths = 
        {
            Path.GetFullPath(Path.Combine(obsCorePath, "obs-plugins", "64bit")),
            Path.GetFullPath(Path.Combine(obsCorePath, "obs-plugins"))
        };

        string dataBasePath = Path.GetFullPath(Path.Combine(obsCorePath, "data", "obs-plugins"));

        foreach (var plugin in plugins)
        {
            string? foundDllPath = null;
            foreach (var binPath in binSearchPaths)
            {
                string candidate = Path.Combine(binPath, plugin);
                if (File.Exists(candidate))
                {
                    foundDllPath = candidate;
                    break;
                }
            }

            if (foundDllPath != null)
            {
                string pluginNameWithoutExt = Path.GetFileNameWithoutExtension(plugin);
                string pluginDataPath = Path.Combine(dataBasePath, pluginNameWithoutExt);

                if (plugin == "obs-outputs.dll")
                {
                    try
                    {
                        Directory.CreateDirectory(pluginDataPath);
                        string sourceMuxer = Path.Combine(obsCorePath, "obs-ffmpeg-mux.exe");
                        if (!File.Exists(sourceMuxer))
                        {
                            sourceMuxer = Path.Combine(obsCorePath, "obs-plugins", "64bit", "obs-ffmpeg-mux.exe");
                        }
                        
                        if (File.Exists(sourceMuxer))
                        {
                            string destMuxer = Path.Combine(pluginDataPath, "obs-ffmpeg-mux.exe");
                            File.Copy(sourceMuxer, destMuxer, overwrite: true);
                            Console.WriteLine($"[Windows Modules] obs-outputs Hack: Copied muxer to {destMuxer}");
                        }
                    }
                    catch (Exception ex)
                    {
                        Console.WriteLine($"[Windows Modules] WARNING: obs-outputs muxer copy hack failed: {ex.Message}");
                    }
                }

                // Use absolute paths with forward slashes for C-library compatibility
                string safeDllPath = foundDllPath.Replace("\\", "/");
                string safeDataPath = pluginDataPath.Replace("\\", "/");

                int result = ObsInterop.obs_open_module(out IntPtr module, safeDllPath, safeDataPath);
                if (result == 0) // MODULE_SUCCESS
                {
                    ObsInterop.obs_init_module(module);
                    Console.WriteLine($"[Windows Modules] SUCCESS: Loaded {plugin}");
                }
                else
                {
                    Console.WriteLine($"[Windows Modules] FAILED: obs_open_module returned {result} for {plugin}");
                }
            }
            else
            {
                Console.WriteLine($"[Windows Modules] WARNING: DLL not found for {plugin}");
            }
        }

        ObsInterop.obs_post_load_modules();
        Console.Out.Flush();
    }



    // ────────────────────── Video & Audio Configuration ────────────────────── //

    private void ConfigureVideoAndAudio(uint width, uint height, uint outputWidth, uint outputHeight, uint fps, string? microphoneId)
    {
        uint safeWidth = width == 0 ? 1920 : width;
        uint safeHeight = height == 0 ? 1080 : height;

        // Remember the canvas size: the game_capture overlay stretches hooked frames to it.
        _baseWidth = safeWidth;
        _baseHeight = safeHeight;

        // Output (render/encode) resolution: defaults to native; when the user picks a downscale
        // preset (1080p/720p) we render smaller to save GPU encode cost and disk space.
        // base_* stays NATIVE so the capture is never cropped — libobs scales base → output
        // with the bicubic scaler (scale_type below). Encoders require even dimensions.
        uint safeOutW = outputWidth == 0 ? safeWidth : Math.Min(outputWidth, safeWidth) & ~1u;
        uint safeOutH = outputHeight == 0 ? safeHeight : Math.Min(outputHeight, safeHeight) & ~1u;
        if (safeOutW == 0 || safeOutH == 0) { safeOutW = safeWidth; safeOutH = safeHeight; }

        // Windows MUST include the .dll extension for libobs-d3d11 to load as a plugin engine.
        string graphicsModule = "libobs-d3d11.dll";

        var ovi = new ObsVideoInfo
        {
            graphics_module = Marshal.StringToHGlobalAnsi(graphicsModule),
            fps_num = fps,
            fps_den = 1,
            base_width = safeWidth,
            base_height = safeHeight,
            output_width = safeOutW,
            output_height = safeOutH,
            output_format = 2, // VIDEO_FORMAT_NV12 (Required for DXGI/NVENC hardware texture mapping)
            colorspace = 2,    // CS_709
            range = 0,         // RANGE_PARTIAL
            adapter = (uint)_adapterIndex, // User-selected GPU (0 = primary)
            gpu_conversion = true,
            scale_type = 2     // SCALE_BICUBIC
        };

        // Windows: verify the D3D11 rendering engine exists on disk
        if (!File.Exists("libobs-d3d11.dll"))
        {
            throw new FileNotFoundException(
                "OBS Init Failed: libobs-d3d11.dll rendering engine is missing from the working directory.");
        }

        // Multi-stage fallback pipeline for video initialization
        int resetVideoCode = ObsInterop.obs_reset_video(ref ovi);

        if (resetVideoCode != 0)
        {
            // Fallback 1: Same graphics module with I420 pixel format (higher compatibility)
            Debug.WriteLine($"[ObsRecorderService] obs_reset_video failed ({resetVideoCode}) with NV12. Trying I420...");
            ovi.output_format = 2; // Keep NV12, as DXGI Duplication and NVENC strictly require it
            resetVideoCode = ObsInterop.obs_reset_video(ref ovi);

            if (resetVideoCode != 0)
            {
                // Fallback 2 (Windows only): Try OpenGL as a last resort
                Debug.WriteLine("[ObsRecorderService] D3D11 exhausted. Trying OpenGL fallback...");
                ovi.graphics_module = Marshal.StringToHGlobalAnsi("libobs-opengl.dll");
                resetVideoCode = ObsInterop.obs_reset_video(ref ovi);
            }

            if (resetVideoCode != 0)
            {
                throw new InvalidOperationException(
                    $"OBS Init Failed at obs_reset_video (Code: {resetVideoCode}). " +
                    $"All D3D11/OpenGL and pixel format fallbacks exhausted.");
            }
        }

        Debug.WriteLine($"[ObsRecorderService] Video initialized: {graphicsModule}, base {safeWidth}x{safeHeight} → output {safeOutW}x{safeOutH} @{fps}fps");

        var oai = new ObsAudioInfo
        {
            samples_per_sec = 48000,
            speakers = 2 // SPEAKERS_STEREO
        };

        if (!ObsInterop.obs_reset_audio(ref oai))
            throw new InvalidOperationException("OBS Init Failed at obs_reset_audio.");
    }

    // ────────────────────── Capture Source Creation (Scene-Based) ────────────────────── //

    /// <summary>
    /// Creates the video capture source and wraps it in an OBS Scene.
    /// 
    /// WHY A SCENE IS REQUIRED:
    /// PipeWire portal sources implement show/hide/activate callbacks that are only triggered
    /// when the source is part of a scene that is set as the active output. Direct assignment
    /// via obs_set_output_source(0, rawSource) bypasses this lifecycle, causing the D-Bus
    /// portal session to never open the PipeWire stream → black frames.
    /// 
    /// The scene approach is DE/WM agnostic — it relies solely on the universal xdg-desktop-portal
    /// interface, working identically on GNOME, KDE Plasma, Sway, Hyprland, and any other
    /// Wayland compositor with a portal backend.
    /// </summary>
    private void CreateCaptureSource(string? monitorId)
    {
        // ── Step 1: Initialize the Scene ──
        // We create the scene first because on Windows, we now unconditionally add sources
        // directly from the platform-specific initialization method.
        _scene = ObsInterop.obs_scene_create("LagScene");
        if (_scene == IntPtr.Zero)
        {
            throw new InvalidOperationException("Failed to create OBS scene. Pipeline cannot continue.");
        }

        using var settings = ObsInterop.obs_data_create();

        // Both WGC sources are created, but only ONE captures at a time: running monitor +
        // window WGC simultaneously throttles each to ~half the frame rate (measured: a game
        // that should record 120 fps came out at 43–69 unique fps with both active, 87–120
        // with just one). So the monitor capture is HIDDEN whenever the window overlay locks
        // onto a fullscreen game (it would be fully covered anyway), and shown again on the
        // desktop. The toggle lives in RetargetGameWindowOverlay via the stored scene items.
        CreateWindowsCaptureSource(settings, monitorId);
        if (_captureSource == null || _captureSource.IsInvalid)
        {
            throw new InvalidOperationException(
                "Failed to create any video capture source. " +
                "Ensure OBS win-capture plugin (monitor_capture) is available.");
        }

        // WGC window overlay ON TOP of the monitor feed (added later = rendered above):
        // captures the fullscreen game's own swapchain at full rate, which monitor capture
        // cannot do past independent flip. Renders nothing until a game is targeted.
        CreateGameWindowOverlay();

        // Hook-based game_capture overlay on the very top. Opt-in: hook retries against
        // anti-tamper games (CS2 Trusted Mode) burn CPU, so it stays off by default.
        if (_gameCaptureEnabled)
            CreateGameCaptureOverlay();

        // ── Step 2: Establish the output pipeline ──
        // Assign the SCENE's source to channel 0.
        IntPtr sceneSource = ObsInterop.obs_scene_get_source(_scene);
        
        if (sceneSource != IntPtr.Zero)
        {
            ObsInterop.obs_set_output_source(0, sceneSource);
            Console.WriteLine("[ObsRecorderService] SUCCESS: Scene firmly connected to video output Channel 0.");
        }
        else
        {
             Console.WriteLine("[ObsRecorderService] ERROR: Failed to resolve scene source for output connection.");
        }
        Console.Out.Flush();
    }

    /// <summary>
    /// Windows: Unconditionally creates a monitor_capture source and adds it to the scene.
    /// This fixes the "No Video" issue where game_capture would return a handle but no frames.
    /// </summary>
    private void CreateWindowsCaptureSource(ObsDataHandle settings, string? monitorId)
    {
        Console.WriteLine("[ObsRecorderService] Attempting to create Windows monitor_capture...");
        Console.Out.Flush();

        using (var monSettings = ObsInterop.obs_data_create())
        {
            if (!string.IsNullOrEmpty(monitorId))
            {
                // Modern win-capture prioritizes monitor_id
                ObsInterop.obs_data_set_string(monSettings, "monitor_id", monitorId);
                Console.WriteLine($"[ObsRecorderService] Injecting specific monitor ID: {monitorId}");
            }
            else
            {
                // monitor = 0 represents the primary monitor in win-capture
                ObsInterop.obs_data_set_int(monSettings, "monitor", 0);
                ObsInterop.obs_data_set_int(monSettings, "monitor_index", 0);
                
                // Fallback for modern win-capture that prefers monitor_id
                ObsInterop.obs_data_set_string(monSettings, "monitor_id", @"\\.\DISPLAY1");
            }
            
            // Windows Graphics Capture (method = 2): unlike DXGI duplication, WGC keeps
            // delivering frames while a fullscreen game holds independent flip (DXGI starves
            // to ~2 FPS and the replay flush stalls until alt-tab). Requires libobs-winrt.dll,
            // which win-capture os_dlopen()s at module load — see AddDllDirectory in App.axaml.cs;
            // without it the historical "WGC = black screen" failure was actually a silent
            // fallback caused by the missing DLL. win-capture still falls back to DXGI on its
            // own when WGC is unsupported, so this is safe on older Windows builds.
            ObsInterop.obs_data_set_int(monSettings, "method", 2);

            _captureSource = ObsInterop.obs_source_create("monitor_capture", "MainMonitorCapture", monSettings, IntPtr.Zero);
            
            if (_captureSource != null && !_captureSource.IsInvalid)
            {
                // Force an immediate update so settings apply before the source starts ticking
                ObsInterop.obs_source_update(_captureSource, monSettings);
            }
        }

        if (_captureSource == null || _captureSource.IsInvalid)
        {
            Console.WriteLine("[ObsRecorderService] FAILED to create monitor_capture!");
            Console.Out.Flush();
        }
        else
        {
            // Unconditionally add to scene. Even if game_capture were created later,
            // monitor_capture ensures we have a valid baseline video feed.
            IntPtr sceneItem = ObsInterop.obs_scene_add(_scene, _captureSource);
            if (sceneItem != IntPtr.Zero)
            {
                // Remember the item so the overlay can hide it (stop its WGC session) while a
                // game window is captured — see RetargetGameWindowOverlay.
                _monitorSceneItem = sceneItem;
                Console.WriteLine("[ObsRecorderService] SUCCESS: Windows monitor_capture source created and added to scene.");
            }
            else
            {
                 Console.WriteLine("[ObsRecorderService] ERROR: Created monitor_capture but failed to add it to scene!");
            }
            Console.Out.Flush();
        }
    }

    // ────────────────────── Game Capture Overlay ────────────────────── //

    private ObsSourceHandle? _gameCaptureSource;

    /// <summary>Whether the opt-in game_capture hook overlay is enabled for this session.</summary>
    private bool _gameCaptureEnabled;

    /// <summary>Canvas (base) size, remembered for the game-capture stretch bounds.</summary>
    private uint _baseWidth = 1920;
    private uint _baseHeight = 1080;

    /// <summary>
    /// Adds a game_capture source ABOVE monitor_capture in the scene.
    ///
    /// Why: DXGI desktop duplication only delivers frames when the DESKTOP composition
    /// updates. When a fullscreen game enters independent flip, the desktop barely
    /// changes — replays degrade to 1–2 FPS and the buffer flush stalls until alt-tab.
    /// game_capture hooks the game's swapchain directly (graphics-hook64.dll from
    /// obs-core, same win-capture module), so hooked frames arrive at full rate.
    /// While no game is hooked the source renders nothing and the monitor capture
    /// underneath remains the fallback. Failure here is non-fatal by design.
    /// </summary>
    private void CreateGameCaptureOverlay()
    {
        try
        {
            using var gcSettings = ObsInterop.obs_data_create();
            // Auto-hook whatever fullscreen/borderless game is in the foreground.
            ObsInterop.obs_data_set_string(gcSettings, "capture_mode", "any_fullscreen");
            ObsInterop.obs_data_set_bool(gcSettings, "capture_cursor", true);

            _gameCaptureSource = ObsInterop.obs_source_create(
                "game_capture", "GameCaptureOverlay", gcSettings, IntPtr.Zero);

            if (_gameCaptureSource == null || _gameCaptureSource.IsInvalid)
            {
                Console.WriteLine("[ObsRecorderService] game_capture unavailable — monitor capture only.");
                _gameCaptureSource = null;
                return;
            }

            IntPtr item = ObsInterop.obs_scene_add(_scene, _gameCaptureSource);
            if (item == IntPtr.Zero)
            {
                Console.WriteLine("[ObsRecorderService] Failed to add game_capture to the scene.");
                _gameCaptureSource.Dispose();
                _gameCaptureSource = null;
                return;
            }

            // Stretch hooked frames to the full canvas: stretched-res games (e.g. 4:3 CS2
            // on a widescreen) deliver their native backbuffer, and the recording must
            // match what the player actually sees on the display.
            var bounds = new ObsInterop.ObsVec2 { x = _baseWidth, y = _baseHeight };
            ObsInterop.obs_sceneitem_set_bounds_type(item, 1); // OBS_BOUNDS_STRETCH
            ObsInterop.obs_sceneitem_set_bounds_alignment(item, 0); // centered
            ObsInterop.obs_sceneitem_set_bounds(item, ref bounds);

            Console.WriteLine("[ObsRecorderService] SUCCESS: game_capture overlay added (any_fullscreen, stretched to canvas).");
            Console.Out.Flush();
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[ObsRecorderService] game_capture overlay failed (non-fatal): {ex.Message}");
        }
    }

    // ────────────────────── WGC Game-Window Overlay ────────────────────── //

    private ObsSourceHandle? _gameWindowSource;
    private System.Threading.Timer? _gameWindowTimer;
    private string _gameWindowTarget = "";

    /// <summary>The game window we are LOCKED onto, held through alt-tabs (Medal-style).</summary>
    private IntPtr _lockedGameHwnd = IntPtr.Zero;
    private uint _lockedGamePid;

    /// <summary>Monitor-capture scene item, hidden while a game window is captured (single-WGC).</summary>
    private IntPtr _monitorSceneItem = IntPtr.Zero;

    /// <summary>
    /// Processes that are never games — we must not lock the game overlay onto them, or the
    /// capture would thrash (and restart its WGC session → ~0.5s freeze) every time the user
    /// alt-tabs to a browser/chat during a loading screen. Lowercase, with .exe.
    /// </summary>
    private static readonly HashSet<string> NonGameProcesses = new(StringComparer.OrdinalIgnoreCase)
    {
        "explorer.exe", "searchhost.exe", "shellexperiencehost.exe", "startmenuexperiencehost.exe",
        "applicationframehost.exe", "textinputhost.exe", "lockapp.exe", "dwm.exe",
        "firefox.exe", "chrome.exe", "msedge.exe", "opera.exe", "operagx.exe", "brave.exe", "vivaldi.exe",
        "telegram.exe", "discord.exe", "slack.exe", "teams.exe", "whatsapp.exe", "viber.exe",
        "snippingtool.exe", "screenclippinghost.exe", "notepad.exe", "code.exe", "devenv.exe",
        "obs64.exe", "lag.exe", "spotify.exe", "vlc.exe", "mpc-hc64.exe", "wmplayer.exe",
    };

    /// <summary>Device id of the recorded monitor (e.g. @"\\.\DISPLAY1"); null = primary.</summary>
    private string? _monitorDeviceId;

    [DllImport("user32.dll")] private static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] private static extern bool IsWindow(IntPtr hWnd);
    [DllImport("user32.dll")] private static extern bool GetWindowRect(IntPtr hWnd, out Win32Rect rect);
    [DllImport("user32.dll")] private static extern IntPtr MonitorFromWindow(IntPtr hWnd, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern bool GetMonitorInfoW(IntPtr hMonitor, ref MonitorInfoEx info);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowTextW(IntPtr hWnd, System.Text.StringBuilder text, int maxCount);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetClassNameW(IntPtr hWnd, System.Text.StringBuilder text, int maxCount);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

    [StructLayout(LayoutKind.Sequential)]
    private struct Win32Rect { public int Left, Top, Right, Bottom; }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct MonitorInfoEx
    {
        public uint Size;
        public Win32Rect Monitor;
        public Win32Rect Work;
        public uint Flags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)]
        public string Device;
    }

    /// <summary>
    /// True when the window is a fullscreen surface ON THE RECORDED MONITOR: its rect covers
    /// the monitor it sits on, and that monitor is the one the user selected for recording
    /// (or the primary one when no explicit selection exists). The monitor check is what
    /// keeps a fullscreen Discord/video on a SECOND display from hijacking the overlay.
    /// </summary>
    private bool IsFullscreenOnRecordedMonitor(IntPtr hwnd)
    {
        if (!GetWindowRect(hwnd, out var rect)) return false;

        IntPtr mon = MonitorFromWindow(hwnd, 2 /* MONITOR_DEFAULTTONEAREST */);
        var info = new MonitorInfoEx { Size = (uint)Marshal.SizeOf<MonitorInfoEx>() };
        if (!GetMonitorInfoW(mon, ref info)) return false;

        bool onRecordedMonitor = string.IsNullOrEmpty(_monitorDeviceId)
            ? (info.Flags & 1) != 0 // MONITORINFOF_PRIMARY
            : string.Equals(info.Device, _monitorDeviceId, StringComparison.OrdinalIgnoreCase);
        if (!onRecordedMonitor) return false;

        return rect.Left <= info.Monitor.Left && rect.Top <= info.Monitor.Top
            && rect.Right >= info.Monitor.Right && rect.Bottom >= info.Monitor.Bottom;
    }

    /// <summary>
    /// Adds a window_capture source (Windows Graphics Capture, method 2) ABOVE the monitor
    /// feed and keeps it pointed at whatever fullscreen window is in the foreground.
    ///
    /// Why this exists: MONITOR capture — DXGI duplication AND monitor-WGC alike — only sees
    /// the desktop COMPOSITION. A fullscreen game presents past the compositor (independent
    /// flip / MPO), so DWM re-composes mostly when the cursor moves and the captured "new"
    /// frames carry stale game content (measured: 70%+ visually frozen frames in CS2 fights
    /// while every OBS pipeline stat looked clean). WGC window capture reads the WINDOW's own
    /// swapchain — full rate regardless of flip mode, no hooks, no Trusted Mode conflicts.
    /// This is exactly how Game Bar records fullscreen games.
    ///
    /// While no fullscreen window is targeted the source renders nothing and the monitor
    /// capture underneath remains the fallback. Failure here is non-fatal by design.
    /// </summary>
    private void CreateGameWindowOverlay()
    {
        try
        {
            using var settings = ObsInterop.obs_data_create();
            ObsInterop.obs_data_set_string(settings, "window", "");
            ObsInterop.obs_data_set_int(settings, "method", 2);   // WGC
            ObsInterop.obs_data_set_int(settings, "priority", 2); // match by executable
            ObsInterop.obs_data_set_bool(settings, "cursor", true);

            _gameWindowSource = ObsInterop.obs_source_create(
                "window_capture", "GameWindowOverlay", settings, IntPtr.Zero);

            if (_gameWindowSource == null || _gameWindowSource.IsInvalid)
            {
                Console.WriteLine("[ObsRecorderService] window_capture unavailable — monitor capture only.");
                _gameWindowSource = null;
                return;
            }

            IntPtr item = ObsInterop.obs_scene_add(_scene, _gameWindowSource);
            if (item == IntPtr.Zero)
            {
                Console.WriteLine("[ObsRecorderService] Failed to add window_capture to the scene.");
                _gameWindowSource.Dispose();
                _gameWindowSource = null;
                return;
            }

            // Stretch to the full canvas — stretched-res games (4:3 CS2 on ultrawide) must
            // record the way the player sees them, same rationale as the game_capture overlay.
            var bounds = new ObsInterop.ObsVec2 { x = _baseWidth, y = _baseHeight };
            ObsInterop.obs_sceneitem_set_bounds_type(item, 1); // OBS_BOUNDS_STRETCH
            ObsInterop.obs_sceneitem_set_bounds_alignment(item, 0);
            ObsInterop.obs_sceneitem_set_bounds(item, ref bounds);

            _gameWindowTarget = "";
            _gameWindowTimer = new System.Threading.Timer(_ => EvaluateGameCapture(), null, 1000, 2000);

            Console.WriteLine("[ObsRecorderService] SUCCESS: WGC window overlay added (locks onto a game, Medal-style).");
            Console.Out.Flush();
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[ObsRecorderService] WGC window overlay failed (non-fatal): {ex.Message}");
        }
    }

    /// <summary>
    /// Timer callback — Medal-style game lock. Once a real game is detected fullscreen on the
    /// recorded monitor, we LOCK the window capture onto it and HOLD that lock as long as the
    /// game process is alive — regardless of foreground focus. So when the user alt-tabs to a
    /// browser/chat during a loading screen, we keep capturing the game window (a borderless
    /// game keeps rendering in the background) instead of thrashing the WGC session over to
    /// the new foreground window (which restarted capture and caused ~0.5s freezes). Only when
    /// the game exits do we release the lock and fall back to the desktop monitor feed.
    /// </summary>
    private void EvaluateGameCapture()
    {
        var source = _gameWindowSource;
        if (source == null || source.IsInvalid) return;

        try
        {
            // 1) Locked onto a game? Hold it while its window exists and its process lives —
            //    NO foreground check, NO source churn. This is what survives alt-tabs.
            if (_lockedGameHwnd != IntPtr.Zero)
            {
                if (IsWindow(_lockedGameHwnd) && IsProcessAlive(_lockedGamePid))
                    return; // steady state — do nothing, capture keeps flowing

                // Game closed → release the lock and restore the desktop monitor feed.
                _lockedGameHwnd = IntPtr.Zero;
                _lockedGamePid = 0;
                _gameWindowTarget = "";
                UpdateGameWindowSetting(source, "");
                if (_monitorSceneItem != IntPtr.Zero)
                    ObsInterop.obs_sceneitem_set_visible(_monitorSceneItem, true);
                Console.WriteLine("[ObsRecorderService] Game closed — released lock, monitor capture restored.");
                Console.Out.Flush();
                // fall through and look for another game this same tick
            }

            // 2) Not locked. Look for a game to lock onto: the foreground window, when it is
            //    fullscreen on the recorded monitor and is NOT a known non-game process.
            IntPtr hwnd = GetForegroundWindow();
            if (hwnd == IntPtr.Zero || !IsFullscreenOnRecordedMonitor(hwnd)) return;

            GetWindowThreadProcessId(hwnd, out uint pid);
            if (pid == 0 || pid == (uint)Environment.ProcessId) return;

            string exe;
            try { exe = System.Diagnostics.Process.GetProcessById((int)pid).ProcessName + ".exe"; }
            catch { return; }
            if (NonGameProcesses.Contains(exe)) return; // browser/chat/shell/tool — never lock

            var titleSb = new System.Text.StringBuilder(256);
            GetWindowTextW(hwnd, titleSb, titleSb.Capacity);
            var classSb = new System.Text.StringBuilder(256);
            GetClassNameW(hwnd, classSb, classSb.Capacity);

            // OBS window id format: "Title:Class:Exe", '#' → "#23", ':' → "#3A".
            static string Esc(string s) => s.Replace("#", "#23").Replace(":", "#3A");
            string target = $"{Esc(titleSb.ToString())}:{Esc(classSb.ToString())}:{Esc(exe)}";

            _lockedGameHwnd = hwnd;
            _lockedGamePid = pid;
            _gameWindowTarget = target;
            UpdateGameWindowSetting(source, target);
            // Single-WGC: hide the redundant monitor capture (the game covers the canvas).
            if (_monitorSceneItem != IntPtr.Zero)
                ObsInterop.obs_sceneitem_set_visible(_monitorSceneItem, false);
            Console.WriteLine($"[ObsRecorderService] Locked onto game → {target} (held through alt-tabs)");
            Console.Out.Flush();
        }
        catch
        {
            // Detection must never take down the pipeline; worst case the monitor feed shows.
        }
    }

    /// <summary>Lightweight liveness check for a captured game process.</summary>
    private static bool IsProcessAlive(uint pid)
    {
        if (pid == 0) return false;
        try { using var p = System.Diagnostics.Process.GetProcessById((int)pid); return !p.HasExited; }
        catch { return false; }
    }

    private static void UpdateGameWindowSetting(ObsSourceHandle source, string target)
    {
        using var settings = ObsInterop.obs_data_create();
        ObsInterop.obs_data_set_string(settings, "window", target);
        ObsInterop.obs_data_set_int(settings, "method", 2);
        ObsInterop.obs_data_set_int(settings, "priority", 2);
        ObsInterop.obs_data_set_bool(settings, "cursor", true);
        ObsInterop.obs_source_update(source, settings);
    }

    // ────────────────────── Audio Source Creation ────────────────────── //

    private ObsSourceHandle? _desktopAudioSource;
    private ObsSourceHandle? _micAudioSource;

    /// <summary>
    /// Creates audio capture sources for desktop output and optional microphone input.
    /// Windows: WASAPI (wasapi_output_capture / wasapi_input_capture) with "default" device_id.
    /// </summary>
    private void CreateAudioSources(string? microphoneId)
    {
        string desktopSourceId = "wasapi_output_capture";
        string micSourceId = "wasapi_input_capture";

        // Track routing masks (only applied when separate-tracks mode is on):
        // system audio → track 1 (bit 0), microphone → track 2 (bit 1).
        const uint SystemTrackMask = 0x1;
        const uint MicTrackMask = 0x2;

        // 1. System audio — either the whole desktop loopback, or per-application capture.
        if (_audioMode == "apps" && _audioApps.Count > 0)
        {
            // Per-application capture (Medal-style). One wasapi_process_output_capture per app,
            // matched by executable name. Sources live in the SCENE (not output channels), which
            // sidesteps the 6-channel limit and lets each one carry its own volume.
            foreach (var app in _audioApps)
            {
                using var appSettings = ObsInterop.obs_data_create();
                // "window" format is "Title:Class:Executable"; priority 2 = match by executable.
                ObsInterop.obs_data_set_string(appSettings, "window", $"::{app.ExeName}");
                ObsInterop.obs_data_set_int(appSettings, "priority", 2);

                var appSource = ObsInterop.obs_source_create(
                    "wasapi_process_output_capture", $"AppAudio_{app.ExeName}", appSettings, IntPtr.Zero);

                if (appSource != null && !appSource.IsInvalid)
                {
                    ObsInterop.obs_source_set_volume(appSource, Math.Clamp(app.Volume, 0f, 1f));
                    if (_separateTracks)
                        ObsInterop.obs_source_set_audio_mixers(appSource, SystemTrackMask);

                    ObsInterop.obs_scene_add(_scene, appSource);
                    _appAudioSources.Add(appSource);
                    Console.WriteLine($"[ObsRecorderService] App audio source active: {app.ExeName} (volume: {app.Volume:P0})");
                }
                else
                {
                    Console.WriteLine($"[ObsRecorderService] WARNING: Failed to create app audio source for {app.ExeName}");
                }
            }
        }
        else
        {
            // Whole-desktop loopback (default).
            using var desktopSettings = ObsInterop.obs_data_create();
            string desktopDeviceId = "default";
            ObsInterop.obs_data_set_string(desktopSettings, "device_id", desktopDeviceId);
            _desktopAudioSource = ObsInterop.obs_source_create(desktopSourceId, "DesktopAudio", desktopSettings, IntPtr.Zero);

            if (_desktopAudioSource != null && !_desktopAudioSource.IsInvalid)
            {
                if (_separateTracks)
                    ObsInterop.obs_source_set_audio_mixers(_desktopAudioSource, SystemTrackMask);

                ObsInterop.obs_set_output_source(1, _desktopAudioSource); // Channel 1: Desktop
                Console.WriteLine($"[ObsRecorderService] Desktop audio source active: {desktopSourceId} (device: {desktopDeviceId})");
            }
            else
            {
                Console.WriteLine($"[ObsRecorderService] WARNING: Failed to create desktop audio source: {desktopSourceId}");
            }
        }

        // 2. Microphone Audio (Input)
        {
            string resolvedMicId = microphoneId ?? string.Empty;

            if (!string.IsNullOrEmpty(resolvedMicId))
            {
                using var micSettings = ObsInterop.obs_data_create();
                ObsInterop.obs_data_set_string(micSettings, "device_id", resolvedMicId);
                _micAudioSource = ObsInterop.obs_source_create(micSourceId, "Microphone", micSettings, IntPtr.Zero);

                if (_micAudioSource != null && !_micAudioSource.IsInvalid)
                {
                    // Apply the user-configured microphone volume (linear, 1.0 = 100%).
                    ObsInterop.obs_source_set_volume(_micAudioSource, _micVolume);

                    // Mono downmix on request.
                    if (_micMono)
                        ObsInterop.obs_source_set_flags(_micAudioSource, ObsSourceFlagForceMono);

                    // Route the mic to its own track when separate tracks are enabled.
                    if (_separateTracks)
                        ObsInterop.obs_source_set_audio_mixers(_micAudioSource, MicTrackMask);

                    // Push-to-talk: start muted; the PTT key handler unmutes while held.
                    if (_micStartMuted)
                        ObsInterop.obs_source_set_muted(_micAudioSource, true);

                    ObsInterop.obs_set_output_source(2, _micAudioSource); // Channel 2: Mic
                    Console.WriteLine($"[ObsRecorderService] Microphone source active: {micSourceId} (device: {resolvedMicId}, volume: {_micVolume:P0}, mono: {_micMono}, ptt-muted: {_micStartMuted})");
                }
                else
                {
                    Console.WriteLine($"[ObsRecorderService] WARNING: Failed to create microphone source: {micSourceId}");
                }
            }
        }
    }

    /// <summary>
    /// Mutes/unmutes the microphone source at runtime (push-to-talk). Safe to call from any
    /// thread and while not recording (no-op when the mic source doesn't exist).
    /// </summary>
    public void SetMicMuted(bool muted)
    {
        try
        {
            if (_micAudioSource != null && !_micAudioSource.IsInvalid)
                ObsInterop.obs_source_set_muted(_micAudioSource, muted);
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[ObsRecorderService] SetMicMuted failed: {ex.Message}");
        }
    }

    // ────────────────────── Encoder Setup ────────────────────── //

    /// <summary>
    /// Implements a dynamic hardware-agnostic encoder fallback loop.
    /// Prioritises hardware (NVIDIA -> AMD -> Intel) and falls back to software (x264).
    /// Rejects "dummy" encoders that libobs creates when a driver is missing.
    /// </summary>
    private void CreateVideoEncoder(uint width, uint height)
    {
        // Automatic encoder fallback: NVIDIA → AMD → Intel → CPU. The first one that creates
        // successfully (and isn't a libobs dummy) is used. If the user explicitly picked a codec
        // in Settings, it is tried FIRST; the automatic chain stays as a safety net so recording
        // still works when the chosen encoder isn't available on this machine.
        //
        // CRITICAL ordering — obs_nvenc_h264_tex (and its alias jim_nvenc) FIRST:
        // these are the TEXTURE-based NVENC encoders. They take the frame as a GPU texture and
        // hand it straight to NVENC with zero copies. ffmpeg_nvenc (and the *_cuda/*_soft
        // variants) instead round-trip the frame GPU→system RAM→GPU every frame. Under a
        // game that saturates the GPU, that round-trip stalls and OBS drops frames it can't
        // hand off in time — measured 78% "skipped due to encoding lag" at native and STILL
        // 39% after halving resolution, because the bottleneck was the copy path, not pixels.
        // The texture path is what OBS, ShadowPlay and Medal all use to record games smoothly.
        // AV1 texture encoder FIRST on RTX 40-series. Counter-intuitively, Ada's AV1 NVENC is
        // FASTER than its H.264 path here: measured encoding lag was 11% on AV1 p1 vs 18-45%
        // on H.264, and the user confirmed AV1 looked clearly smoother (H.264 was worse in
        // every window). AV1 also gives the better quality Medal's clips have. It auto-creates
        // only on AV1-capable GPUs; older cards fall through to the H.264 texture encoder.
        string[] autoChain = ["obs_nvenc_av1_tex", "obs_nvenc_h264_tex", "jim_nvenc", "ffmpeg_nvenc", "ffmpeg_amf", "obs_qsv11", "obs_x264"];

        var candidates = new List<string>();
        if (!string.IsNullOrEmpty(_preferredEncoder))
        {
            // Saved settings store "ffmpeg_nvenc" for the NVENC choice — expand it to AV1 first
            // (faster + better on Ada), then H.264 texture as fallback.
            if (_preferredEncoder == "ffmpeg_nvenc")
            {
                candidates.Add("obs_nvenc_av1_tex");
                candidates.Add("obs_nvenc_h264_tex");
                candidates.Add("jim_nvenc");
            }
            candidates.Add(_preferredEncoder);
            Console.WriteLine($"[Encoder Fallback] User preference: trying '{_preferredEncoder}' first.");
        }
        foreach (var id in autoChain)
        {
            if (!candidates.Contains(id)) candidates.Add(id);
        }

        foreach (var encoderId in candidates)
        {
            using var settings = ObsInterop.obs_data_create();

            // VBR at the user-configured bitrate (kbps), with 2x headroom for motion spikes —
            // plain VBR without a max starves fast scenes and they come out pixelated.
            ObsInterop.obs_data_set_string(settings, "rate_control", "VBR");
            ObsInterop.obs_data_set_int(settings, "bitrate", _videoBitrateKbps);
            ObsInterop.obs_data_set_int(settings, "max_bitrate", _videoBitrateKbps * 2);
            // Keyframe every 2s: replay trims/saves start on a keyframe, and the default
            // (often 250 frames) makes the first seconds of a clip mushy after seeking.
            ObsInterop.obs_data_set_int(settings, "keyint_sec", 2);

            bool isNvencH264 = encoderId is "obs_nvenc_h264_tex" or "jim_nvenc" or "obs_nvenc_h264" or "ffmpeg_nvenc";
            bool isNvencAv1 = encoderId is "obs_nvenc_av1_tex";
            if (isNvencH264 || isNvencAv1)
            {
                // Native obs-nvenc knobs (unknown keys are ignored by other encoders).
                // FASTEST preset (p1): we force CFR 120, so the encoder must finish a frame in
                // <8.3ms even while the game pegs the GPU. At p5 (default) AV1 couldn't and
                // dropped 32% of frames → stutter. p1 keeps NVENC well inside budget so every
                // canvas frame is encoded → smooth. At 26 Mbps the p1 quality hit is minor.
                // Set "preset" too (legacy key) in case the texture encoder reads that one.
                ObsInterop.obs_data_set_string(settings, "preset2", "p1");
                ObsInterop.obs_data_set_string(settings, "preset", "p1");
                ObsInterop.obs_data_set_string(settings, "multipass", "disabled");
                ObsInterop.obs_data_set_bool(settings, "adaptive_quantization", true);
                // Strip the heavy extras NVENC enables by default. Lookahead (8 frames) buffers
                // ahead before encoding and B-frames add reordering — together they spiked
                // encoding lag to 45.7% under heavy combat (encoder couldn't finish in time →
                // dropped frames → stutter). Medal records "Constrained Baseline": no B-frames,
                // no lookahead. At our bitrate the compression loss is invisible, and the encode
                // becomes light enough to keep up at 120fps while the game owns the GPU.
                ObsInterop.obs_data_set_int(settings, "bf", 0);
                ObsInterop.obs_data_set_bool(settings, "lookahead", false);
                // "profile" is an H.264 concept (high/main/baseline); AV1 has no such key.
                if (isNvencH264)
                {
                    ObsInterop.obs_data_set_string(settings, "tune", "hq");
                    ObsInterop.obs_data_set_string(settings, "profile", "high");
                }
            }
            else if (encoderId == "obs_x264")
            {
                ObsInterop.obs_data_set_string(settings, "preset", "veryfast");
            }

            Console.WriteLine($"[Encoder Fallback] Attempting to create: {encoderId}");
            var handle = ObsInterop.obs_video_encoder_create(encoderId, "VideoEncoder", settings, IntPtr.Zero);
            
            if (handle == null || handle.IsInvalid)
            {
                Console.WriteLine($"[Encoder Fallback] {encoderId} creation failed.");
                continue;
            }

            // Reject "dummy" objects (libobs fallback for missing plugins)
            IntPtr props = ObsInterop.obs_encoder_properties(handle);
            if (props == IntPtr.Zero)
            {
                Console.WriteLine($"[Encoder Fallback] {encoderId} is an unsupported dummy object. Skipping.");
                handle.Dispose();
                continue;
            }
            ObsInterop.obs_properties_destroy(props);

            // Successfully found a working encoder
            _videoEncoder = handle;
            ObsInterop.obs_encoder_set_video(_videoEncoder, ObsInterop.obs_get_video());
            Console.WriteLine($"[Encoder Fallback] SUCCESS: Selected {encoderId} as target encoder.");
            return;
        }

        throw new NotSupportedException("CRITICAL: No supported video encoders found on this system, including the software fallback.");
    }

    private void CreateAudioEncoder()
    {
        using var settings = ObsInterop.obs_data_create();
        ObsInterop.obs_data_set_int(settings, "bitrate", 192);

        // Track 1 (system audio — or the full mix when separate tracks are off).
        _audioEncoder = ObsInterop.obs_audio_encoder_create("ffmpeg_aac", "AudioEncoder", settings, 0, IntPtr.Zero);

        // Map the OBS core audio pipeline to the encoder
        ObsInterop.obs_encoder_set_audio(_audioEncoder, ObsInterop.obs_get_audio());

        // Track 2 (microphone) — only when the user wants separate tracks in the file.
        if (_separateTracks)
        {
            using var settings2 = ObsInterop.obs_data_create();
            ObsInterop.obs_data_set_int(settings2, "bitrate", 192);

            _audioEncoder2 = ObsInterop.obs_audio_encoder_create("ffmpeg_aac", "AudioEncoderMic", settings2, 1, IntPtr.Zero);
            ObsInterop.obs_encoder_set_audio(_audioEncoder2, ObsInterop.obs_get_audio());
        }
    }

    // ────────────────────── Replay Buffer ────────────────────── //

    private void CreateReplayBufferOutput(int bufferSeconds)
    {
        using var settings = ObsInterop.obs_data_create();
        
        ObsInterop.obs_data_set_int(settings, "max_time_sec", bufferSeconds);
        // Explicitly restore the max_size_mb override so long buffers don't get truncated by RAM limits
        ObsInterop.obs_data_set_int(settings, "max_size_mb", 8192);

        string videoDir = ResolveReplayDirectory();
        // Use the injected path
        ObsInterop.obs_data_set_string(settings, "directory", videoDir);
        ObsInterop.obs_data_set_string(settings, "format", "%CCYY-%MM-%DD_%hh-%mm-%ss-Replay");
        ObsInterop.obs_data_set_string(settings, "extension", _fileFormat);

        // NO movflags=+faststart here: faststart makes the muxer rewrite the whole file a
        // second time to move the moov atom to the front. Under full game load that doubles
        // the hotkey-to-saved latency (measured ~10s for a 60 MB replay in CS2). Local
        // playback doesn't need faststart — it only matters for progressive HTTP streaming.
        // Editor exports keep faststart (EditorViewModel) since they are not time-critical.

        // Explicit muxer path override to fix ERROR_FILE_NOT_FOUND (code 2)
        string muxerFullPath = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "obs-core", "obs-ffmpeg-mux.exe")).Replace("\\", "/");
        ObsInterop.obs_data_set_string(settings, "muxer_path", muxerFullPath);

        _replayBuffer = ObsInterop.obs_output_create("replay_buffer", "ReplayBuffer", settings, IntPtr.Zero);

        if (_replayBuffer == null || _replayBuffer.IsInvalid)
        {
            throw new InvalidOperationException("Failed to create OBS replay buffer output.");
        }

        // Link the encoders to the output destination
        ObsInterop.obs_output_set_video_encoder(_replayBuffer, _videoEncoder!);
        ObsInterop.obs_output_set_audio_encoder(_replayBuffer, _audioEncoder!, 0);

        // Second audio track (microphone) when separate tracks are enabled.
        if (_separateTracks && _audioEncoder2 != null && !_audioEncoder2.IsInvalid)
        {
            ObsInterop.obs_output_set_audio_encoder(_replayBuffer, _audioEncoder2, 1);
            Console.WriteLine("[ObsRecorderService] Separate audio tracks: mic routed to track 2.");
        }

        // Connect native signal handler to harvest physical MP4 path on completion
        IntPtr handler = ObsInterop.obs_output_get_signal_handler(_replayBuffer);
        if (handler != IntPtr.Zero)
        {
            _savedCallback = OnBufferSaved;
            ObsInterop.signal_handler_connect(handler, "saved", _savedCallback, IntPtr.Zero);
            Console.WriteLine("[ObsRecorderService] Signal handler 'saved' connected for replay path extraction.");
        }
        else
        {
            Console.WriteLine("[ObsRecorderService] WARNING: Could not get signal handler — saved event will not fire.");
        }

        Console.WriteLine($"[ObsRecorderService] Replay buffer configured: {bufferSeconds}s max, format={_fileFormat}, saving to {videoDir}");
        Console.Out.Flush();
    }

    /// <summary>
    /// Resolves and forcefully creates the replay output directory.
    /// Windows: <c>%UserProfile%\Lag</c> (avoids localized "Videos" folder names).
    /// </summary>
    private string ResolveReplayDirectory()
    {
        string replayDir = !string.IsNullOrWhiteSpace(_libraryPath) 
            ? _libraryPath 
            : Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), "Lag");

        Directory.CreateDirectory(replayDir);
        Debug.WriteLine($"[ObsRecorderService] Replay directory ensured: {replayDir}");

        return replayDir;
    }

    // ────────────────────── Buffer Control ────────────────────── //

    /// <summary>
    /// Starts the OBS replay buffer output. The ring buffer begins accumulating frames.
    /// Includes diagnostic error extraction on failure via obs_output_get_last_error.
    /// </summary>
    public void StartBuffer()
    {
        if (!_isInitialized || _isRecording || _replayBuffer == null) return;

        if (!ObsInterop.obs_output_start(_replayBuffer))
        {
            // Extract the native error message for precise failure diagnosis
            string errorDetail = "No error details available";
            try
            {
                IntPtr errorPtr = ObsInterop.obs_output_get_last_error(_replayBuffer);
                if (errorPtr != IntPtr.Zero)
                {
                    errorDetail = Marshal.PtrToStringAnsi(errorPtr) ?? errorDetail;
                }
            }
            catch { /* obs_output_get_last_error itself failed — use default message */ }

            Console.WriteLine($"[ObsRecorderService] obs_output_start FAILED: {errorDetail}");
            Console.Out.Flush();
            throw new InvalidOperationException($"Failed to start the OBS replay buffer: {errorDetail}");
        }

        _isRecording = true;
        Console.WriteLine("[ObsRecorderService] Replay buffer started successfully.");
        Console.Out.Flush();
    }

    /// <summary>
    /// Gracefully stops the replay buffer output without shutting down the OBS context.
    /// This allows for "warm" restarts where the engine stays ready for the next recording.
    /// Wrapped in try-catch to prevent application crashes — a failed stop is non-fatal.
    /// </summary>
    public void StopBuffer()
    {
        if (!_isRecording || _replayBuffer == null || _replayBuffer.IsInvalid) return;

        try
        {
            Console.WriteLine("[ObsRecorderService] Stopping replay buffer output...");
            Console.Out.Flush();

            ObsInterop.obs_output_stop(_replayBuffer);
            _isRecording = false;

            // Give the background muxer thread a fraction of a second to cleanly detach
            // before we potentially start destroying resources.
            Thread.Sleep(500);

            Console.WriteLine("[ObsRecorderService] Replay buffer output stopped.");
            Console.Out.Flush();
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[ObsRecorderService] StopBuffer error (non-fatal, forcing state reset): {ex.Message}");
            Console.Out.Flush();
            _isRecording = false;
        }
    }

    /// <summary>
    /// Triggers the active replay buffer to flush the last X minutes to disk instantly.
    /// Uses the OBS proc handler ("save" action), not the signal handler ("saved" event).
    ///
    /// Includes 5 diagnostic steps to pinpoint exactly where the pipeline fails:
    ///   Step 1: Validate muxer exists where libobs resolves it (Linux: next to host process)
    ///   Step 2: Validate target directory exists
    ///   Step 3: Verify write permissions via dummy file
    ///   Step 4: Trigger OBS save via proc_handler_call
    ///   Step 5: Post-save async scan for output .mp4 file
    /// </summary>
    public void SaveReplay()
    {
        Console.WriteLine(">>> SAVE REPLAY METHOD ENTERED <<<");
        Console.Out.Flush();

        try
        {
            if (!_isRecording || _replayBuffer == null) return;

            // ── Step 1: Validate muxer path from OBS_EXEC_PATH ──
            string? execPath = Environment.GetEnvironmentVariable("OBS_EXEC_PATH");
            Console.WriteLine($"[SaveReplay Step 1] OBS_EXEC_PATH = '{execPath}'");
            Console.Out.Flush();
            if (!string.IsNullOrEmpty(execPath))
            {
                string muxerExeName = "obs-ffmpeg-mux.exe";
                string muxerFullPath = Path.Combine(execPath, muxerExeName);
                bool muxerExists = File.Exists(muxerFullPath);
                Console.WriteLine($"[SaveReplay Step 1] {muxerExeName} exists at '{muxerFullPath}': {muxerExists}");
                Console.Out.Flush();
                if (!muxerExists)
                {
                    throw new FileNotFoundException(
                        $"[SaveReplay] FATAL: {muxerExeName} not found at '{muxerFullPath}'. Cannot save replay.");
                }
            }
            else
            {
                Console.WriteLine("[SaveReplay Step 1] OBS_EXEC_PATH not set; relying on default module layout.");
                Console.Out.Flush();
            }

            // ── Step 2: Validate Target Directory ──
            string replayDir = ResolveReplayDirectory();
            bool dirExists = Directory.Exists(replayDir);
            Console.WriteLine($"[SaveReplay Step 2] Target directory '{replayDir}' exists: {dirExists}");
            Console.Out.Flush();
            if (!dirExists)
            {
                throw new DirectoryNotFoundException(
                    $"[SaveReplay] FATAL: Replay directory '{replayDir}' does not exist even after CreateDirectory.");
            }

            // ── Step 3: Check Write Permissions ──
            string testFile = Path.Combine(replayDir, ".write_test_" + Guid.NewGuid().ToString("N")[..8] + ".tmp");
            try
            {
                File.WriteAllText(testFile, "write_test");
                File.Delete(testFile);
                Console.WriteLine($"[SaveReplay Step 3] Write permission verified in '{replayDir}'.");
                Console.Out.Flush();
            }
            catch (Exception ex)
            {
                throw new UnauthorizedAccessException(
                    $"[SaveReplay] FATAL: Cannot write to '{replayDir}': {ex.Message}", ex);
            }

            // ── Step 4: Trigger OBS Save ──
            IntPtr procHandler = ObsInterop.obs_output_get_proc_handler(_replayBuffer);
            if (procHandler == IntPtr.Zero)
            {
                throw new InvalidOperationException("[SaveReplay Step 4] FATAL: proc_handler is null — replay buffer was not properly initialized.");
            }

            // DIAGNOSTIC: Check if any frames have actually reached the output before saving
            int totalFrames = ObsInterop.obs_output_get_total_frames(_replayBuffer);
            Console.WriteLine($"[SaveReplay Step 4] Total video frames in buffer: {totalFrames}");
            Console.Out.Flush();

            if (totalFrames == 0)
            {
                Console.WriteLine("[SaveReplay Step 4] WARNING: Zero frames in buffer! The capture source may not be producing video.");
                Console.Out.Flush();
            }

            // Allocate calldata_t for proc_handler_call("save").
            // calldata_t layout on x64 (OBS 30+):
            //   offset  0: uint8_t *stack    (8 bytes) — pointer to data stack
            //   offset  8: size_t   size     (8 bytes) — current used size
            //   offset 16: size_t   capacity (8 bytes) — allocated capacity
            //   offset 24: bool     fixed    (1 byte + 7 padding)
            // Total: 32 bytes.
            //
            // The "save" proc in replay-buffer does NOT write output to calldata (it's fire-and-forget).
            // The file path comes through the "saved" SIGNAL callback, not the proc return.
            // So a zeroed block (equivalent to calldata_init's memset) is correct here.
            const int CalldataSize = 32;
            IntPtr cd = Marshal.AllocHGlobal(CalldataSize);
            try
            {
                // Zero-initialize: equivalent to calldata_init() which is just memset(0)
                byte[] zeroes = new byte[CalldataSize];
                Marshal.Copy(zeroes, 0, cd, CalldataSize);

                Console.WriteLine("[SaveReplay Step 4] Sending 'save' command to proc_handler...");
                Console.Out.Flush();

                ObsInterop.proc_handler_call(procHandler, "save", cd);

                Console.WriteLine("[SaveReplay Step 4] Command sent successfully. Muxer pipeline triggered.");
                Console.Out.Flush();

                // Check for immediate errors after save trigger
                try
                {
                    IntPtr lastError = ObsInterop.obs_output_get_last_error(_replayBuffer);
                    if (lastError != IntPtr.Zero)
                    {
                        string errMsg = Marshal.PtrToStringAnsi(lastError) ?? string.Empty;
                        if (!string.IsNullOrEmpty(errMsg))
                        {
                            Console.WriteLine($"[SaveReplay Step 4] OBS output last error: {errMsg}");
                            Console.Out.Flush();
                        }
                    }
                }
                catch { /* obs_output_get_last_error failed — non-fatal */ }
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[SaveReplay Step 4 ERROR] proc_handler_call failed: {ex.Message}");
                Console.Out.Flush();
                throw;
            }
            finally
            {
                // Safe to free: the "save" proc does not retain or reallocate the calldata stack.
                Marshal.FreeHGlobal(cd);
            }

            // ── Step 5: Post-Save Verification (async, non-blocking) ──
            string scanDir = replayDir;
            _ = Task.Run(async () =>
            {
                Console.WriteLine("[SaveReplay Step 5] Waiting 5 seconds for muxer to finish...");
                Console.Out.Flush();
                await Task.Delay(5000);

                try
                {
                    var recentFiles = Directory.GetFiles(scanDir, "*.*")
                        .Where(f => f.EndsWith(".mp4") || f.EndsWith(".mp4.tmp") || f.EndsWith(".mkv")
                                 || f.EndsWith(".mov") || f.EndsWith(".avi"))
                        .Select(f => new FileInfo(f))
                        .Where(fi => fi.LastWriteTime > DateTime.Now.AddSeconds(-10))
                        .ToList();

                    if (recentFiles.Count > 0)
                    {
                        foreach (var fi in recentFiles)
                        {
                            Console.WriteLine($"[SaveReplay Step 5] ✓ FOUND: {fi.Name} ({fi.Length / 1024} KB, modified {fi.LastWriteTime:HH:mm:ss})");
                            Console.Out.Flush();
                        }
                    }
                    else
                    {
                        Console.WriteLine($"[SaveReplay Step 5] ✗ NO recent video files found in '{scanDir}'.");
                        Console.Out.Flush();
                        Console.WriteLine($"[SaveReplay Step 5] All files in directory:");
                        Console.Out.Flush();
                        foreach (string f in Directory.GetFiles(scanDir))
                        {
                            var fi = new FileInfo(f);
                            Console.WriteLine($"  - {fi.Name} ({fi.Length / 1024} KB, {fi.LastWriteTime:HH:mm:ss})");
                            Console.Out.Flush();
                        }
                    }
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[SaveReplay Step 5] Scan error: {ex.Message}");
                    Console.Out.Flush();
                }
            });
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[FATAL CRASH IN SAVE] {ex}");
            Console.Out.Flush();
        }
    }

    /// <summary>
    /// COLD-RESTART stop: tears down the recording PIPELINE (output → encoders → sources → scene)
    /// while leaving the libobs CORE resident. Called on every Stop so the next Start runs a clean
    /// <see cref="Initialize"/> that re-reads all settings (monitor, mic, FPS, codec, buffer length).
    ///
    /// Crash-safety guarantees:
    ///   • The 'saved' signal is disconnected BEFORE the output is freed, so a late async muxer
    ///     completion can never fire into released memory (access violation).
    ///   • Output channels are cleared (set to NULL) before the sources are released, dropping
    ///     libobs's channel references first so capture stops cleanly while idle and our handle
    ///     release actually frees the object.
    ///   • Idempotent: every handle is nulled after disposal, so a second call (e.g. Stop followed
    ///     by app-exit Dispose) is a safe no-op and never double-frees.
    /// </summary>
    public void Teardown()
    {
        Console.WriteLine(">>> PIPELINE TEARDOWN ENTERED (core stays resident) <<<");
        Console.Out.Flush();

        try
        {
            // First stop the output if it's currently active
            StopBuffer();

            // CRITICAL: Disconnect signal handlers BEFORE disposing the output.
            // If a muxer finishes asynchronously after teardown, the "saved" callback
            // would otherwise fire into a freed object → access violation crash.
            if (_replayBuffer != null && !_replayBuffer.IsInvalid && _savedCallback != null)
            {
                try
                {
                    IntPtr handler = ObsInterop.obs_output_get_signal_handler(_replayBuffer);
                    if (handler != IntPtr.Zero)
                    {
                        ObsInterop.signal_handler_disconnect(handler, "saved", _savedCallback, IntPtr.Zero);
                        Console.WriteLine("[Teardown Step 1b] Signal handler 'saved' disconnected.");
                        Console.Out.Flush();
                    }
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"[Teardown Step 1b WARNING] Signal disconnect failed: {ex.Message}");
                    Console.Out.Flush();
                }
            }
            _savedCallback = null;

            Console.WriteLine("[Teardown Step 2] Destroying output...");
            Console.Out.Flush();
            try
            {
                _replayBuffer?.Dispose();
                _replayBuffer = null;
                Console.WriteLine("[Teardown Step 2] Replay Buffer disposed.");
                Console.Out.Flush();
            }
            catch (Exception ex) { Console.WriteLine($"[Teardown Step 2 ERROR] {ex.Message}"); Console.Out.Flush(); }

            Console.WriteLine("[Teardown Step 3] Destroying encoders...");
            Console.Out.Flush();
            try
            {
                _videoEncoder?.Dispose();
                _videoEncoder = null;
                _audioEncoder?.Dispose();
                _audioEncoder = null;
                _audioEncoder2?.Dispose();
                _audioEncoder2 = null;
                Console.WriteLine("[Teardown Step 3] Encoders disposed.");
                Console.Out.Flush();
            }
            catch (Exception ex) { Console.WriteLine($"[Teardown Step 3 ERROR] {ex.Message}"); Console.Out.Flush(); }

            Console.WriteLine("[Teardown Step 4] Destroying sources...");
            Console.Out.Flush();
            try
            {
                // Clear the output channels FIRST so libobs drops its references to our sources.
                // This stops the monitor/audio capture from ticking while idle and ensures the
                // handle Dispose() calls below actually drive the refcount to zero.
                ObsInterop.obs_set_output_source(0, IntPtr.Zero); // video / scene
                ObsInterop.obs_set_output_source(1, IntPtr.Zero); // desktop audio
                ObsInterop.obs_set_output_source(2, IntPtr.Zero); // microphone

                // CRITICAL: Decrement refcounts for capture source before disposal
                if (_forcedActiveShowing && _captureSource != null && !_captureSource.IsInvalid)
                {
                    ObsInterop.obs_source_dec_showing(_captureSource);
                    ObsInterop.obs_source_dec_active(_captureSource);
                    _forcedActiveShowing = false;
                    Console.WriteLine("[Teardown Step 4] Forced refcounts released.");
                    Console.Out.Flush();
                }

                // Stop retargeting BEFORE the source dies so the timer never touches a
                // disposed handle.
                _gameWindowTimer?.Dispose();
                _gameWindowTimer = null;
                _gameWindowSource?.Dispose();
                _gameWindowSource = null;
                _lockedGameHwnd = IntPtr.Zero;
                _lockedGamePid = 0;
                _gameWindowTarget = "";

                _gameCaptureSource?.Dispose();
                _gameCaptureSource = null;
                _captureSource?.Dispose();
                _captureSource = null;
                _desktopAudioSource?.Dispose();
                _desktopAudioSource = null;
                _micAudioSource?.Dispose();
                _micAudioSource = null;

                // Per-application audio sources (scene release below drops the scene's refs).
                foreach (var appSource in _appAudioSources)
                {
                    try { appSource.Dispose(); } catch { /* non-fatal */ }
                }
                _appAudioSources.Clear();

                if (_scene != IntPtr.Zero)
                {
                    ObsInterop.obs_scene_release(_scene);
                    _scene = IntPtr.Zero;
                    // Scene items die with the scene — drop the dangling pointer.
                    _monitorSceneItem = IntPtr.Zero;
                    Console.WriteLine("[Teardown Step 4] Scene released.");
                    Console.Out.Flush();
                }
                Console.WriteLine("[Teardown Step 4] Sources and Scene released.");
                Console.Out.Flush();
            }
            catch (Exception ex) { Console.WriteLine($"[Teardown Step 4 ERROR] {ex.Message}"); Console.Out.Flush(); }

            _isInitialized = false;
            Console.WriteLine(">>> PIPELINE TEARDOWN COMPLETE <<<");
            Console.Out.Flush();
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[FATAL CRASH IN TEARDOWN] {ex}");
            Console.Out.Flush();
            // Force state to "stopped" so the UI/engine can never get wedged after a failed teardown.
            _isInitialized = false;
            _isRecording = false;
        }
    }

    /// <summary>
    /// Internal alias kept for the <see cref="Initialize"/> defensive/rollback paths.
    /// </summary>
    private void TeardownPipeline() => Teardown();

    /// <summary>
    /// FULL teardown for real application exit (invoked by the DI container on ShutdownRequested).
    /// Releases the pipeline via <see cref="Teardown"/>, then shuts the libobs core down with
    /// <c>obs_shutdown</c>. This is the ONLY place obs_shutdown is called — Stop never does, because
    /// re-running obs_startup afterwards in the same process is unsupported and crash-prone.
    /// </summary>
    public void Dispose()
    {
        Console.WriteLine(">>> FULL TEARDOWN (DISPOSE) ENTERED <<<");
        Console.Out.Flush();

        try
        {
            // Release the pipeline (idempotent — safe even if already torn down by Stop).
            Teardown();

            Console.WriteLine("[Teardown Step 5] Releasing OBS core context...");
            Console.Out.Flush();
            if (_isObsStarted)
            {
                try
                {
                    ObsInterop.obs_shutdown();
                    _isObsStarted = false;
                    _modulesLoaded = false;
                    Console.WriteLine("[Teardown Step 5] libobs shutdown complete.");
                    Console.Out.Flush();
                }
                catch (Exception ex) { Console.WriteLine($"[Teardown Step 5 ERROR] {ex.Message}"); Console.Out.Flush(); }
            }

            // Detach the native log handler last so shutdown messages are still captured.
            try { ObsInterop.base_set_log_handler(null!, IntPtr.Zero); } catch { /* non-fatal */ }
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[FATAL CRASH IN DISPOSE] {ex}");
            Console.Out.Flush();
        }
    }

    // ────────────────────── Signal Handlers ────────────────────── //

    /// <summary>
    /// Called by the native OBS signal handler when the replay buffer finishes writing.
    /// 
    /// OBS 30+: calldata_get_string IS exported as a real function (not inline macro),
    /// so we can now extract the output file path directly from the signal calldata.
    /// The "path" key contains the absolute path to the muxed MP4 file.
    /// </summary>
    private void OnBufferSaved(IntPtr data, IntPtr calldata)
    {
        string outputPath = string.Empty;

        try
        {
            // Extract the file path from the calldata ("path" key set by replay-buffer after muxing)
            if (calldata != IntPtr.Zero &&
                ObsInterop.calldata_get_string(calldata, "path", out IntPtr pathPtr) &&
                pathPtr != IntPtr.Zero)
            {
                outputPath = Marshal.PtrToStringAnsi(pathPtr) ?? string.Empty;
            }
        }
        catch (EntryPointNotFoundException)
        {
            // Fallback: calldata_get_string not exported in this OBS build (pre-30).
            // The Step 5 post-save scan in SaveReplay() will locate the file independently.
            Console.WriteLine("[ObsRecorderService] calldata_get_string not available in this OBS build. Path will be resolved via directory scan.");
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[ObsRecorderService] Failed to read path from calldata: {ex.Message}");
        }

        if (!string.IsNullOrEmpty(outputPath))
        {
            Console.WriteLine($"[ObsRecorderService] ✓ Replay saved to: {outputPath}");
        }
        else
        {
            Console.WriteLine("[ObsRecorderService] ✓ Replay buffer saved (path will be resolved via directory scan).");
        }
        Console.Out.Flush();

        Task.Run(() => ReplaySaved?.Invoke(this, outputPath));
    }

    // ────────────────────── Helpers ────────────────────── //

    private static void CopyDirectory(string sourceDir, string destinationDir)
    {
        var dir = new DirectoryInfo(sourceDir);
        if (!dir.Exists) return;

        DirectoryInfo[] dirs = dir.GetDirectories();
        Directory.CreateDirectory(destinationDir);

        foreach (FileInfo file in dir.GetFiles())
        {
            file.CopyTo(Path.Combine(destinationDir, file.Name), true);
        }

        foreach (DirectoryInfo subDir in dirs)
        {
            CopyDirectory(subDir.FullName, Path.Combine(destinationDir, subDir.Name));
        }
    }
}
