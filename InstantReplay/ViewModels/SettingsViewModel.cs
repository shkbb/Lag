using System.Text.Json;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Lag.Core;
using Lag.Services;
using Lag.Services.ObsIntegration;
using Microsoft.Win32;
using SharpHook.Native;
using Velopack;
using Velopack.Sources;

namespace Lag.ViewModels;

/// <summary>
/// ViewModel for the Settings view. Manages all user-configurable options
/// and persists them to a JSON file.
/// </summary>
public partial class SettingsViewModel : ViewModelBase
{
    private readonly Lag.Services.IReplayRecorder _engine;   // the active recorder (VFR, or OBS fallback)
    private readonly GlobalHotkeyManager _hotkeyManager;
    private readonly GlobalHotkeyService _hotkeyService;

    private static readonly string SettingsFilePath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        "Lag", "settings.json");

    [ObservableProperty]
    private bool _hasPendingChanges;

    /// <summary>Active settings tab (0 = Video, 1 = Audio, 2 = General). UI state only.</summary>
    [ObservableProperty]
    private int _selectedSettingsTab;

    /// <summary>
    /// True while the constructor is populating collections and loading persisted settings.
    /// SaveSettings() is suppressed during this window — otherwise RefreshMonitors()/
    /// RefreshMicrophones() (which assign SelectedMonitor/SelectedMicrophone) would trigger a save
    /// that overwrites settings.json with defaults BEFORE LoadSettings() can read it. That was the
    /// root cause of "settings reset on restart".
    /// </summary>
    private bool _isInitializing;

    // ───────────── Buffer Duration ─────────────

    /// <summary>
    /// Replay buffer duration options. Labels are LOCALIZED, so the list is (re)built by
    /// <see cref="RebuildLocalizedOptions"/> at startup and on every language switch.
    /// </summary>
    public System.Collections.ObjectModel.ObservableCollection<BufferOption> BufferOptions { get; } = new();

    [ObservableProperty]
    private BufferOption _selectedBuffer = null!;

    partial void OnSelectedBufferChanged(BufferOption value)
    {
        SaveSettings();
    }

    /// <summary>
    /// Selected buffer length in seconds. Duration is the authoritative value — labels are
    /// localized display-only strings (the old label-parsing approach broke on translation).
    /// </summary>
    public int BufferSeconds => (int)SelectedBuffer.Duration.TotalSeconds;

    // ───────────── Monitors ─────────────
    
    private readonly HardwareDetector _hardwareDetector;

    public System.Collections.ObjectModel.ObservableCollection<HardwareDetector.MonitorInfo> Monitors { get; } = new();

    [ObservableProperty]
    private HardwareDetector.MonitorInfo? _selectedMonitor;

    partial void OnSelectedMonitorChanged(HardwareDetector.MonitorInfo? value)
    {
        SaveSettings();
    }

    private void RefreshMonitors()
    {
        Monitors.Clear();
        var screens = _hardwareDetector.GetAvailableMonitors();
        foreach (var s in screens) Monitors.Add(s);

        SelectedMonitor = Monitors.FirstOrDefault(m => m.IsPrimary) ?? Monitors.FirstOrDefault();
    }

    // ───────────── Microphones ─────────────

    public System.Collections.ObjectModel.ObservableCollection<MicrophoneInfo> Microphones { get; } = new();

    [ObservableProperty]
    private MicrophoneInfo? _selectedMicrophone;

    partial void OnSelectedMicrophoneChanged(MicrophoneInfo? value)
    {
        SaveSettings();
    }

    private void RefreshMicrophones()
    {
        Microphones.Clear();
        var mics = _hardwareDetector.GetMicrophones();
        foreach (var m in mics) Microphones.Add(m);

        SelectedMicrophone = Microphones.FirstOrDefault();
    }

    // ───────────── Hotkey ─────────────

    [ObservableProperty]
    private string _hotkeyDisplayText = "Alt + F10";

    partial void OnHotkeyDisplayTextChanged(string value)
    {
        OnPropertyChanged(nameof(HotkeyParts));
    }

    /// <summary>Hotkey split into kbd-chip parts for the Figma design ("Ctrl","Shift","S").</summary>
    public IReadOnlyList<string> HotkeyParts =>
        HotkeyDisplayText.Split('+', StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries);

    [ObservableProperty]
    private bool _isCapturingHotkey;

    // ───────────── Screenshot hotkey ─────────────

    private KeyCode _screenshotKey = KeyCode.VcF9;
    private ModifierMask _screenshotModifiers = ModifierMask.LeftAlt;

    [ObservableProperty]
    private string _screenshotHotkeyDisplayText = "Alt + F9";

    partial void OnScreenshotHotkeyDisplayTextChanged(string value) =>
        OnPropertyChanged(nameof(ScreenshotHotkeyParts));

    /// <summary>Screenshot hotkey split into kbd-chip parts ("Alt","F9").</summary>
    public IReadOnlyList<string> ScreenshotHotkeyParts =>
        ScreenshotHotkeyDisplayText.Split('+', StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries);

    [ObservableProperty]
    private bool _isCapturingScreenshotKey;

    private bool _screenshotCaptureMode;

    [RelayCommand]
    private void CaptureScreenshotHotkey()
    {
        _screenshotCaptureMode = true;
        IsCapturingScreenshotKey = true;
        _hotkeyManager.IsCapturing = true;
    }

    private void ApplyScreenshotHotkeyToService()
    {
        try
        {
            _hotkeyService.UpdateScreenshotHotkey(_screenshotModifiers, _screenshotKey);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to apply screenshot hotkey: {ex.Message}");
        }
    }

    // ───────────── Paths ─────────────

    [ObservableProperty]
    private string _libraryPath;

    partial void OnLibraryPathChanged(string value)
    {
        SaveSettings();
    }

    // ───────────── Frame Rate ─────────────

    /// <summary>FPS presets; Value = 0 means "Custom" (label localized → rebuilt on language switch).</summary>
    public System.Collections.ObjectModel.ObservableCollection<FpsOption> FpsOptions { get; } = new();

    [ObservableProperty]
    private FpsOption _selectedFps = null!;

    partial void OnSelectedFpsChanged(FpsOption value)
    {
        OnPropertyChanged(nameof(IsCustomFps));
        SaveSettings();
    }

    /// <summary>Custom frame rate (used when the "Custom" preset is selected).</summary>
    [ObservableProperty]
    private int _customFps = 60;

    partial void OnCustomFpsChanged(int value)
    {
        SaveSettings();
    }

    public bool IsCustomFps => SelectedFps.Value == 0;

    /// <summary>The frame rate that actually goes to the engine.</summary>
    public int EffectiveFps =>
        SelectedFps.Value > 0 ? SelectedFps.Value : Math.Clamp(CustomFps, 1, 1000);

    // ───────────── Output File Format ─────────────

    /// <summary>Container formats for saved replays.</summary>
    // mp4 + mov are verified clean with our H.264/HEVC/AV1 + AAC streams. mkv/avi are omitted: the
    // matroska muxer rejects our stream extradata at write_header (needs separate muxer work) and avi
    // can't carry HEVC/AV1 — offering them would produce broken files. (mkv: future task.)
    public IReadOnlyList<string> FormatOptions { get; } = new[] { "mp4", "mov" };

    [ObservableProperty]
    private string _selectedFormat = "mp4";

    partial void OnSelectedFormatChanged(string value)
    {
        SaveSettings();
    }

    // ───────────── Output Resolution (render downscale) ─────────────

    /// <summary>
    /// Output (render/encode) resolution presets. Capture always stays at the native screen
    /// resolution; this only downscales the encoded output (TargetHeight = 0 means "Native").
    /// The "Native" label is localized → list is rebuilt by <see cref="RebuildLocalizedOptions"/>.
    /// </summary>
    public System.Collections.ObjectModel.ObservableCollection<ResolutionOption> ResolutionOptions { get; } = new();

    [ObservableProperty]
    private ResolutionOption _selectedResolution = null!;

    partial void OnSelectedResolutionChanged(ResolutionOption value)
    {
        SaveSettings();
    }

    // ───────────── Video Codec ─────────────

    /// <summary>
    /// Encoder choice. "Auto" (empty id, default) keeps the automatic hardware-fallback chain
    /// (NVENC → AMF → QSV → x264). Picking a specific codec makes it the preferred encoder —
    /// the engine tries it first and only falls back to the chain if it can't be created.
    /// </summary>
    public System.Collections.ObjectModel.ObservableCollection<CodecOption> CodecOptions { get; } = new();

    [ObservableProperty]
    private CodecOption _selectedCodec = null!;

    partial void OnSelectedCodecChanged(CodecOption value)
    {
        SaveSettings();
    }

    // ───────────── Library Auto-Cleanup (opt-in) ─────────────

    /// <summary>When enabled, the oldest clips are auto-deleted once the library exceeds the limit.</summary>
    [ObservableProperty]
    private bool _autoCleanupEnabled;

    partial void OnAutoCleanupEnabledChanged(bool value)
    {
        SaveSettings();
    }

    /// <summary>Available library size limits for the auto-cleanup feature.</summary>
    public IReadOnlyList<StorageLimitOption> StorageLimitOptions { get; } = new[]
    {
        new StorageLimitOption(10),
        new StorageLimitOption(25),
        new StorageLimitOption(50),
        new StorageLimitOption(100),
        new StorageLimitOption(200),
        new StorageLimitOption(500)
    };

    [ObservableProperty]
    private StorageLimitOption _selectedStorageLimit;

    partial void OnSelectedStorageLimitChanged(StorageLimitOption value)
    {
        SaveSettings();
    }

    // ───────────── Video Bitrate ─────────────

    /// <summary>Bitrate presets; Kbps = 0 means "Custom" (label localized → rebuilt on language switch).</summary>
    public System.Collections.ObjectModel.ObservableCollection<BitrateOption> BitrateOptions { get; } = new();

    [ObservableProperty]
    private BitrateOption _selectedBitrate = null!;

    partial void OnSelectedBitrateChanged(BitrateOption value)
    {
        OnPropertyChanged(nameof(IsCustomBitrate));
        SaveSettings();
    }

    /// <summary>Custom bitrate in Mbps (used when the "Custom" preset is selected).</summary>
    [ObservableProperty]
    private int _customBitrateMbps = 20;

    partial void OnCustomBitrateMbpsChanged(int value)
    {
        SaveSettings();
    }

    public bool IsCustomBitrate => SelectedBitrate.Kbps == 0;

    /// <summary>The bitrate that actually goes to the encoder, in kbps.</summary>
    public int EffectiveBitrateKbps =>
        SelectedBitrate.Kbps > 0 ? SelectedBitrate.Kbps : Math.Clamp(CustomBitrateMbps, 1, 300) * 1000;

    /// <summary>
    /// Figma-style bitrate slider (Mbps). Reads the effective bitrate; writing snaps the
    /// selection to "Custom" with the chosen value, so the engine always gets exactly it.
    /// </summary>
    public double BitrateSliderValue
    {
        get => EffectiveBitrateKbps / 1000.0;
        set
        {
            int mbps = Math.Clamp((int)Math.Round(value), 1, 150);
            CustomBitrateMbps = mbps;
            var preset = BitrateOptions.FirstOrDefault(b => b.Kbps == mbps * 1000);
            SelectedBitrate = preset ?? BitrateOptions.First(b => b.Kbps == 0);
            OnPropertyChanged(nameof(BitrateSliderValue));
            OnPropertyChanged(nameof(BitrateDisplayMbps));
        }
    }

    /// <summary>Right-side readout for the bitrate row ("50").</summary>
    public int BitrateDisplayMbps => EffectiveBitrateKbps / 1000;

    // ───────────── Recording GPU ─────────────

    /// <summary>Available GPU adapters: "Auto (name of primary)" + each physical adapter.</summary>
    public IReadOnlyList<GpuOption> GpuOptions { get; private set; } = [];

    [ObservableProperty]
    private GpuOption _selectedGpu;

    partial void OnSelectedGpuChanged(GpuOption value)
    {
        SaveSettings();
    }

    private IReadOnlyList<GpuOption> BuildGpuOptions()
    {
        var gpus = _hardwareDetector.GetGpuAdapters();
        var list = new List<GpuOption> { new(-1, $"Auto ({gpus[0].Name})") };
        foreach (var gpu in gpus)
            list.Add(new GpuOption(gpu.Index, $"GPU {gpu.Index}: {gpu.Name}"));
        return list;
    }

    // ───────────── System Audio Capture (all / specific apps) ─────────────

    /// <summary>0 = all PC audio, 1 = specific apps only (drives the ComboBox).</summary>
    [ObservableProperty]
    private int _audioModeIndex;

    partial void OnAudioModeIndexChanged(int value)
    {
        OnPropertyChanged(nameof(IsAppsMode));
        SaveSettings();
    }

    public bool IsAppsMode => AudioModeIndex == 1;

    /// <summary>Live list of apps playing audio (auto-refreshed) merged with saved selections.</summary>
    public System.Collections.ObjectModel.ObservableCollection<AppAudioItem> AudioApps { get; } = new();

    private Avalonia.Threading.DispatcherTimer? _audioAppsTimer;

    /// <summary>Kicks off a background scan of apps currently playing audio and merges the result.</summary>
    private void RefreshAudioAppsNow()
    {
        _ = Task.Run(AudioSessionService.GetActiveAudioApps).ContinueWith(t =>
        {
            if (t.IsCompletedSuccessfully)
                Avalonia.Threading.Dispatcher.UIThread.Post(() => MergeAudioApps(t.Result));
        });
    }

    /// <summary>
    /// Merges the freshly scanned RUNNING apps into the visible list: new apps are appended (with
    /// their icon), apps that have closed are dropped UNLESS the user has them enabled (so a
    /// selection survives the app being closed temporarily).
    /// </summary>
    private void MergeAudioApps(List<AudioSessionService.AudioApp> live)
    {
        foreach (var app in live)
        {
            var existing = AudioApps.FirstOrDefault(a => a.Exe.Equals(app.ExeName, StringComparison.OrdinalIgnoreCase));
            if (existing == null)
                AudioApps.Add(new AppAudioItem(app.ExeName, app.DisplayName, OnAppAudioChanged, app.IconPng));
            else
                existing.UpdateMeta(app.DisplayName, app.IconPng);   // fill icon/name on a stale/restored row
        }

        for (int i = AudioApps.Count - 1; i >= 0; i--)
        {
            var item = AudioApps[i];
            bool stillRunning = live.Any(l => l.ExeName.Equals(item.Exe, StringComparison.OrdinalIgnoreCase));
            if (!stillRunning && !item.IsEnabled)
                AudioApps.RemoveAt(i);
        }
    }

    private void OnAppAudioChanged() => SaveSettings();

    // ───────────── System audio source ("Звук системи" row: enable + volume) ─────────────

    /// <summary>Capture the full-system loopback (used in "Весь звук ПК" mode). Its row checkbox.</summary>
    [ObservableProperty]
    private bool _systemAudioEnabled = true;
    partial void OnSystemAudioEnabledChanged(bool value) => SaveSettings();

    /// <summary>System-audio volume in percent (0–100), applied as a 0.0–1.0 gain to the loopback.</summary>
    [ObservableProperty]
    private int _systemAudioVolume = 100;
    partial void OnSystemAudioVolumeChanged(int value) => SaveSettings();

    /// <summary>Microphone row checkbox — off = no mic captured at all.</summary>
    [ObservableProperty]
    private bool _micEnabled = true;
    partial void OnMicEnabledChanged(bool value) => SaveSettings();

    /// <summary>"Звук гри" row (specific-apps mode): capture the detected game's own audio so it's
    /// always recorded even if it isn't in the picked-apps list, with its own volume — like Medal.</summary>
    [ObservableProperty]
    private bool _gameAudioEnabled = true;
    partial void OnGameAudioEnabledChanged(bool value) => SaveSettings();

    [ObservableProperty]
    private int _gameAudioVolume = 100;
    partial void OnGameAudioVolumeChanged(int value) => SaveSettings();

    // ───────────── Microphone Volume ─────────────

    /// <summary>Microphone volume in percent (0–100). Applied to the OBS mic source as a 0.0–1.0 gain.</summary>
    [ObservableProperty]
    private int _micVolume = 100;

    partial void OnMicVolumeChanged(int value)
    {
        SaveSettings();
    }

    // ───────────── Microphone Channels (stereo / mono) ─────────────

    /// <summary>0 = stereo, 1 = mono (drives the ComboBox).</summary>
    [ObservableProperty]
    private int _micChannelIndex;

    partial void OnMicChannelIndexChanged(int value)
    {
        SaveSettings();
    }

    public bool MicMono => MicChannelIndex == 1;

    // ───────────── Push-to-talk ─────────────

    /// <summary>When on, the mic is muted and live only while the PTT key is held.</summary>
    [ObservableProperty]
    private bool _pushToTalkEnabled;

    partial void OnPushToTalkEnabledChanged(bool value)
    {
        ApplyPttToManager();
        SaveSettings();
    }

    private KeyCode _pttKey = KeyCode.VcV;

    [ObservableProperty]
    private string _pttKeyDisplayText = "V";

    [ObservableProperty]
    private bool _isCapturingPttKey;

    /// <summary>True while the next captured key should be bound as the PTT key (not the save hotkey).</summary>
    private bool _pttCaptureMode;

    [RelayCommand]
    private void CapturePttKey()
    {
        _pttCaptureMode = true;
        IsCapturingPttKey = true;
        _hotkeyManager.IsCapturing = true;
    }

    private void ApplyPttToManager()
    {
        _hotkeyManager.PttEnabled = PushToTalkEnabled;
        _hotkeyManager.PttKey = _pttKey;
    }

    // ───────────── Separate Audio Tracks ─────────────

    /// <summary>Save system audio (track 1) and mic (track 2) as separate tracks in the file.</summary>
    [ObservableProperty]
    private bool _separateAudioTracks;

    partial void OnSeparateAudioTracksChanged(bool value)
    {
        SaveSettings();
    }

    // ───────────── Capture engine (OBS vs native VFR) ─────────────

    /// <summary>Use the native WGC/NVENC VFR engine instead of the OBS replay buffer. OBS stays the
    /// default; this is opt-in until the native engine is fully proven. Takes effect on next Start.</summary>
    [ObservableProperty]
    private bool _useVfrEngine;

    partial void OnUseVfrEngineChanged(bool value)
    {
        HasPendingChanges = true;   // engine swap needs a stop/start to apply
        SaveSettings();
    }

    // ───────────── Automation ─────────────

    /// <summary>The Run-key registry value name used for "Start with Windows".</summary>
    private const string AutoRunKeyName = "Lag";
    private const string AutoRunRegistryPath = @"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";

    /// <summary>Launch the app automatically when Windows starts (HKCU ...\Run registry entry).</summary>
    [ObservableProperty]
    private bool _startWithWindows;

    partial void OnStartWithWindowsChanged(bool value)
    {
        // Don't touch the registry while loading persisted state — only on a real user toggle.
        if (!_isInitializing)
            ApplyStartWithWindows(value);
        SaveSettings();
    }

    /// <summary>Automatically begin recording into the replay buffer as soon as the app launches.</summary>
    [ObservableProperty]
    private bool _autoStartRecording;

    partial void OnAutoStartRecordingChanged(bool value)
    {
        SaveSettings();
    }

    /// <summary>Launch hidden in the system tray instead of showing the main window.</summary>
    [ObservableProperty]
    private bool _startMinimized;

    partial void OnStartMinimizedChanged(bool value)
    {
        SaveSettings();
    }

    /// <summary>
    /// Disable Windows Game Mode (Medal-style). Game Mode deprioritizes background apps
    /// while a game is focused, which throttles the capture pipeline to a few FPS and
    /// delays hotkey handling. Default ON — strongly recommended.
    /// </summary>
    [ObservableProperty]
    private bool _disableGameMode = true;

    partial void OnDisableGameModeChanged(bool value)
    {
        if (!_isInitializing)
            ApplyDisableGameMode(value);
        SaveSettings();
    }

    /// <summary>
    /// Toggles Windows Game Mode via the HKCU GameBar keys (the same approach Medal uses).
    /// disable=true → Game Mode off; false → restored to the Windows default (on).
    /// </summary>
    private static void ApplyDisableGameMode(bool disable)
    {
        try
        {
            using RegistryKey key = Registry.CurrentUser.CreateSubKey(@"SOFTWARE\Microsoft\GameBar");
            int value = disable ? 0 : 1;
            key.SetValue("AllowAutoGameMode", value, RegistryValueKind.DWord);
            key.SetValue("AutoGameModeEnabled", value, RegistryValueKind.DWord);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to toggle Windows Game Mode: {ex.Message}");
        }
    }

    /// <summary>
    /// Adds or removes the application from the Windows startup (Run) registry key so it launches
    /// automatically at logon. Uses <see cref="Environment.ProcessPath"/> as the target executable.
    /// </summary>
    private static void ApplyStartWithWindows(bool enable)
    {
        try
        {
            using RegistryKey? runKey = Registry.CurrentUser.OpenSubKey(AutoRunRegistryPath, writable: true);
            if (runKey == null) return;

            if (enable)
            {
                string? exePath = Environment.ProcessPath;
                if (!string.IsNullOrEmpty(exePath))
                    runKey.SetValue(AutoRunKeyName, $"\"{exePath}\"");
            }
            else
            {
                if (runKey.GetValue(AutoRunKeyName) != null)
                    runKey.DeleteValue(AutoRunKeyName, throwOnMissingValue: false);
            }
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to update 'Start with Windows' registry entry: {ex.Message}");
        }
    }

    // ───────────── Language ─────────────

    /// <summary>Available UI languages (each name shown in its own language). English is the default.</summary>
    public IReadOnlyList<LanguageOption> LanguageOptions { get; } = new[]
    {
        new LanguageOption("en", "English"),
        new LanguageOption("uk", "Українська"),
        new LanguageOption("de", "Deutsch"),
        new LanguageOption("fr", "Français"),
        new LanguageOption("be", "Беларуская"),
        new LanguageOption("lt", "Lietuvių"),
        new LanguageOption("et", "Eesti"),
        new LanguageOption("lv", "Latviešu"),
        new LanguageOption("fi", "Suomi"),
        new LanguageOption("sv", "Svenska"),
        new LanguageOption("no", "Norsk"),
        new LanguageOption("da", "Dansk"),
        new LanguageOption("nl", "Nederlands"),
        new LanguageOption("it", "Italiano"),
        new LanguageOption("es", "Español"),
        new LanguageOption("pt", "Português"),
        new LanguageOption("ja", "日本語")
    };

    [ObservableProperty]
    private LanguageOption _selectedLanguage;

    partial void OnSelectedLanguageChanged(LanguageOption value)
    {
        // Switch the live UI language immediately (applies even while loading the persisted value),
        // then rebuild every dropdown whose item labels are localized strings.
        Lag.App.SetLanguage(value.Code);
        RebuildLocalizedOptions();
        SaveSettings();
    }

    /// <summary>
    /// (Re)builds all option lists whose item LABELS are localized ("5 хв" / "5 min", "Auto",
    /// "Native", "Custom"). XAML {DynamicResource} can't reach inside data items, so on a language
    /// switch we regenerate the items and re-select the entry with the same underlying value.
    /// SaveSettings is suppressed during the churn — selections are value-identical.
    /// </summary>
    private void RebuildLocalizedOptions()
    {
        bool wasInitializing = _isInitializing;
        _isInitializing = true;
        try
        {
            // ── Buffer durations (default: 5 min) ──
            int selBufSec = (int)(SelectedBuffer?.Duration.TotalSeconds ?? 300);
            BufferOptions.Clear();
            foreach (var d in new[]
                     {
                         TimeSpan.FromSeconds(30), TimeSpan.FromMinutes(1), TimeSpan.FromMinutes(2),
                         TimeSpan.FromMinutes(5), TimeSpan.FromMinutes(10), TimeSpan.FromMinutes(15)
                     })
            {
                string label = d.TotalSeconds < 60
                    ? Localizer.Format("Time_SecShort", (int)d.TotalSeconds)
                    : Localizer.Format("Time_MinShort", (int)d.TotalMinutes);
                BufferOptions.Add(new BufferOption(label, d));
            }
            SelectedBuffer = BufferOptions.FirstOrDefault(b => (int)b.Duration.TotalSeconds == selBufSec)
                             ?? BufferOptions[3];

            // ── Output resolution (default: Native) ──
            int selRes = SelectedResolution?.TargetHeight ?? 0;
            ResolutionOptions.Clear();
            ResolutionOptions.Add(new ResolutionOption(Localizer.Get("Option_Native"), 0));
            ResolutionOptions.Add(new ResolutionOption("1080p", 1080));
            ResolutionOptions.Add(new ResolutionOption("720p", 720));
            SelectedResolution = ResolutionOptions.FirstOrDefault(r => r.TargetHeight == selRes)
                                 ?? ResolutionOptions[0];

            // ── Codec (default: Auto) ──
            string selCodec = SelectedCodec?.EncoderId ?? "";
            CodecOptions.Clear();
            CodecOptions.Add(new CodecOption(Localizer.Get("Option_Auto"), ""));
            CodecOptions.Add(new CodecOption("NVIDIA NVENC", "ffmpeg_nvenc"));
            CodecOptions.Add(new CodecOption("AMD AMF", "ffmpeg_amf"));
            CodecOptions.Add(new CodecOption("Intel QuickSync", "obs_qsv11"));
            CodecOptions.Add(new CodecOption("x264 (CPU)", "obs_x264"));
            SelectedCodec = CodecOptions.FirstOrDefault(c => c.EncoderId == selCodec) ?? CodecOptions[0];

            // ── Bitrate (default: 20 Mbps) ──
            int selKbps = SelectedBitrate?.Kbps ?? 20000;
            BitrateOptions.Clear();
            foreach (int kbps in new[] { 10000, 20000, 30000, 50000, 80000, 100000 })
                BitrateOptions.Add(new BitrateOption($"{kbps / 1000} Mbps", kbps));
            BitrateOptions.Add(new BitrateOption(Localizer.Get("Option_Custom"), 0));
            SelectedBitrate = BitrateOptions.FirstOrDefault(b => b.Kbps == selKbps) ?? BitrateOptions[2];

            // ── FPS (default: 30) ──
            int selFps = SelectedFps?.Value ?? 30;
            FpsOptions.Clear();
            foreach (int fps in new[] { 24, 30, 60, 120, 240, 360 })
                FpsOptions.Add(new FpsOption(fps.ToString(), fps));
            FpsOptions.Add(new FpsOption(Localizer.Get("Option_Custom"), 0));
            SelectedFps = FpsOptions.FirstOrDefault(f => f.Value == selFps) ?? FpsOptions[1];
        }
        finally
        {
            _isInitializing = wasInitializing;
        }
    }

    // ───────────── About / Auto-Update (Velopack) ─────────────

    // GitHub repository hosting the published Velopack releases.
    private const string UpdateRepoUrl = "https://github.com/shkbb/Lag";

    /// <summary>Currently installed app version (or "Debug/Dev" when not installed via Velopack).</summary>
    [ObservableProperty]
    private string _appVersion = "Debug/Dev";

    /// <summary>True while an update check/download is in progress (drives the UI loading state).</summary>
    [ObservableProperty]
    private bool _isCheckingForUpdate;

    /// <summary>Human-readable result of the last update check.</summary>
    [ObservableProperty]
    private string _updateStatus = string.Empty;

    /// <summary>Resolves the installed version via Velopack (local metadata, no network).</summary>
    private static string ResolveAppVersion()
    {
        try
        {
            var mgr = new UpdateManager(new GithubSource(UpdateRepoUrl, string.Empty, false));
            return mgr.IsInstalled && mgr.CurrentVersion != null
                ? mgr.CurrentVersion.ToString()
                : "Debug/Dev";
        }
        catch
        {
            return "Debug/Dev";
        }
    }

    /// <summary>
    /// Checks GitHub releases for a newer version; if found, downloads it and restarts into it.
    /// Does nothing in local/dev mode (not installed via Velopack).
    /// </summary>
    [RelayCommand]
    private async Task CheckForUpdatesAsync()
    {
        if (IsCheckingForUpdate) return;

        IsCheckingForUpdate = true;
        UpdateStatus = Lag.Core.Localizer.Get("Update_Checking");
        try
        {
            var mgr = new UpdateManager(new GithubSource(UpdateRepoUrl, string.Empty, false));

            // Don't check for updates in local debug mode (not installed via Velopack).
            if (!mgr.IsInstalled)
            {
                UpdateStatus = Lag.Core.Localizer.Get("Update_DevMode");
                return;
            }

            var newVersion = await mgr.CheckForUpdatesAsync();
            if (newVersion == null)
            {
                UpdateStatus = Lag.Core.Localizer.Get("Update_UpToDate");
                return;
            }

            // Download and restart into the new version.
            UpdateStatus = Lag.Core.Localizer.Get("Update_Downloading");
            await mgr.DownloadUpdatesAsync(newVersion);
            mgr.ApplyUpdatesAndRestart(newVersion);
        }
        catch (Exception ex)
        {
            Console.WriteLine($"Update failed: {ex.Message}");
            UpdateStatus = Lag.Core.Localizer.Format("Update_Failed", ex.Message);
        }
        finally
        {
            IsCheckingForUpdate = false;
        }
    }

    public SettingsViewModel(
        Lag.Services.IReplayRecorder engine,
        GlobalHotkeyManager hotkeyManager,
        GlobalHotkeyService hotkeyService,
        HardwareDetector hardwareDetector)
    {
        Title = "Налаштування";
        _engine = engine;
        _hotkeyManager = hotkeyManager;
        _hotkeyService = hotkeyService;
        _hardwareDetector = hardwareDetector;

        // Suppress saves while we build the initial state, so device enumeration can't clobber
        // the persisted settings file before LoadSettings() reads it.
        _isInitializing = true;
        try
        {
            // Establish safe defaults FIRST (saves are suppressed by _isInitializing).
            _selectedLanguage = LanguageOptions[0]; // English by default
            _selectedStorageLimit = StorageLimitOptions[2]; // 50 GB (used only when cleanup is enabled)
            GpuOptions = BuildGpuOptions();
            _selectedGpu = GpuOptions[0];               // Auto by default

            // Build the localized dropdown lists (buffer 5 min, Native, Auto, 20 Mbps, 30 fps).
            RebuildLocalizedOptions();

            _appVersion = ResolveAppVersion();
            _libraryPath = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyVideos), "Lag");

            // Populate device lists BEFORE LoadSettings so the persisted monitor/mic can be matched.
            RefreshMonitors();
            RefreshMicrophones();

            // Override defaults + device selections with persisted values when settings.json exists.
            LoadSettings();
        }
        finally
        {
            _isInitializing = false;
        }

        // Register the persisted (or default) hotkey with the active Win32 service at startup.
        ApplyHotkeyToGlobalService();
        ApplyScreenshotHotkeyToService();

        // Re-assert "Game Mode off" each launch when enabled (Windows updates and the
        // user toggling it back in Windows Settings would otherwise silently undo it).
        if (DisableGameMode)
            ApplyDisableGameMode(true);

        // Mirror the persisted push-to-talk state onto the global hook.
        ApplyPttToManager();

        // Listen for hotkey capture events
        _hotkeyManager.HotkeyCaptured += OnHotkeyCaptured;

        // Live "apps playing audio" scanner (Medal-style picker): scan now, then every 4 s.
        RefreshAudioAppsNow();
        _audioAppsTimer = new Avalonia.Threading.DispatcherTimer { Interval = TimeSpan.FromSeconds(4) };
        _audioAppsTimer.Tick += (_, _) => RefreshAudioAppsNow();
        _audioAppsTimer.Start();
    }

    /// <summary>
    /// Mirrors the currently bound combination (modifiers + key) onto the active Win32 global
    /// hotkey so a rebind takes effect immediately and persists across restarts.
    /// </summary>
    private void ApplyHotkeyToGlobalService()
    {
        try
        {
            _hotkeyService.UpdateHotkey(_hotkeyManager.RequiredModifiers, _hotkeyManager.RequiredKey);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to apply hotkey to global service: {ex.Message}");
        }
    }

    /// <summary>
    /// Enters hotkey capture mode. The next key press will be bound as the new hotkey.
    /// </summary>
    [RelayCommand]
    private void CaptureHotkey()
    {
        IsCapturingHotkey = true;
        HotkeyDisplayText = Lag.Core.Localizer.Get("Hotkey_PressCombo");
        _hotkeyManager.IsCapturing = true;
    }

    /// <summary>
    /// Hotkey ACTIONS are ignored while a capture is in progress and for a short grace
    /// period right after it. Without this, the combo being typed fires immediately:
    /// the old Win32 registration is still live during capture, and re-registering the
    /// new combo while its keys are physically held lets keyboard auto-repeat trigger it.
    /// Checked by MainViewModel and the Win32 hotkey handler in App.
    /// </summary>
    public bool AreHotkeysSuppressed =>
        IsCapturingHotkey || IsCapturingPttKey || IsCapturingScreenshotKey ||
        DateTime.UtcNow < _hotkeySuppressedUntil;

    private DateTime _hotkeySuppressedUntil = DateTime.MinValue;

    /// <summary>
    /// Handles the captured hotkey event from the GlobalHotkeyManager.
    /// Updates the display and saves the new binding.
    /// </summary>
    private void OnHotkeyCaptured(object? sender, HotkeyCapturedEventArgs e)
    {
        // Swallow the keys the user is still holding (auto-repeat, late releases).
        _hotkeySuppressedUntil = DateTime.UtcNow.AddMilliseconds(800);

        if (_screenshotCaptureMode)
        {
            _screenshotCaptureMode = false;
            _screenshotKey = e.Key;
            _screenshotModifiers = e.Modifiers;
            ScreenshotHotkeyDisplayText = FormatHotkey(e.Modifiers, e.Key);
            IsCapturingScreenshotKey = false;

            ApplyScreenshotHotkeyToService();
            SaveSettings();
            return;
        }

        // Push-to-talk capture takes the SINGLE key only (modifiers ignored — PTT is a held key).
        if (_pttCaptureMode)
        {
            _pttCaptureMode = false;
            _pttKey = e.Key;
            PttKeyDisplayText = e.Key.ToString().Replace("Vc", "");
            IsCapturingPttKey = false;

            ApplyPttToManager();
            SaveSettings();
            return;
        }

        _hotkeyManager.RequiredKey = e.Key;
        _hotkeyManager.RequiredModifiers = e.Modifiers;

        HotkeyDisplayText = FormatHotkey(e.Modifiers, e.Key);
        IsCapturingHotkey = false;

        // Apply the new combination to the active Win32 hotkey, then persist it.
        ApplyHotkeyToGlobalService();
        SaveSettings();
    }

    /// <summary>
    /// Formats modifier + key into a human-readable string (e.g., "Alt + F10").
    /// </summary>
    private static string FormatHotkey(ModifierMask modifiers, KeyCode key)
    {
        var parts = new List<string>();

        if (modifiers.HasFlag(ModifierMask.LeftCtrl) || modifiers.HasFlag(ModifierMask.RightCtrl))
            parts.Add("Ctrl");
        if (modifiers.HasFlag(ModifierMask.LeftAlt) || modifiers.HasFlag(ModifierMask.RightAlt))
            parts.Add("Alt");
        if (modifiers.HasFlag(ModifierMask.LeftShift) || modifiers.HasFlag(ModifierMask.RightShift))
            parts.Add("Shift");

        // Convert KeyCode enum name to readable format (e.g., VcF10 → F10)
        string keyName = key.ToString().Replace("Vc", "");
        parts.Add(keyName);

        return string.Join(" + ", parts);
    }

    [RelayCommand]
    private async Task BrowseFolderAsync()
    {
        if (Avalonia.Application.Current?.ApplicationLifetime is Avalonia.Controls.ApplicationLifetimes.IClassicDesktopStyleApplicationLifetime desktop && desktop.MainWindow != null)
        {
            var storageProvider = Avalonia.Controls.TopLevel.GetTopLevel(desktop.MainWindow)?.StorageProvider;
            if (storageProvider != null)
            {
                var result = await storageProvider.OpenFolderPickerAsync(new Avalonia.Platform.Storage.FolderPickerOpenOptions
                {
                    Title = Lag.Core.Localizer.Get("Browse_Title"),
                    AllowMultiple = false
                });

                if (result != null && result.Count > 0)
                {
                    LibraryPath = result[0].Path.LocalPath;
                    SaveSettings();
                }
            }
        }
    }

    // ───────────── Settings Persistence ─────────────

    private void SaveSettings()
    {
        // Never persist while constructing/loading — see _isInitializing.
        if (_isInitializing) return;

        try
        {
            if (_engine.IsRecording)
            {
                HasPendingChanges = true;
            }

            var settings = new SettingsData
            {
                // Persist seconds (not minutes) so sub-minute buffers like "30 секунд" survive restart.
                BufferSeconds = BufferSeconds,
                HotkeyKey = _hotkeyManager.RequiredKey.ToString(),
                HotkeyModifiers = _hotkeyManager.RequiredModifiers.ToString(),
                ScreenshotKey = _screenshotKey.ToString(),
                ScreenshotModifiers = _screenshotModifiers.ToString(),
                LibraryPath = LibraryPath,
                FrameRate = EffectiveFps,
                FileFormat = SelectedFormat,
                MonitorDeviceName = SelectedMonitor?.DeviceName ?? string.Empty,
                MicrophoneId = SelectedMicrophone?.Id ?? string.Empty,
                StartWithWindows = StartWithWindows,
                AutoStartRecording = AutoStartRecording,
                StartMinimized = StartMinimized,
                DisableGameMode = DisableGameMode,
                Language = SelectedLanguage.Code,
                MicVolume = MicVolume,
                OutputResolutionHeight = SelectedResolution.TargetHeight,
                CodecName = SelectedCodec.EncoderId,
                AutoCleanupEnabled = AutoCleanupEnabled,
                MaxLibrarySizeGb = SelectedStorageLimit.Gb,
                BitrateKbps = EffectiveBitrateKbps,
                GpuIndex = SelectedGpu.Index,
                AudioCaptureMode = IsAppsMode ? "apps" : "all",
                AudioApps = AudioApps
                    .Where(a => a.IsEnabled || a.Volume != 100)
                    .Select(a => new AppAudioSetting { Exe = a.Exe, Enabled = a.IsEnabled, Volume = a.Volume })
                    .ToList(),
                PttEnabled = PushToTalkEnabled,
                PttKey = _pttKey.ToString(),
                MicMono = MicMono,
                SeparateAudioTracks = SeparateAudioTracks,
                UseVfrEngine = UseVfrEngine,
                SystemAudioEnabled = SystemAudioEnabled,
                SystemAudioVolume = SystemAudioVolume,
                MicEnabled = MicEnabled,
                GameAudioEnabled = GameAudioEnabled,
                GameAudioVolume = GameAudioVolume
            };

            string dir = Path.GetDirectoryName(SettingsFilePath)!;
            Directory.CreateDirectory(dir);
            string json = JsonSerializer.Serialize(settings, new JsonSerializerOptions { WriteIndented = true });
            File.WriteAllText(SettingsFilePath, json);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to save settings: {ex.Message}");
        }
    }

    private void LoadSettings()
    {
        try
        {
            if (!File.Exists(SettingsFilePath)) return;

            string json = File.ReadAllText(SettingsFilePath);
            var settings = JsonSerializer.Deserialize<SettingsData>(json);
            if (settings == null) return;

            // Prefer the new seconds field; fall back to the legacy minutes field for old settings files.
            int targetSeconds = settings.BufferSeconds > 0
                ? settings.BufferSeconds
                : settings.BufferMinutes * 60;
            SelectedBuffer = BufferOptions.FirstOrDefault(b =>
                (int)b.Duration.TotalSeconds == targetSeconds) ?? SelectedBuffer;

            if (Enum.TryParse<KeyCode>(settings.HotkeyKey, out var key))
                _hotkeyManager.RequiredKey = key;
            if (Enum.TryParse<ModifierMask>(settings.HotkeyModifiers, out var mod))
                _hotkeyManager.RequiredModifiers = mod;

            HotkeyDisplayText = FormatHotkey(_hotkeyManager.RequiredModifiers, _hotkeyManager.RequiredKey);

            if (Enum.TryParse<KeyCode>(settings.ScreenshotKey, out var shotKey))
                _screenshotKey = shotKey;
            if (Enum.TryParse<ModifierMask>(settings.ScreenshotModifiers, out var shotMod))
                _screenshotModifiers = shotMod;
            ScreenshotHotkeyDisplayText = FormatHotkey(_screenshotModifiers, _screenshotKey);


            LibraryPath = settings.LibraryPath ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyVideos), "Lag");
            // FPS: match a preset, otherwise restore as "Custom".
            if (settings.FrameRate > 0)
            {
                var fpsPreset = FpsOptions.FirstOrDefault(f => f.Value == settings.FrameRate);
                if (fpsPreset != null)
                {
                    SelectedFps = fpsPreset;
                }
                else
                {
                    CustomFps = Math.Clamp(settings.FrameRate, 1, 1000);
                    SelectedFps = FpsOptions.First(f => f.Value == 0); // Custom
                }
            }

            SelectedFormat = FormatOptions.Contains(settings.FileFormat) ? settings.FileFormat : "mp4";
            MicVolume = Math.Clamp(settings.MicVolume, 0, 100);
            SelectedResolution = ResolutionOptions.FirstOrDefault(r =>
                r.TargetHeight == settings.OutputResolutionHeight) ?? ResolutionOptions[0];
            SelectedCodec = CodecOptions.FirstOrDefault(c =>
                c.EncoderId == settings.CodecName) ?? CodecOptions[0];
            AutoCleanupEnabled = settings.AutoCleanupEnabled;
            SelectedStorageLimit = StorageLimitOptions.FirstOrDefault(l =>
                l.Gb == settings.MaxLibrarySizeGb) ?? StorageLimitOptions[2];

            // Bitrate: match a preset, otherwise restore as "Custom".
            if (settings.BitrateKbps > 0)
            {
                var preset = BitrateOptions.FirstOrDefault(b => b.Kbps == settings.BitrateKbps);
                if (preset != null)
                {
                    SelectedBitrate = preset;
                }
                else
                {
                    CustomBitrateMbps = Math.Clamp(settings.BitrateKbps / 1000, 1, 300);
                    SelectedBitrate = BitrateOptions.First(b => b.Kbps == 0); // Custom
                }
            }

            // GPU adapter (falls back to Auto when the saved adapter no longer exists).
            SelectedGpu = GpuOptions.FirstOrDefault(g => g.Index == settings.GpuIndex) ?? GpuOptions[0];

            // System audio capture mode + saved per-app selections.
            AudioModeIndex = settings.AudioCaptureMode == "apps" ? 1 : 0;
            SystemAudioEnabled = settings.SystemAudioEnabled;
            SystemAudioVolume = settings.SystemAudioVolume;
            MicEnabled = settings.MicEnabled;
            GameAudioEnabled = settings.GameAudioEnabled;
            GameAudioVolume = settings.GameAudioVolume;
            foreach (var saved in settings.AudioApps)
            {
                if (string.IsNullOrWhiteSpace(saved.Exe)) continue;
                if (AudioApps.Any(a => a.Exe.Equals(saved.Exe, StringComparison.OrdinalIgnoreCase))) continue;

                string display = saved.Exe.EndsWith(".exe", StringComparison.OrdinalIgnoreCase)
                    ? saved.Exe[..^4] : saved.Exe;
                AudioApps.Add(new AppAudioItem(saved.Exe, display, OnAppAudioChanged)
                {
                    IsEnabled = saved.Enabled,
                    Volume = Math.Clamp(saved.Volume, 0, 100)
                });
            }

            // Push-to-talk + mic channels + tracks.
            PushToTalkEnabled = settings.PttEnabled;
            if (Enum.TryParse<KeyCode>(settings.PttKey, out var pttKey))
            {
                _pttKey = pttKey;
                PttKeyDisplayText = pttKey.ToString().Replace("Vc", "");
            }
            MicChannelIndex = settings.MicMono ? 1 : 0;
            SeparateAudioTracks = settings.SeparateAudioTracks;
            UseVfrEngine = settings.UseVfrEngine;

            // Restore the persisted monitor/microphone by matching against the enumerated devices.
            if (!string.IsNullOrEmpty(settings.MonitorDeviceName))
            {
                var monitor = Monitors.FirstOrDefault(m => m.DeviceName == settings.MonitorDeviceName);
                if (monitor != null) SelectedMonitor = monitor;
            }

            if (!string.IsNullOrEmpty(settings.MicrophoneId))
            {
                var mic = Microphones.FirstOrDefault(m => m.Id == settings.MicrophoneId);
                if (mic != null) SelectedMicrophone = mic;
            }

            // Automation flags (registry side-effects are suppressed during init via _isInitializing).
            StartWithWindows = settings.StartWithWindows;
            AutoStartRecording = settings.AutoStartRecording;
            StartMinimized = settings.StartMinimized;
            DisableGameMode = settings.DisableGameMode;

            // Language (applies the persisted UI language via OnSelectedLanguageChanged).
            SelectedLanguage = LanguageOptions.FirstOrDefault(l => l.Code == settings.Language) ?? LanguageOptions[0];
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to load settings: {ex.Message}");
        }
    }

    private class SettingsData
    {
        /// <summary>Buffer length in seconds (authoritative). Supports sub-minute values.</summary>
        public int BufferSeconds { get; set; }

        /// <summary>Legacy minutes field, kept only for reading older settings files.</summary>
        public int BufferMinutes { get; set; } = 5;
        public int MonitorIndex { get; set; }
        public string HotkeyKey { get; set; } = "VcF10";
        public string HotkeyModifiers { get; set; } = "LeftAlt";

        /// <summary>Screenshot hotkey (separate from the save-replay one). Default Alt+F9.</summary>
        public string ScreenshotKey { get; set; } = "VcF9";
        public string ScreenshotModifiers { get; set; } = "LeftAlt";
        public string FFmpegPath { get; set; } = "ffmpeg";
        public string LibraryPath { get; set; } = "";
        public int FrameRate { get; set; } = 30;
        public string MonitorDeviceName { get; set; } = "";
        public string MicrophoneId { get; set; } = "";
        public bool StartWithWindows { get; set; }
        public bool AutoStartRecording { get; set; }

        /// <summary>Launch hidden in the system tray (window not shown until requested).</summary>
        public bool StartMinimized { get; set; }

        /// <summary>Keep Windows Game Mode disabled (recommended for stable in-game capture).</summary>
        public bool DisableGameMode { get; set; } = true;

        /// <summary>Opt-in game_capture hook for exclusive-fullscreen games.</summary>
        public bool GameCaptureEnabled { get; set; }

        /// <summary>UI language code: "en" (default) or "uk".</summary>
        public string Language { get; set; } = "en";

        /// <summary>Microphone volume in percent (0–100).</summary>
        public int MicVolume { get; set; } = 100;

        /// <summary>Encoded output height (0 = native, 1080 = 1080p, 720 = 720p).</summary>
        public int OutputResolutionHeight { get; set; }

        /// <summary>Preferred encoder id ("" = automatic hardware fallback chain).</summary>
        public string CodecName { get; set; } = "";

        /// <summary>Opt-in: auto-delete oldest clips when the library exceeds the limit.</summary>
        public bool AutoCleanupEnabled { get; set; }

        /// <summary>Library size limit in GB for the auto-cleanup feature.</summary>
        public int MaxLibrarySizeGb { get; set; } = 50;

        /// <summary>Video encoder bitrate in kbps.</summary>
        public int BitrateKbps { get; set; } = 20000;

        /// <summary>DXGI adapter index for capture/render (-1 = Auto/primary).</summary>
        public int GpuIndex { get; set; } = -1;

        /// <summary>"all" = whole desktop audio; "apps" = selected applications only.</summary>
        public string AudioCaptureMode { get; set; } = "all";

        /// <summary>Per-application audio selections (checked apps and custom volumes).</summary>
        public List<AppAudioSetting> AudioApps { get; set; } = new();

        /// <summary>Push-to-talk enabled + its key.</summary>
        public bool PttEnabled { get; set; }
        public string PttKey { get; set; } = "VcV";

        /// <summary>Downmix microphone to mono.</summary>
        public bool MicMono { get; set; }

        /// <summary>System-audio source row: enabled + volume (default on, 100%).</summary>
        public bool SystemAudioEnabled { get; set; } = true;
        public int SystemAudioVolume { get; set; } = 100;

        /// <summary>Microphone row enabled (default on).</summary>
        public bool MicEnabled { get; set; } = true;

        /// <summary>"Звук гри" row: capture the detected game's audio + its volume (default on, 100%).</summary>
        public bool GameAudioEnabled { get; set; } = true;
        public int GameAudioVolume { get; set; } = 100;

        /// <summary>Save system audio and mic as separate tracks in the file.</summary>
        public bool SeparateAudioTracks { get; set; }

        /// <summary>Use the native WGC/NVENC VFR engine instead of OBS (opt-in).</summary>
        public bool UseVfrEngine { get; set; }

        /// <summary>Output container format: mp4 (default), mkv, mov or avi.</summary>
        public string FileFormat { get; set; } = "mp4";
    }

    /// <summary>Persisted per-application audio selection.</summary>
    public class AppAudioSetting
    {
        public string Exe { get; set; } = "";
        public bool Enabled { get; set; }
        public int Volume { get; set; } = 100;
    }
}

/// <summary>Replay buffer duration option for the Settings dropdown.</summary>
public record BufferOption(string Display, TimeSpan Duration)
{
    public override string ToString() => Display;
}

/// <summary>UI language option for the Settings dropdown.</summary>
public record LanguageOption(string Code, string Display)
{
    public override string ToString() => Display;
}

/// <summary>Output resolution preset (TargetHeight = 0 means native screen resolution).</summary>
public record ResolutionOption(string Display, int TargetHeight)
{
    public override string ToString() => Display;
}

/// <summary>Video codec option (EncoderId = "" means automatic selection).</summary>
public record CodecOption(string Display, string EncoderId)
{
    public override string ToString() => Display;
}

/// <summary>Library size limit option for auto-cleanup.</summary>
public record StorageLimitOption(int Gb)
{
    public override string ToString() => $"{Gb} GB";
}

/// <summary>Video bitrate preset (Kbps = 0 means "Custom").</summary>
public record BitrateOption(string Display, int Kbps)
{
    public override string ToString() => Display;
}

/// <summary>Frame-rate preset (Value = 0 means "Custom").</summary>
public record FpsOption(string Display, int Value)
{
    public override string ToString() => Display;
}

/// <summary>GPU adapter option (Index = -1 means Auto/primary).</summary>
public record GpuOption(int Index, string Display)
{
    public override string ToString() => Display;
}

/// <summary>
/// One row of the Medal-style "record audio from these apps" list: checkbox + per-app volume.
/// Raises the supplied callback on every change so selections persist immediately.
/// </summary>
public partial class AppAudioItem : ObservableObject
{
    /// <summary>Executable name used to match the OBS application-audio capture (e.g. "Discord.exe").</summary>
    public string Exe { get; }

    /// <summary>Friendly name shown in the UI (app's FileDescription / window title / process name).
    /// Observable so a later scan can upgrade a restored "Process.exe" name to the real one.</summary>
    [ObservableProperty]
    private string _displayName;

    /// <summary>The app's icon for the list row (decoded from the exe), or null. Observable so the
    /// icon can fill in once the app is seen playing audio (restored entries start without one).</summary>
    [ObservableProperty]
    private Avalonia.Media.Imaging.Bitmap? _icon;

    private readonly Action _onChanged;

    [ObservableProperty]
    private bool _isEnabled;

    partial void OnIsEnabledChanged(bool value) => _onChanged();

    /// <summary>Per-application capture volume in percent (0–100).</summary>
    [ObservableProperty]
    private int _volume = 100;

    partial void OnVolumeChanged(int value) => _onChanged();

    public AppAudioItem(string exe, string displayName, Action onChanged, byte[]? iconPng = null)
    {
        Exe = exe;
        _displayName = displayName;
        _onChanged = onChanged;
        Icon = DecodeIcon(iconPng);
    }

    /// <summary>Refreshes a row with fresh scan data — upgrades the friendly name and fills the icon
    /// if it was missing (e.g. a restored selection that started as just the process name).</summary>
    public void UpdateMeta(string displayName, byte[]? iconPng)
    {
        if (!string.IsNullOrWhiteSpace(displayName)) DisplayName = displayName;
        if (Icon == null) { var ic = DecodeIcon(iconPng); if (ic != null) Icon = ic; }
    }

    private static Avalonia.Media.Imaging.Bitmap? DecodeIcon(byte[]? iconPng)
    {
        if (iconPng is not { Length: > 0 }) return null;
        try { return new Avalonia.Media.Imaging.Bitmap(new System.IO.MemoryStream(iconPng)); }
        catch { return null; }
    }
}
