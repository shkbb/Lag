using System.Collections.ObjectModel;
using System.Linq;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Lag.Core;
using Lag.Services;

namespace Lag.ViewModels;

/// <summary>A hardware-tailored preset card shown atop the Recording settings. <see cref="Subtitle"/>
/// is the resolved summary for THIS machine; <see cref="IsActive"/> highlights the card whose values
/// match the current selections.</summary>
public partial class PresetCard : ObservableObject
{
    public RecordingIntent Intent { get; }
    public string Title { get; }
    public string Description { get; }

    [ObservableProperty] private string _subtitle = "";
    [ObservableProperty] private bool _isRecommended;
    [ObservableProperty] private bool _isActive;

    public PresetCard(RecordingIntent intent, string title, string description)
    {
        Intent = intent;
        Title = title;
        Description = description;
    }
}

/// <summary>
/// Hardware-tailored presets: the user picks a GOAL (Performance / Balanced / Quality) and the app
/// maps it onto the real machine. The profile is captured fresh each launch (never persisted), so a
/// between-sessions upgrade is picked up automatically; the "Re-scan" button re-captures it live.
/// </summary>
public partial class SettingsViewModel
{
    private MachineProfile? _machineProfile;

    /// <summary>The machine's hardware snapshot. Captured lazily on first use (startup) and re-captured
    /// by <see cref="RescanHardwareCommand"/>. Never persisted — always reflects the current hardware.</summary>
    public MachineProfile Profile => _machineProfile ??= HardwareProfiler.Capture(LibraryPath);

    /// <summary>The three intent presets, resolved for this box.</summary>
    public ObservableCollection<PresetCard> Presets { get; } = new();

    /// <summary>True when free disk is low enough to SUGGEST auto-cleanup. Hint only — we never enable
    /// it automatically (it deletes clips); the user opts in.</summary>
    [ObservableProperty] private bool _showCleanupHint;
    [ObservableProperty] private string _cleanupHintText = "";

    /// <summary>Creates the cards once, then fills their resolved values + recommendation + hint. Called
    /// from the ctor after the option lists and persisted selections exist, and after a rescan.</summary>
    private void InitPresets()
    {
        if (Presets.Count == 0)
        {
            Presets.Add(new PresetCard(RecordingIntent.Performance, Localizer.Get("Preset_Performance"), Localizer.Get("Preset_PerformanceDesc")));
            Presets.Add(new PresetCard(RecordingIntent.Balanced, Localizer.Get("Preset_Balanced"), Localizer.Get("Preset_BalancedDesc")));
            Presets.Add(new PresetCard(RecordingIntent.Quality, Localizer.Get("Preset_Quality"), Localizer.Get("Preset_QualityDesc")));
        }
        RefreshPresetCards();
    }

    private void RefreshPresetCards()
    {
        if (Presets.Count == 0) return;
        var profile = Profile;
        foreach (var card in Presets)
        {
            var r = PresetResolver.Resolve(card.Intent, profile);
            card.Subtitle = r.Summary();
            card.IsRecommended = card.Intent == RecordingIntent.Balanced;   // the recommended middle
        }
        RefreshPresetSelection();

        // Disk low → RECOMMEND auto-cleanup (never auto-enabled). Hidden once the user enables it.
        var balanced = PresetResolver.Resolve(RecordingIntent.Balanced, profile);
        ShowCleanupHint = balanced.RecommendCleanup && !AutoCleanupEnabled;
        CleanupHintText = ShowCleanupHint
            ? Localizer.Format("Preset_CleanupHint", (int)profile.DiskFreeGiB, balanced.SuggestedStorageGb)
            : "";
    }

    /// <summary>Highlights the card whose resolved settings match the current selections (none → Custom).
    /// Safe to call during init (no-op until the cards exist).</summary>
    internal void RefreshPresetSelection()
    {
        if (Presets.Count == 0) return;
        var profile = Profile;
        foreach (var card in Presets)
            card.IsActive = CurrentMatches(PresetResolver.Resolve(card.Intent, profile));
    }

    private bool CurrentMatches(RecommendedSettings r) =>
        SelectedResolution != null && SelectedFps != null && SelectedCodec != null
        && SelectedBitrate != null && SelectedBuffer != null
        && SelectedResolution.TargetHeight == r.TargetHeight
        && SelectedFps.Value == r.Fps
        && SelectedCodec.EncoderId == r.CodecId
        && SelectedBitrate.Kbps == r.BitrateKbps
        && (int)SelectedBuffer.Duration.TotalSeconds == r.BufferSeconds;

    /// <summary>Applies a preset by writing its resolved values into the existing dropdowns. Each value
    /// has a home in the hardware/monitor-built lists; falls back to the closest/Auto when absent.</summary>
    [RelayCommand]
    private void ApplyPreset(PresetCard? card)
    {
        if (card == null) return;
        var r = PresetResolver.Resolve(card.Intent, Profile);

        // Write all five values under the guard so the per-property OnChanged handlers don't pop the
        // intensive-quality warning on an intermediate state (the bug: old fps still above the ceiling
        // while resolution is already set). A preset never warrants the warning — it IS the ceiling.
        _applyingPreset = true;
        try
        {
            SelectedResolution = ResolutionOptions.FirstOrDefault(o => o.TargetHeight == r.TargetHeight)
                                 ?? ResolutionOptions.FirstOrDefault() ?? SelectedResolution;
            SelectedFps = FpsOptions.FirstOrDefault(o => o.Value == r.Fps)
                          ?? FpsOptions.FirstOrDefault(o => o.Value > 0) ?? SelectedFps;
            SelectedCodec = CodecOptions.FirstOrDefault(o => o.EncoderId == r.CodecId)
                            ?? CodecOptions.FirstOrDefault(o => o.EncoderId == "") ?? SelectedCodec;
            SelectedBitrate = BitrateOptions.FirstOrDefault(o => o.Kbps == r.BitrateKbps)
                              ?? BitrateOptions.FirstOrDefault(o => o.Kbps == -1) ?? SelectedBitrate;
            SelectedBuffer = BufferOptions.FirstOrDefault(o => (int)o.Duration.TotalSeconds == r.BufferSeconds)
                             ?? BufferOptions.FirstOrDefault() ?? SelectedBuffer;
        }
        finally { _applyingPreset = false; }

        RefreshPresetSelection();   // the applied card now matches → highlights
    }

}
