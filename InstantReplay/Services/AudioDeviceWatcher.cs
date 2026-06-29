using System;
using NAudio.CoreAudioApi;
using NAudio.CoreAudioApi.Interfaces;

namespace Lag.Services;

/// <summary>
/// Watches for audio-device hot-plug events — headphones in/out, a USB mic, a default-device change —
/// via the WASAPI endpoint notification callback, and raises <see cref="DevicesChanged"/> so the UI
/// can refresh its mic/output lists live instead of only at startup. The notifications fire on a
/// system thread, so the subscriber MUST marshal to the UI thread itself (and should debounce — a
/// single plug event often produces a burst of callbacks).
/// </summary>
public sealed class AudioDeviceWatcher : IMMNotificationClient, IDisposable
{
    private readonly MMDeviceEnumerator _enumerator = new();
    private bool _registered;

    /// <summary>Raised (on a system thread) whenever the set of audio endpoints or the default changes.</summary>
    public event Action? DevicesChanged;

    public AudioDeviceWatcher()
    {
        try { _enumerator.RegisterEndpointNotificationCallback(this); _registered = true; }
        catch (Exception ex) { Console.WriteLine($"[AudioDeviceWatcher] register failed: {ex.Message}"); }
    }

    private void Raise() => DevicesChanged?.Invoke();

    public void OnDeviceStateChanged(string deviceId, DeviceState newState) => Raise();
    public void OnDeviceAdded(string pwstrDeviceId) => Raise();
    public void OnDeviceRemoved(string deviceId) => Raise();
    public void OnDefaultDeviceChanged(DataFlow flow, Role role, string defaultDeviceId) => Raise();
    public void OnPropertyValueChanged(string pwstrDeviceId, PropertyKey key) { /* not interesting */ }

    public void Dispose()
    {
        try { if (_registered) _enumerator.UnregisterEndpointNotificationCallback(this); } catch { }
        try { _enumerator.Dispose(); } catch { }
    }
}
