using CommunityToolkit.Mvvm.ComponentModel;

namespace Lag.Models;

/// <summary>
/// Represents a saved instant replay video clip with associated metadata.
/// </summary>
public partial class ReplayClip : ObservableObject
{
    /// <summary>Selected in the library grid (multi-select for bulk deletion).</summary>
    [ObservableProperty]
    private bool _isSelected;

    /// <summary>True for screenshots (PNG/JPG): no duration, opens in the system viewer.</summary>
    public bool IsImage { get; set; }

    /// <summary>Absolute path to the .mp4 replay file.</summary>
    public string FilePath { get; set; } = string.Empty;

    /// <summary>Absolute path to the generated thumbnail image.</summary>
    public string ThumbnailPath { get; set; } = string.Empty;

    /// <summary>In-memory Avalonia Bitmap for the UI.</summary>
    public Avalonia.Media.Imaging.Bitmap? Thumbnail { get; set; }

    /// <summary>Duration of the video clip.</summary>
    public TimeSpan Duration { get; set; }

    /// <summary>Date and time when the replay was saved.</summary>
    public DateTime CreatedDate { get; set; }

    /// <summary>File size in bytes.</summary>
    public long FileSize { get; set; }

    /// <summary>Human-readable file size (e.g., "45.2 MB").</summary>
    public string FileSizeDisplay =>
        FileSize switch
        {
            >= 1_073_741_824 => $"{FileSize / 1_073_741_824.0:F1} GB",
            >= 1_048_576 => $"{FileSize / 1_048_576.0:F1} MB",
            >= 1024 => $"{FileSize / 1024.0:F1} KB",
            _ => $"{FileSize} B"
        };

    /// <summary>Clip title shown on cards/lists (file name without extension).</summary>
    public string DisplayName => System.IO.Path.GetFileNameWithoutExtension(FilePath);

    /// <summary>Short "duration · date" line for list items (e.g., "0:42 · 10.06").</summary>
    public string ShortMeta => $"{DurationDisplay} · {CreatedDate:dd.MM}";

    /// <summary>Human-readable duration (e.g., "2:35").</summary>
    public string DurationDisplay =>
        Duration.TotalHours >= 1
            ? Duration.ToString(@"h\:mm\:ss")
            : Duration.ToString(@"m\:ss");
}
