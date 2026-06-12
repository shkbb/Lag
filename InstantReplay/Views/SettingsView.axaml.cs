using Avalonia.Controls;
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
}
