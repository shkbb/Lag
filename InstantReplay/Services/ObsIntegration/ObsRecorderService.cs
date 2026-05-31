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
public sealed class ObsRecorderService : IDisposable
{
    private readonly HardwareDetector _hardwareDetector;
    private bool _isInitialized;
    private bool _isRecording;
    public bool IsRecording => _isRecording;

    private string? _libraryPath;

    /// <summary>Linear microphone volume applied to the mic source (1.0 = 100%). Set in Initialize.</summary>
    private float _micVolume = 1.0f;

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
    public void Initialize(int bufferSeconds, int frameRate, uint width, uint height, string? microphoneId = null, string? monitorId = null, string? libraryPath = null, float micVolume = 1.0f)
    {
        // Refresh the destination path so a changed library folder is honoured on every Start.
        _libraryPath = libraryPath;
        _micVolume = micVolume;

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

            // Configure raw base audio and video (matched to exact UI bounds + FPS)
            ConfigureVideoAndAudio(width, height, (uint)frameRate, microphoneId);

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

    private void ConfigureVideoAndAudio(uint width, uint height, uint fps, string? microphoneId)
    {
        uint safeWidth = width == 0 ? 1920 : width;
        uint safeHeight = height == 0 ? 1080 : height;

        // Windows MUST include the .dll extension for libobs-d3d11 to load as a plugin engine.
        string graphicsModule = "libobs-d3d11.dll";

        var ovi = new ObsVideoInfo
        {
            graphics_module = Marshal.StringToHGlobalAnsi(graphicsModule),
            fps_num = fps,
            fps_den = 1,
            base_width = safeWidth,
            base_height = safeHeight,
            output_width = safeWidth,
            output_height = safeHeight,
            output_format = 2, // VIDEO_FORMAT_NV12 (Required for DXGI/NVENC hardware texture mapping)
            colorspace = 2,    // CS_709
            range = 0,         // RANGE_PARTIAL
            adapter = 0,       // Primary GPU
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

        Debug.WriteLine($"[ObsRecorderService] Video initialized: {graphicsModule}, {safeWidth}x{safeHeight}@{fps}fps");

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

        CreateWindowsCaptureSource(settings, monitorId);

        if (_captureSource == null || _captureSource.IsInvalid)
        {
            throw new InvalidOperationException(
                "Failed to create any video capture source. " +
                "Ensure OBS win-capture plugin (monitor_capture) is available.");
        }

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
            
            // Force DXGI Desktop Duplication (method = 1)
            // WGC (method = 2) often returns black screens in headless Avalonia applications.
            ObsInterop.obs_data_set_int(monSettings, "method", 1);

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
                Console.WriteLine("[ObsRecorderService] SUCCESS: Windows monitor_capture source created and added to scene.");
            }
            else
            {
                 Console.WriteLine("[ObsRecorderService] ERROR: Created monitor_capture but failed to add it to scene!");
            }
            Console.Out.Flush();
        }
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

        // 1. Desktop Audio (Output loopback)
        using (var desktopSettings = ObsInterop.obs_data_create())
        {
            string desktopDeviceId = "default";
            ObsInterop.obs_data_set_string(desktopSettings, "device_id", desktopDeviceId);
            _desktopAudioSource = ObsInterop.obs_source_create(desktopSourceId, "DesktopAudio", desktopSettings, IntPtr.Zero);

            if (_desktopAudioSource != null && !_desktopAudioSource.IsInvalid)
            {
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
                    ObsInterop.obs_set_output_source(2, _micAudioSource); // Channel 2: Mic
                    Console.WriteLine($"[ObsRecorderService] Microphone source active: {micSourceId} (device: {resolvedMicId}, volume: {_micVolume:P0})");
                }
                else
                {
                    Console.WriteLine($"[ObsRecorderService] WARNING: Failed to create microphone source: {micSourceId}");
                }
            }
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
        // successfully (and isn't a libobs dummy) is used. No manual codec selection in the UI.
        string[] candidates = ["ffmpeg_nvenc", "ffmpeg_amf", "obs_qsv11", "obs_x264"];

        foreach (var encoderId in candidates)
        {
            using var settings = ObsInterop.obs_data_create();
            
            // Configure targeting 20 Mbps VBR
            ObsInterop.obs_data_set_string(settings, "rate_control", "VBR");
            ObsInterop.obs_data_set_int(settings, "bitrate", 20000);

            if (encoderId == "obs_x264")
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

        _audioEncoder = ObsInterop.obs_audio_encoder_create("ffmpeg_aac", "AudioEncoder", settings, 0, IntPtr.Zero);
        
        // Map the OBS core audio pipeline to the encoder
        ObsInterop.obs_encoder_set_audio(_audioEncoder, ObsInterop.obs_get_audio());
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
        ObsInterop.obs_data_set_string(settings, "extension", "mp4");

        // MP4 muxer settings: movflags=+faststart moves the moov atom to the beginning
        // of the file, which is critical for:
        //  1. Immediate playability (no need to download/read the entire file first)
        //  2. Preventing "corrupt" files when the muxer exits before full finalization
        //  3. Compatibility with media players that don't support streaming moov
        ObsInterop.obs_data_set_string(settings, "muxer_settings", "movflags=+faststart");

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

        Console.WriteLine($"[ObsRecorderService] Replay buffer configured: {bufferSeconds}s max, muxer=movflags=+faststart, saving to {videoDir}");
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
                        .Where(f => f.EndsWith(".mp4") || f.EndsWith(".mp4.tmp") || f.EndsWith(".mkv"))
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

                _captureSource?.Dispose();
                _captureSource = null;
                _desktopAudioSource?.Dispose();
                _desktopAudioSource = null;
                _micAudioSource?.Dispose();
                _micAudioSource = null;

                if (_scene != IntPtr.Zero)
                {
                    ObsInterop.obs_scene_release(_scene);
                    _scene = IntPtr.Zero;
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
