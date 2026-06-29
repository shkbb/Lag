using System;
using Avalonia.Threading;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Lag.Core;
using Lag.Services;

namespace Lag.ViewModels;

/// <summary>
/// Live audio meter for the Audio settings tab (Phase 1): two level bars driven by an
/// <see cref="AudioMonitor"/>. It only runs while the Audio tab is actually on screen, and it is a
/// pure measurement aid — it records nothing and changes nothing in the captured clips.
/// </summary>
public partial class SettingsViewModel
{
    private AudioMonitor? _audioMonitor;
    private DispatcherTimer? _meterTimer;
    private bool _audioViewAttached;

    private AudioDeviceWatcher? _deviceWatcher;
    private DispatcherTimer? _deviceRefreshDebounce;
    private bool _windowActive = true;   // false while the window is minimised to tray / not focused

    /// <summary>Starts watching for audio-device hot-plug (headphones/USB mic/default change) so the
    /// device lists update live, not only at startup. Called once from the ctor.</summary>
    private void InitDeviceWatcher()
    {
        _deviceWatcher = new AudioDeviceWatcher();
        _deviceWatcher.DevicesChanged += OnAudioDevicesChanged;
    }

    private void OnAudioDevicesChanged()
    {
        // Fires on a system thread, often as a burst — marshal to UI and debounce into one refresh.
        Dispatcher.UIThread.Post(() =>
        {
            _deviceRefreshDebounce ??= CreateDeviceDebounce();
            _deviceRefreshDebounce.Stop();
            _deviceRefreshDebounce.Start();
        });
    }

    private DispatcherTimer CreateDeviceDebounce()
    {
        var t = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(300) };
        t.Tick += (_, _) =>
        {
            t.Stop();
            RefreshAudioDevices();
        };
        return t;
    }

    private void RefreshAudioDevices()
    {
        bool wasInit = _isInitializing;
        _isInitializing = true;   // device-list rebuilds preserve the current pick; don't nag "restart"
        try
        {
            RefreshMicrophones();
            RefreshOutputDevices();
        }
        finally { _isInitializing = wasInit; }

        // Re-open the meter/monitor on the (possibly new default) device if it's running.
        RestartAudioMeterIfRunning();
    }

    /// <summary>Live microphone input level, 0..1 (perceptual). Settings meter only.</summary>
    [ObservableProperty] private double _micLevel;

    /// <summary>Live output (loopback) level, 0..1 — shows whether the chosen output is the live one.</summary>
    [ObservableProperty] private double _outputLevel;

    /// <summary>"Hear yourself": routes the mic to the selected output for a quick test. Settings-only,
    /// never recorded; reset to off whenever the meter stops so it can't keep playing in the background
    /// (a different tab, navigating away, or minimising to tray all stop it).</summary>
    [ObservableProperty] private bool _hearYourself;

    partial void OnHearYourselfChanged(bool value)
    {
        if (_audioViewAttached && _windowActive && SelectedSettingsTab == 1)
            _audioMonitor?.SetMonitoring(value, SelectedOutputDevice?.Id);
        OnPropertyChanged(nameof(HearYourselfButtonText));
    }

    /// <summary>The "hear yourself" toggle button's label — flips between Listen / Stop.</summary>
    public string HearYourselfButtonText => HearYourself ? Localizer.Get("Settings_Stop") : Localizer.Get("Settings_Listen");

    [RelayCommand]
    private void ToggleHearYourself() => HearYourself = !HearYourself;

    /// <summary>The host window reports focus/visibility so monitoring stops when minimised to tray or
    /// the window loses focus (no surprise background mic-to-speakers).</summary>
    public void SetWindowActive(bool active)
    {
        _windowActive = active;
        UpdateAudioMeter();
    }

    /// <summary>The gate threshold currently in effect, 0..1 — drives the marker on the mic meter.
    /// Mirrors the gate that runs inside the monitor (so the marker = what you hear).</summary>
    [ObservableProperty] private double _gateThreshold = 0.08;

    /// <summary>Whether the live mic level is currently passing the gate (for the bar colour).</summary>
    [ObservableProperty] private bool _micGateOpen;

    /// <summary>Pushes the current gate config to the live monitor, so "hear yourself" and the marker
    /// reflect Auto / the threshold immediately. Safe before the monitor exists.</summary>
    public void PushGateConfig() =>
        _audioMonitor?.SetInputGate(InputSensitivityAuto, (float)(InputSensitivityThreshold / 100.0));

    /// <summary>Pushes the noise-suppression toggle to the live monitor, so "hear yourself" reflects
    /// RNNoise on/off immediately. Safe before the monitor exists.</summary>
    public void PushDenoiseConfig() => _audioMonitor?.SetNoiseSuppression(NoiseSuppression);

    /// <summary>The view reports its on-screen state so the meter runs only while it's visible (no
    /// background mic capture once the user navigates away).</summary>
    public void SetAudioViewAttached(bool attached)
    {
        _audioViewAttached = attached;
        UpdateAudioMeter();
    }

    /// <summary>Switches the meter on only while the Audio tab is the visible one.</summary>
    partial void OnSelectedSettingsTabChanged(int value) => UpdateAudioMeter();

    private void UpdateAudioMeter()
    {
        if (_audioViewAttached && _windowActive && SelectedSettingsTab == 1) StartAudioMeter();
        else StopAudioMeter();
    }

    private void StartAudioMeter()
    {
        _audioMonitor ??= new AudioMonitor();
        _audioMonitor.Start(SelectedMicrophone?.Id, SelectedOutputDevice?.Id);
        _audioMonitor.SetInputGate(InputSensitivityAuto, (float)(InputSensitivityThreshold / 100.0));
        _audioMonitor.SetNoiseSuppression(NoiseSuppression);
        if (HearYourself) _audioMonitor.SetMonitoring(true, SelectedOutputDevice?.Id);

        _meterTimer ??= CreateMeterTimer();
        _meterTimer.Start();
    }

    private DispatcherTimer CreateMeterTimer()
    {
        var t = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(33) };   // ~30 fps
        t.Tick += (_, _) =>
        {
            if (_audioMonitor == null) return;
            MicLevel = Smooth(MicLevel, Curve(_audioMonitor.ReadMicPeak()));
            OutputLevel = Smooth(OutputLevel, Curve(_audioMonitor.ReadOutputPeak()));

            // The gate runs inside the monitor (per mic buffer) — mirror its live state to the UI.
            GateThreshold = _audioMonitor.GateThreshold;
            MicGateOpen = _audioMonitor.GateOpen;
        };
        return t;
    }

    // Lift quiet levels so normal speech visibly moves the bar (meters are perceptual, not linear).
    private static double Curve(float level) => Math.Min(1.0, Math.Pow(level, 0.6));

    // Ease the bar toward the new peak: quick attack so it tracks your voice, slow release so it
    // glides down instead of snapping — the VU-meter feel that's easy on the eye.
    private static double Smooth(double current, double target)
    {
        double rate = target > current ? 0.5 : 0.16;
        return current + (target - current) * rate;
    }

    private void StopAudioMeter()
    {
        HearYourself = false;          // never leave the mic playing to the speakers in the background
        _meterTimer?.Stop();
        _audioMonitor?.Stop();
        MicLevel = 0;
        OutputLevel = 0;
    }

    /// <summary>Re-opens the meter (and monitoring) on the newly chosen mic/output, if running.</summary>
    private void RestartAudioMeterIfRunning()
    {
        if (!(_audioViewAttached && _windowActive && SelectedSettingsTab == 1)) return;
        _audioMonitor?.Start(SelectedMicrophone?.Id, SelectedOutputDevice?.Id);
        _audioMonitor?.SetInputGate(InputSensitivityAuto, (float)(InputSensitivityThreshold / 100.0));
        _audioMonitor?.SetNoiseSuppression(NoiseSuppression);
        if (HearYourself) _audioMonitor?.SetMonitoring(true, SelectedOutputDevice?.Id);
    }
}
