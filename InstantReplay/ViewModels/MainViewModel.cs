using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Lag.Core;
using Lag.Services;
using Lag.Services.ObsIntegration;

namespace Lag.ViewModels;

/// <summary>
/// Root ViewModel for the application shell. Manages navigation between views,
/// recording state, and coordinates the hotkey → save replay flow.
/// 
/// Navigation:
///   Uses a <see cref="CurrentView"/> property bound to a ContentControl in MainWindow.
///   Switching views updates this property, triggering a DataTemplate match in the UI.
/// </summary>
public partial class MainViewModel : ViewModelBase
{
    private readonly ObsRecorderService _engine;
    private readonly GlobalHotkeyManager _hotkeyManager;

    /// <summary>Child ViewModels for each view.</summary>
    public SettingsViewModel Settings { get; }
    public LibraryViewModel Library { get; }
    public PlayerViewModel Player { get; }

    /// <summary>The currently displayed view (bound to ContentControl).</summary>
    [ObservableProperty]
    private ViewModelBase _currentView;

    /// <summary>Whether the recording engine is active.</summary>
    [ObservableProperty]
    private bool _isRecording;

    /// <summary>Status text shown in the status bar.</summary>
    [ObservableProperty]
    private string _statusText = Lag.Core.Localizer.Get("Status_Ready");

    /// <summary>Buffer fill indicator text (e.g., "2:35 / 5:00").</summary>
    [ObservableProperty]
    private string _bufferStatus = "0:00";

    /// <summary>Whether a replay save is currently in progress.</summary>
    [ObservableProperty]
    private bool _isSaving;

    /// <summary>
    /// Whether the player is in fullscreen mode. When true, MainWindow collapses the left
    /// navigation sidebar and the player collapses its right "Replays" sidebar, giving the
    /// VideoView the entire window. Toggled from the Player and restored with ESC.
    /// </summary>
    [ObservableProperty]
    private bool _isFullscreen;

    public MainViewModel(
        ObsRecorderService engine,
        GlobalHotkeyManager hotkeyManager,
        SettingsViewModel settings,
        LibraryViewModel library,
        PlayerViewModel player)
    {
        Title = "Lag";
        _engine = engine;
        _hotkeyManager = hotkeyManager;

        Settings = settings;
        Library = library;
        Player = player;
        _currentView = settings; // Default to Settings view

        // Wire up events
        _engine.ReplaySaved += (_, outputVideoPath) =>
        {
            IsSaving = false;
            StatusText = Localizer.Format("Status_ReplaySaved", Path.GetFileName(outputVideoPath));
            
            // Auto-refresh library
            _ = Library.RefreshCommand.ExecuteAsync(null);

            // Play the custom "replay saved" sound (replaces the old system beep).
            PlaySaveSound();
        };

        // Global hotkey → save replay
        _hotkeyManager.HotkeyPressed += (_, _) =>
        {
            if (IsRecording && !IsSaving)
            {
                IsSaving = true;
                Avalonia.Threading.Dispatcher.UIThread.Post(() => StatusText = Localizer.Get("Status_Saving"));
                _ = Task.Run(() => _engine.SaveReplay());
            }
        };

        // Library clip play navigation
        Library.PlayClipRequested += (_, clip) =>
        {
            Player.LoadAndPlay(clip);
            CurrentView = Player;
        };

        // Auto-start recording on launch if the user enabled it. Deferred to the UI loop (Background
        // priority) so it runs after the window and framework finish initializing, not mid-construction.
        if (Settings.AutoStartRecording)
        {
            Avalonia.Threading.Dispatcher.UIThread.Post(
                () => { _ = StartRecordingCommand.ExecuteAsync(null); },
                Avalonia.Threading.DispatcherPriority.Background);
        }
    }

    // ───────────── Navigation Commands ─────────────

    [RelayCommand]
    private void NavigateToSettings()
    {
        if (CurrentView == Player) Player.StopPlayback();
        CurrentView = Settings;
    }

    [RelayCommand]
    private async Task NavigateToLibrary()
    {
        if (CurrentView == Player) Player.StopPlayback();
        CurrentView = Library;
        await Library.RefreshCommand.ExecuteAsync(null);
    }

    [RelayCommand]
    private async Task NavigateToPlayer()
    {
        CurrentView = Player;
        // Ensure the Player sidebar shows the latest clips even on direct navigation.
        await Library.RefreshCommand.ExecuteAsync(null);
    }

    /// <summary>
    /// Toggles fullscreen playback. Bound from the Player's fullscreen button and the ESC key.
    /// The actual WindowState change is applied in MainWindow code-behind, which observes this flag.
    /// </summary>
    [RelayCommand]
    private void ToggleFullscreen() => IsFullscreen = !IsFullscreen;

    // ───────────── Recording Commands ─────────────

    /// <summary>Starts background recording with current settings.</summary>
    [RelayCommand]
    private async Task StartRecordingAsync()
    {
        if (IsRecording) return;
        
        try
        {
            StatusText = Localizer.Get("Status_Starting");
            int bufferSeconds = Settings.BufferSeconds;
            int fps = Settings.SelectedFrameRate;

            if (Settings.SelectedMonitor == null)
                throw new InvalidOperationException(Localizer.Get("Status_NoMonitor"));

            uint width = Settings.SelectedMonitor.Width;
            uint height = Settings.SelectedMonitor.Height;
            string? micId = Settings.SelectedMicrophone?.Id;
            string? monitorId = Settings.SelectedMonitor.DeviceName;

            // Shift heavy native initialization to a background thread to prevent UI freezing/crashing
            string? libPath = Settings.LibraryPath;
            await Task.Run(() =>
            {
                _engine.Initialize(bufferSeconds, fps, width, height, micId, monitorId, libPath);
                _engine.StartBuffer();
            });
            
            IsRecording = true;
            Console.WriteLine($"[DEBUG] Recording started! IsRecording={IsRecording}");
            StatusText = Localizer.Get("Status_Recording");
            BufferStatus = Settings.SelectedBuffer.Display;
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[DEBUG] StartRecording FAILED: {ex.Message}");
            StatusText = Localizer.Format("Status_ObsError", ex.Message);
        }
    }

    /// <summary>Stops background recording.</summary>
    [RelayCommand]
    private void StopRecording()
    {
        if (!IsRecording) return;
        try
        {
            StatusText = Localizer.Get("Status_Stopping");
            // COLD RESTART: fully tear down the capture pipeline (output → encoders → sources → scene),
            // not just the buffer. The libobs core stays resident, but the next StartRecordingAsync()
            // runs a clean Initialize() that re-reads ALL current settings (monitor, mic, FPS,
            // codec, buffer length). The core is only shut down on app exit (DI → Dispose()).
            _engine.Teardown();
            IsRecording = false;
            StatusText = Localizer.Get("Status_Stopped");
            BufferStatus = "0:00";
            Settings.HasPendingChanges = false;
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[DEBUG] StopRecording FAILED: {ex.Message}");
            // Force UI state to stopped regardless of native error —
            // leaving IsRecording=true after a failed stop would lock out the user.
            IsRecording = false;
            StatusText = Localizer.Format("Status_StopError", ex.Message);
            BufferStatus = "0:00";
            Settings.HasPendingChanges = false;
        }
    }

    /// <summary>Toggles recording state.</summary>
    [RelayCommand]
    private async Task ToggleRecordingAsync()
    {
        if (IsRecording)
            StopRecording();
        else
            await StartRecordingAsync();
    }

    /// <summary>Manually triggers a replay save (same as hotkey).</summary>
    [RelayCommand]
    private void SaveReplay()
    {
        Console.WriteLine($"[DEBUG] SaveReplay() EXECUTING! IsRecording={IsRecording}, IsSaving={IsSaving}");
        
        if (!IsRecording)
        {
            Console.WriteLine("[DEBUG] SaveReplay() BLOCKED: Not recording.");
            StatusText = Localizer.Get("Status_NotRecording");
            return;
        }
        
        if (IsSaving)
        {
            Console.WriteLine("[DEBUG] SaveReplay() BLOCKED: Already saving.");
            return;
        }
        
        IsSaving = true;
        StatusText = Localizer.Get("Status_Saving");
        _ = Task.Run(() =>
        {
            try
            {
                _engine.SaveReplay();
                Console.WriteLine("[DEBUG] Save command sent to OBS. Wait 3-5 seconds for muxing to complete...");
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[DEBUG] SaveReplay() EXCEPTION: {ex.Message}");
                Avalonia.Threading.Dispatcher.UIThread.Post(() =>
                {
                    IsSaving = false;
                    StatusText = Localizer.Format("Status_SaveError", ex.Message);
                });
            }
        });
    }

    /// <summary>
    /// Plays the custom "replay saved" sound (Assets/save.wav, copied next to the executable).
    /// Runs on a background thread with PlaySync so the clip plays in full without blocking the UI,
    /// and the SoundPlayer is disposed only after playback completes (Play() + immediate dispose
    /// would truncate the audio).
    /// </summary>
    private static void PlaySaveSound()
    {
        string soundPath = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "Assets", "save.wav");
        if (!File.Exists(soundPath)) return;

        _ = Task.Run(() =>
        {
            try
            {
                using var player = new System.Media.SoundPlayer(soundPath);
                player.PlaySync();
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[MainViewModel] Failed to play save sound: {ex.Message}");
            }
        });
    }

}
