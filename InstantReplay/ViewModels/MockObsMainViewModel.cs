using System;
using System.Diagnostics;
using System.IO;
using System.Threading.Tasks;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Lag.Services;
using Lag.Services.ObsIntegration;

namespace Lag.ViewModels;

/// <summary>
/// A mock ViewModel demonstrating how to integrate the ObsRecorderService 
/// with the Avalonia UI, handle dynamic buffer settings, and hook global keys.
/// </summary>
public partial class MockObsMainViewModel : ObservableObject
{
    private readonly ObsRecorderService _obsService;
    private readonly GlobalHotkeyManager _hotkeyManager;

    [ObservableProperty]
    private int _selectedBufferMinutes = 5;

    [ObservableProperty]
    private bool _isRecording;

    [ObservableProperty]
    private string _statusMessage = "Ready to initialize OBS";

    public MockObsMainViewModel(ObsRecorderService obsService, GlobalHotkeyManager hotkeyManager)
    {
        _obsService = obsService;
        _hotkeyManager = hotkeyManager;

        // 1. Subscribe to the successful replay save event
        _obsService.ReplaySaved += OnReplaySaved;

        // 2. Wire up the Global Keyboard Hook
        _hotkeyManager.HotkeyPressed += OnHotkeyPressed;
    }

    /// <summary>
    /// Starts the OBS engine with the dynamically selected UI buffer setting.
    /// Re-bind this command to your "Start Recording" button in Avalonia.
    /// </summary>
    [RelayCommand]
    private void StartRecording()
    {
        if (IsRecording) return;

        try
        {
            StatusMessage = "Initializing OBS backend...";
            
            // Re-initializes libobs with the newly selected UI dropdown value (2, 5, or 10 min)
            _obsService.Initialize(bufferSeconds: SelectedBufferMinutes * 60, frameRate: 60, width: 1920, height: 1080, microphoneId: null);
            
            _obsService.StartBuffer();
            
            IsRecording = true;
            StatusMessage = $"Recording active. Buffer max length: {SelectedBufferMinutes} minutes.";
        }
        catch (Exception ex)
        {
            StatusMessage = $"OBS Error: {ex.Message}";
        }
    }

    /// <summary>
    /// Safely terminates OBS and flushes memory natively.
    /// </summary>
    [RelayCommand]
    private void StopRecording()
    {
        if (!IsRecording) return;

        // Cold restart: tear down the pipeline only; the libobs core stays resident so the next
        // Start re-initializes cleanly. Full obs_shutdown happens only on app exit (Dispose()).
        _obsService.Teardown();
        IsRecording = false;
        StatusMessage = "OBS engine stopped gracefully.";
    }

    /// <summary>
    /// Automatically invoked by GlobalHotkeyManager (SharpHook) when Alt+F10 is pressed globally
    /// </summary>
    private void OnHotkeyPressed(object? sender, EventArgs e)
    {
        if (!IsRecording) return;

        StatusMessage = "Flushing replay buffer...";
        
        // Asynchronously call SaveReplay to prevent UI thread locking during the native signal execution
        Task.Run(() => _obsService.SaveReplay());
    }

    private void OnReplaySaved(object? sender, string outputVideoPath)
    {
        // Must marshal back to UI thread if updating Avalonia bindings
        Avalonia.Threading.Dispatcher.UIThread.Post(() =>
        {
            StatusMessage = $"Replay highly compressed and saved to: {Path.GetFileName(outputVideoPath)}";
        });
    }
}
