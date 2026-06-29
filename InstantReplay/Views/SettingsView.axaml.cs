using System;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Interactivity;
using Lag.ViewModels;

namespace Lag.Views;

public partial class SettingsView : UserControl
{
    public SettingsView()
    {
        InitializeComponent();
    }

    private SettingsViewModel? Vm => DataContext as SettingsViewModel;
    private Window? _hostWindow;

    // The live audio meter (and "hear yourself") runs only while this view is actually on screen AND
    // the window is focused — so navigating away or minimising to the tray stops it, no surprise
    // background mic-to-speakers.
    protected override void OnAttachedToVisualTree(VisualTreeAttachmentEventArgs e)
    {
        base.OnAttachedToVisualTree(e);
        Vm?.SetAudioViewAttached(true);

        _hostWindow = TopLevel.GetTopLevel(this) as Window;
        if (_hostWindow != null)
        {
            _hostWindow.Activated += OnWindowActivated;
            _hostWindow.Deactivated += OnWindowDeactivated;
            Vm?.SetWindowActive(_hostWindow.IsActive);
        }
    }

    protected override void OnDetachedFromVisualTree(VisualTreeAttachmentEventArgs e)
    {
        base.OnDetachedFromVisualTree(e);
        Vm?.SetAudioViewAttached(false);

        if (_hostWindow != null)
        {
            _hostWindow.Activated -= OnWindowActivated;
            _hostWindow.Deactivated -= OnWindowDeactivated;
            _hostWindow = null;
        }
    }

    private void OnWindowActivated(object? sender, EventArgs e) => Vm?.SetWindowActive(true);
    private void OnWindowDeactivated(object? sender, EventArgs e) => Vm?.SetWindowActive(false);

    private static int TagToInt(object? sender) =>
        sender is Control { Tag: { } tag } && int.TryParse(tag.ToString(), out int v) ? v : 0;

    /// <summary>Tab strip (Video / Audio / General) — drives the Carousel + underline animation.</summary>
    private void OnTabClick(object? sender, RoutedEventArgs e)
    {
        if (Vm != null) Vm.SelectedSettingsTab = TagToInt(sender);
    }

    /// <summary>Mic channels segmented control (Tag: 0 = stereo, 1 = mono).</summary>
    private void OnChannelSegClick(object? sender, RoutedEventArgs e)
    {
        if (Vm != null) Vm.MicChannelIndex = TagToInt(sender);
    }

    /// <summary>System-audio source segmented control (Tag: 0 = whole PC, 1 = specific apps).</summary>
    private void OnSourceSegClick(object? sender, RoutedEventArgs e)
    {
        if (Vm != null) Vm.AudioModeIndex = TagToInt(sender);
    }

    /// <summary>"I understand" on the intensive-quality disclaimer — dismiss for the session.</summary>
    private void OnIntensiveUnderstood(object? sender, RoutedEventArgs e)
    {
        Vm?.AcknowledgeIntensive();
    }

    /// <summary>A hardware preset card was clicked — apply its resolved settings.</summary>
    private void OnPresetClick(object? sender, TappedEventArgs e)
    {
        if (Vm != null && sender is Control { DataContext: PresetCard card })
            Vm.ApplyPresetCommand.Execute(card);
    }

    // Press/drag anywhere on the input-sensitivity bar to set the threshold (0–100% across the bar).
    // PointerPressed and PointerMoved take different event-arg types, so two thin handlers share one body.
    private void OnGatePressed(object? sender, PointerPressedEventArgs e) => SetGateFromPointer(sender, e);
    private void OnGateMoved(object? sender, PointerEventArgs e) => SetGateFromPointer(sender, e);

    private void SetGateFromPointer(object? sender, PointerEventArgs e)
    {
        if (Vm == null || sender is not Control bar) return;
        var p = e.GetCurrentPoint(bar);
        if (!p.Properties.IsLeftButtonPressed) return;
        double w = bar.Bounds.Width;
        if (w <= 0) return;
        Vm.InputSensitivityThreshold = (int)Math.Round(Math.Clamp(p.Position.X / w * 100.0, 0, 100));
        e.Pointer.Capture(bar);
    }
}
