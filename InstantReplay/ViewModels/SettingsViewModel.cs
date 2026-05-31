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
    private readonly ObsRecorderService _engine;
    private readonly GlobalHotkeyManager _hotkeyManager;
    private readonly GlobalHotkeyService _hotkeyService;

    private static readonly string SettingsFilePath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        "Lag", "settings.json");

    [ObservableProperty]
    private bool _hasPendingChanges;

    /// <summary>
    /// True while the constructor is populating collections and loading persisted settings.
    /// SaveSettings() is suppressed during this window — otherwise RefreshMonitors()/
    /// RefreshMicrophones() (which assign SelectedMonitor/SelectedMicrophone) would trigger a save
    /// that overwrites settings.json with defaults BEFORE LoadSettings() can read it. That was the
    /// root cause of "settings reset on restart".
    /// </summary>
    private bool _isInitializing;

    // ───────────── Buffer Duration ─────────────

    /// <summary>Available replay buffer duration options.</summary>
    public IReadOnlyList<BufferOption> BufferOptions { get; } = new[]
    {
        new BufferOption("30 секунд", TimeSpan.FromSeconds(30)),
        new BufferOption("1 хвилина", TimeSpan.FromMinutes(1)),
        new BufferOption("2 хвилини", TimeSpan.FromMinutes(2)),
        new BufferOption("5 хвилин", TimeSpan.FromMinutes(5)),
        new BufferOption("10 хвилин", TimeSpan.FromMinutes(10)),
        new BufferOption("15 хвилин", TimeSpan.FromMinutes(15))
    };

    [ObservableProperty]
    private BufferOption _selectedBuffer;

    partial void OnSelectedBufferChanged(BufferOption value)
    {
        SaveSettings();
    }

    /// <summary>Reliably gets the selected buffer length in seconds (supports sub-minute values).</summary>
    public int BufferSeconds => ParseBufferToSeconds(SelectedBuffer);

    /// <summary>
    /// Converts a buffer option to seconds. Parses the human label per spec — "30 секунд" → 30,
    /// "5 хвилин" → 300 — instead of assuming minutes (which truncated 30 s to 0). Falls back to the
    /// option's authoritative <see cref="TimeSpan"/> if the label can't be parsed.
    /// </summary>
    private static int ParseBufferToSeconds(BufferOption option)
    {
        string label = option.Display;
        var match = System.Text.RegularExpressions.Regex.Match(label, @"\d+");

        if (match.Success && int.TryParse(match.Value, out int n))
        {
            if (label.Contains("секунд")) return n;
            if (label.Contains("хвилин")) return n * 60;
        }

        // Safety net: the option always carries a real TimeSpan.
        return (int)option.Duration.TotalSeconds;
    }

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

    [ObservableProperty]
    private bool _isCapturingHotkey;

    // ───────────── Paths ─────────────

    [ObservableProperty]
    private string _libraryPath;

    partial void OnLibraryPathChanged(string value)
    {
        SaveSettings();
    }

    // ───────────── Frame Rate ─────────────

    public IReadOnlyList<int> FrameRateOptions { get; } = new[] { 24, 30, 60 };

    [ObservableProperty]
    private int _selectedFrameRate = 30;

    partial void OnSelectedFrameRateChanged(int value)
    {
        SaveSettings();
    }

    // ───────────── Microphone Volume ─────────────

    /// <summary>Microphone volume in percent (0–100). Applied to the OBS mic source as a 0.0–1.0 gain.</summary>
    [ObservableProperty]
    private int _micVolume = 100;

    partial void OnMicVolumeChanged(int value)
    {
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

    /// <summary>Available UI languages. English is the default.</summary>
    public IReadOnlyList<LanguageOption> LanguageOptions { get; } = new[]
    {
        new LanguageOption("en", "English"),
        new LanguageOption("uk", "Українська")
    };

    [ObservableProperty]
    private LanguageOption _selectedLanguage;

    partial void OnSelectedLanguageChanged(LanguageOption value)
    {
        // Switch the live UI language immediately (applies even while loading the persisted value).
        Lag.App.SetLanguage(value.Code);
        SaveSettings();
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
        ObsRecorderService engine,
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
            // Establish safe defaults FIRST (direct field writes, no PropertyChanged / no save).
            _selectedBuffer = BufferOptions.FirstOrDefault(b => (int)b.Duration.TotalMinutes == 5) ?? BufferOptions[1]; // 5 minutes
            _selectedLanguage = LanguageOptions[0]; // English by default
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

        // Listen for hotkey capture events
        _hotkeyManager.HotkeyCaptured += OnHotkeyCaptured;
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
    /// Handles the captured hotkey event from the GlobalHotkeyManager.
    /// Updates the display and saves the new binding.
    /// </summary>
    private void OnHotkeyCaptured(object? sender, HotkeyCapturedEventArgs e)
    {
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
                LibraryPath = LibraryPath,
                FrameRate = SelectedFrameRate,
                MonitorDeviceName = SelectedMonitor?.DeviceName ?? string.Empty,
                MicrophoneId = SelectedMicrophone?.Id ?? string.Empty,
                StartWithWindows = StartWithWindows,
                AutoStartRecording = AutoStartRecording,
                Language = SelectedLanguage.Code,
                MicVolume = MicVolume
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
            
            LibraryPath = settings.LibraryPath ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyVideos), "Lag");
            SelectedFrameRate = FrameRateOptions.Contains(settings.FrameRate)
                ? settings.FrameRate : 30;
            MicVolume = Math.Clamp(settings.MicVolume, 0, 100);

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
        public string FFmpegPath { get; set; } = "ffmpeg";
        public string LibraryPath { get; set; } = "";
        public int FrameRate { get; set; } = 30;
        public string MonitorDeviceName { get; set; } = "";
        public string MicrophoneId { get; set; } = "";
        public bool StartWithWindows { get; set; }
        public bool AutoStartRecording { get; set; }

        /// <summary>UI language code: "en" (default) or "uk".</summary>
        public string Language { get; set; } = "en";

        /// <summary>Microphone volume in percent (0–100).</summary>
        public int MicVolume { get; set; } = 100;
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
