using System.Reflection;
using System.Runtime.InteropServices;

namespace Lag.Services.ObsIntegration;

/// <summary>
/// Safe memory management wrapper for OBS objects. Ensures obs_xxx_release is always called,
/// preventing memory leaks and crashes when the C# GC cleans up unmanaged pointers.
/// </summary>
public abstract class ObsSafeHandle : SafeHandle
{
    protected ObsSafeHandle() : base(IntPtr.Zero, true) { }
    protected ObsSafeHandle(IntPtr invalidHandleValue, bool ownsHandle) : base(invalidHandleValue, ownsHandle) { }
    public override bool IsInvalid => handle == IntPtr.Zero;
}

public sealed class ObsSourceHandle : ObsSafeHandle
{
    protected override bool ReleaseHandle()
    {
        ObsInterop.obs_source_release(handle);
        return true;
    }
}

public sealed class ObsOutputHandle : ObsSafeHandle
{
    protected override bool ReleaseHandle()
    {
        ObsInterop.obs_output_release(handle);
        return true;
    }
}

public sealed class ObsEncoderHandle : ObsSafeHandle
{
    protected override bool ReleaseHandle()
    {
        ObsInterop.obs_encoder_release(handle);
        return true;
    }
}

public sealed class ObsDataHandle : ObsSafeHandle
{
    protected override bool ReleaseHandle()
    {
        ObsInterop.obs_data_release(handle);
        return true;
    }
}

/// <summary>
/// Core DllImports for native libobs integration.
/// Uses NativeLibrary.SetDllImportResolver for cross-platform library resolution:
///   Windows: obs.dll (resolved from local obs-core/ via SetDllDirectory in App.axaml.cs)
///   Linux:   libobs.so.0 (system-wide via pacman)
/// </summary>
public static class ObsInterop
{
    private const string ObsDll = "obs";

    /// <summary>
    /// Registers the cross-platform DLL import resolver on first access.
    /// Intercepts all P/Invoke calls from this assembly and redirects library
    /// names based on the runtime OS.
    /// </summary>
    static ObsInterop()
    {
        NativeLibrary.SetDllImportResolver(typeof(ObsInterop).Assembly, ResolveNativeLibrary);
    }

    private static IntPtr ResolveNativeLibrary(string libraryName, Assembly assembly, DllImportSearchPath? searchPath)
    {
        // ── libobs resolution ──
        if (libraryName == ObsDll && RuntimeInformation.IsOSPlatform(OSPlatform.Linux))
        {
            // System-wide pacman install: /usr/lib/libobs.so.0
            if (NativeLibrary.TryLoad("libobs.so.0", assembly, searchPath, out var obsHandle))
                return obsHandle;
            if (NativeLibrary.TryLoad("libobs.so", assembly, searchPath, out obsHandle))
                return obsHandle;
        }

        // ── C runtime resolution (for vsnprintf log formatting) ──
        if (libraryName == "msvcrt.dll" && RuntimeInformation.IsOSPlatform(OSPlatform.Linux))
        {
            // glibc provides vsnprintf with identical C calling convention
            if (NativeLibrary.TryLoad("libc.so.6", assembly, searchPath, out var libcHandle))
                return libcHandle;
            if (NativeLibrary.TryLoad("libc", assembly, searchPath, out libcHandle))
                return libcHandle;
        }

        // Return IntPtr.Zero → fall back to default OS resolution strategy
        return IntPtr.Zero;
    }

    [DllImport("libglib-2.0.so.0", CallingConvention = CallingConvention.Cdecl)]
    public static extern bool g_main_context_iteration(IntPtr context, bool may_block);

    // ────────────────────── Core Lifecycle ────────────────────── //

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern bool obs_startup(string locale, string module_config_path, IntPtr profiler_name_store);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_shutdown();

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern void obs_add_data_path(string path);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern void obs_add_module_path(string bin, string data);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_load_all_modules();

    // ── Explicit Module Loading (for allowlist strategy on Linux) ──

    /// <summary>
    /// Opens a specific OBS module by path without initializing it.
    /// Returns 0 (MODULE_SUCCESS) on success.
    /// The module handle is output via the 'module' out parameter.
    /// </summary>
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern int obs_open_module(out IntPtr module, string path, string data_path);

    /// <summary>
    /// Initializes a previously opened module. Must be called after obs_open_module.
    /// Returns true if the module initialized successfully.
    /// </summary>
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    [return: MarshalAs(UnmanagedType.I1)]
    public static extern bool obs_init_module(IntPtr module);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_post_load_modules();

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern int obs_reset_video(ref ObsVideoInfo ovi);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    [return: MarshalAs(UnmanagedType.I1)]
    public static extern bool obs_reset_audio(ref ObsAudioInfo oai);

    // Audio/Video Setup
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_set_output_source(uint channel, ObsSourceHandle source);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_set_output_source(uint channel, IntPtr source);

    // ────────────────────── Logging ────────────────────── //
    
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    public delegate void log_handler_t(int log_level, IntPtr msg, IntPtr args, IntPtr param);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void base_set_log_handler(log_handler_t handler, IntPtr param);

    /// <summary>
    /// C runtime vsnprintf for formatting OBS va_list log messages.
    /// On Windows: resolves to msvcrt.dll natively.
    /// On Linux:   the DllImportResolver transparently redirects to libc.so.6 (glibc),
    ///             which exports vsnprintf with identical C calling convention.
    /// </summary>
    [DllImport("msvcrt.dll", CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern int vsnprintf(System.Text.StringBuilder buffer, UIntPtr count, IntPtr format, IntPtr args);

    // ────────────────────── Data Objects (C-struct) ────────────────────── //

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern ObsDataHandle obs_data_create();

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern void obs_data_set_int(ObsDataHandle data, string name, long val);

    // The VALUE is marshalled as UTF-8 (libobs expects strict UTF-8). This fixes corrupted output
    // when the replay directory / file path contains Cyrillic (or any non-ASCII) characters.
    // The key name stays ASCII, which is a valid UTF-8 subset.
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern void obs_data_set_string(ObsDataHandle data, string name, [MarshalAs(UnmanagedType.LPUTF8Str)] string val);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern void obs_data_set_bool(ObsDataHandle data, string name, bool val);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_data_release(IntPtr data);

    // Sources (Game Capture / Monitor Capture / PipeWire / XSHM)
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern ObsSourceHandle obs_source_create(string id, string name, ObsDataHandle settings, IntPtr hotkey_data);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_source_release(IntPtr source);

    // Sets the linear volume of a source (1.0 = 100%, 0.0 = muted). Used for mic volume control.
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_source_set_volume(ObsSourceHandle source, float volume);

    // Mutes/unmutes a source at the mixer level. Used for push-to-talk.
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_source_set_muted(ObsSourceHandle source, [MarshalAs(UnmanagedType.I1)] bool muted);

    // Bitmask of audio tracks (mixers) this source feeds: bit0 = track 1, bit1 = track 2, ...
    // Used to route system audio → track 1 and microphone → track 2 for separate-track recording.
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_source_set_audio_mixers(ObsSourceHandle source, uint mixers);

    // Source behaviour flags. OBS_SOURCE_FLAG_FORCE_MONO (1<<1) downmixes the source to mono.
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_source_set_flags(ObsSourceHandle source, uint flags);

    // Encoders (NVENC, AMF, VAAPI, x264)
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern ObsEncoderHandle obs_video_encoder_create(string id, string name, ObsDataHandle settings, IntPtr hotkey_data);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern ObsEncoderHandle obs_audio_encoder_create(string id, string name, ObsDataHandle settings, int mixer_idx, IntPtr hotkey_data);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_encoder_set_video(ObsEncoderHandle encoder, IntPtr obs_video_t);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern IntPtr obs_get_video();

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_encoder_set_audio(ObsEncoderHandle encoder, IntPtr obs_audio_t);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern IntPtr obs_get_audio();

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_encoder_release(IntPtr encoder);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern IntPtr obs_encoder_properties(ObsEncoderHandle encoder);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_properties_destroy(IntPtr properties);

    // Outputs (Replay Buffer)
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern ObsOutputHandle obs_output_create(string id, string name, ObsDataHandle settings, IntPtr hotkey_data);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_output_set_video_encoder(ObsOutputHandle output, ObsEncoderHandle encoder);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_output_set_audio_encoder(ObsOutputHandle output, ObsEncoderHandle encoder, int idx);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern bool obs_output_start(ObsOutputHandle output);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_output_stop(ObsOutputHandle output);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_output_release(IntPtr output);

    /// <summary>
    /// Retrieves the total number of frames that have been sent to this output.
    /// Useful for diagnosing frame starvation or capture failures.
    /// </summary>
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern int obs_output_get_total_frames(ObsOutputHandle output);

    // ────────────────────── Scene Management ────────────────────── //

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern IntPtr obs_scene_create(string name);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_scene_release(IntPtr scene);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern IntPtr obs_scene_add(IntPtr scene, ObsSourceHandle source);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern IntPtr obs_scene_get_source(IntPtr scene);

    /// <summary>2D vector used by scene-item transforms (struct vec2 in libobs).</summary>
    [StructLayout(LayoutKind.Sequential)]
    public struct ObsVec2
    {
        public float x;
        public float y;
    }

    // Scene-item bounds: used to stretch the game_capture overlay to the full canvas
    // (obs_bounds_type: 0 = NONE, 1 = STRETCH, 2 = SCALE_INNER, ...).
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_sceneitem_set_bounds_type(IntPtr item, int type);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_sceneitem_set_bounds(IntPtr item, ref ObsVec2 bounds);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_sceneitem_set_bounds_alignment(IntPtr item, uint alignment);

    /// <summary>
    /// Shows/hides a scene item. A hidden item is not rendered and, when it is the only
    /// reference to its source in the active program scene, OBS marks the source inactive —
    /// which stops a WGC capture's frame pool. Used to disable the redundant monitor capture
    /// while a game window is being captured, so only ONE WGC session runs at a time.
    /// </summary>
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_sceneitem_set_visible(IntPtr item, [MarshalAs(UnmanagedType.I1)] bool visible);

    // ────────────────────── Source Control ────────────────────── //

    /// <summary>
    /// Enables or disables a source globally. Disabled sources stop processing
    /// audio/video data entirely.
    /// </summary>
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_source_set_enabled(ObsSourceHandle source,
        [MarshalAs(UnmanagedType.I1)] bool enabled);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_source_inc_showing(ObsSourceHandle source);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_source_inc_active(ObsSourceHandle source);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_source_dec_showing(ObsSourceHandle source);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_source_dec_active(ObsSourceHandle source);

    /// <summary>
    /// Pushes updated settings to a live source, triggering its update() callback.
    /// </summary>
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern void obs_source_update(ObsSourceHandle source, ObsDataHandle settings);

    /// <summary>
    /// Retrieves a string value from an obs_data_t settings object.
    /// The returned pointer is borrowed — do not free it.
    /// </summary>
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern IntPtr obs_data_get_string(ObsDataHandle data, string name);


    // ── Signal handlers (for receiving "saved" completion notification) ──

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern IntPtr obs_output_get_signal_handler(ObsOutputHandle output);
    
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    public delegate void signal_callback_t(IntPtr data, IntPtr calldata);

    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern void signal_handler_connect(IntPtr handler, string signal, signal_callback_t callback, IntPtr data);

    // ── Proc handlers (for triggering "save" action on replay buffer) ──

    /// <summary>
    /// Gets the proc handler for an output. Proc handlers dispatch ACTIONS (e.g., "save").
    /// This is distinct from signal handlers, which SUBSCRIBE to events (e.g., "saved").
    /// </summary>
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern IntPtr obs_output_get_proc_handler(ObsOutputHandle output);

    /// <summary>
    /// Calls a named procedure on a proc handler. For replay buffer, calling "save"
    /// triggers the mux + write pipeline via obs-ffmpeg-mux.
    /// calldata must be a valid pointer to initialized calldata_t memory, NOT IntPtr.Zero.
    /// </summary>
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern void proc_handler_call(IntPtr handler, string name, IntPtr calldata);

    // ── Calldata Accessors (for reading signal callback data) ──

    /// <summary>
    /// Reads a string value from a calldata_t object by key name.
    /// Used in the "saved" signal handler to extract the output file path.
    /// Available as an exported function in OBS 30+ (not an inline macro).
    /// The output <paramref name="val"/> is a borrowed pointer — do NOT free it.
    /// </summary>
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    [return: MarshalAs(UnmanagedType.I1)]
    public static extern bool calldata_get_string(IntPtr data, string name, out IntPtr val);

    // ── Output Diagnostics ──

    /// <summary>
    /// Returns the last error message from an output, or null if no error occurred.
    /// Critical for diagnosing muxer failures silently swallowed by libobs.
    /// The returned pointer is owned by OBS — do NOT free it.
    /// </summary>
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl)]
    public static extern IntPtr obs_output_get_last_error(ObsOutputHandle output);

    /// <summary>
    /// Disconnects a previously connected signal handler callback.
    /// Required for clean teardown to prevent dangling callback pointers
    /// after the managed delegate is garbage collected.
    /// </summary>
    [DllImport(ObsDll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
    public static extern void signal_handler_disconnect(IntPtr handler, string signal, signal_callback_t callback, IntPtr data);
}

[StructLayout(LayoutKind.Sequential)]
public struct ObsVideoInfo
{
    public IntPtr graphics_module; // "libobs-d3d11" (Windows) or "libobs-opengl" (Linux)
    public uint fps_num;
    public uint fps_den;
    public uint base_width;
    public uint base_height;
    public uint output_width;
    public uint output_height;
    public int output_format; // VIDEO_FORMAT_NV12
    public uint adapter;
    [MarshalAs(UnmanagedType.I1)]
    public bool gpu_conversion;
    public int colorspace;
    public int range;
    public int scale_type;
}

[StructLayout(LayoutKind.Sequential)]
public struct ObsAudioInfo
{
    public uint samples_per_sec;
    public int speakers; // SPEAKERS_STEREO
}
