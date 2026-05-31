using System.Collections.ObjectModel;
using System.Drawing;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using FFMpegCore;
using Lag.Core;
using Lag.Models;
using Lag.Services;

namespace Lag.ViewModels;

/// <summary>
/// ViewModel for the Library (Gallery) view. Scans the replay library directory
/// for .mp4 files, generates thumbnails via FFMpegCore, and provides CRUD operations.
/// </summary>
public partial class LibraryViewModel : ViewModelBase
{
    public ObservableCollection<ReplayClip> Clips { get; } = new();

    [ObservableProperty]
    private ReplayClip? _selectedClip;

    [ObservableProperty]
    private bool _isLoading;

    /// <summary>
    /// Fired when the user wants to play a clip. The MainViewModel handles
    /// navigation to the Player view.
    /// </summary>
    public event EventHandler<ReplayClip>? PlayClipRequested;

    private readonly SettingsViewModel _settings;

    /// <summary>
    /// Persistent thumbnail cache directory inside the library folder.
    /// Avoids re-generating thumbnails on every refresh.
    /// </summary>
    private string ThumbnailCacheDir => Path.Combine(_settings.LibraryPath, ".thumbnails");

    public LibraryViewModel(SettingsViewModel settings)
    {
        Title = "Library";
        _settings = settings;
    }

    /// <summary>
    /// Scans the library directory for .mp4 files and populates the clip collection.
    /// Generates real video thumbnails via FFMpegCore for clips that don't already have a cached one.
    /// </summary>
    [RelayCommand]
    private async Task RefreshAsync()
    {
        IsLoading = true;

        try
        {
            Clips.Clear();

            string libraryDir = _settings.LibraryPath;
            if (!Directory.Exists(libraryDir))
            {
                Directory.CreateDirectory(libraryDir);
                return;
            }

            // Ensure thumbnail cache directory exists
            Directory.CreateDirectory(ThumbnailCacheDir);

            var files = Directory.GetFiles(libraryDir, "*.mp4")
                .OrderByDescending(File.GetCreationTime);

            foreach (var filePath in files)
            {
                var fileInfo = new FileInfo(filePath);
                Avalonia.Media.Imaging.Bitmap? avaloniaBitmap = null;
                TimeSpan duration = TimeSpan.Zero;
                string thumbPath = GetThumbnailCachePath(filePath);

                // ── Step 1: Get duration via FFProbe ──
                try
                {
                    var mediaInfo = await FFProbe.AnalyseAsync(filePath);
                    duration = mediaInfo.Duration;
                }
                catch (Exception ex)
                {
                    System.Diagnostics.Debug.WriteLine($"[FFProbe] Failed to get duration for {filePath}: {ex.Message}");
                }

                // ── Step 2: Generate or load cached thumbnail ──
                try
                {
                    // If cached thumbnail doesn't exist or is older than the video, regenerate
                    if (!File.Exists(thumbPath) || File.GetLastWriteTime(thumbPath) < fileInfo.LastWriteTime)
                    {
                        // Extract a frame at the 2-second mark (or at 0s for very short clips)
                        var snapshotTime = duration.TotalSeconds > 3 ? TimeSpan.FromSeconds(2) : TimeSpan.Zero;
                        await FFMpeg.SnapshotAsync(filePath, thumbPath, new Size(320, 180), snapshotTime);
                    }

                    if (File.Exists(thumbPath))
                    {
                        await using var fs = File.OpenRead(thumbPath);
                        avaloniaBitmap = new Avalonia.Media.Imaging.Bitmap(fs);
                    }
                }
                catch (Exception ex)
                {
                    System.Diagnostics.Debug.WriteLine($"[FFMpeg] Failed to generate thumbnail for {filePath}: {ex.Message}");
                }

                Clips.Add(new ReplayClip
                {
                    FilePath = filePath,
                    ThumbnailPath = thumbPath,
                    Thumbnail = avaloniaBitmap,
                    Duration = duration,
                    CreatedDate = fileInfo.CreationTime,
                    FileSize = fileInfo.Length
                });
            }
        }
        finally
        {
            IsLoading = false;
        }
    }

    /// <summary>
    /// Generates a deterministic thumbnail cache file path for the given video.
    /// </summary>
    private string GetThumbnailCachePath(string videoPath)
    {
        string fileName = Path.GetFileNameWithoutExtension(videoPath);
        return Path.Combine(ThumbnailCacheDir, $"{fileName}_thumb.jpg");
    }

    /// <summary>
    /// Opens the selected clip in the integrated video player.
    /// </summary>
    [RelayCommand]
    private void PlayClip(ReplayClip? clip)
    {
        if (clip != null)
            PlayClipRequested?.Invoke(this, clip);
    }

    /// <summary>
    /// Deletes the selected clip from disk and removes it from the collection.
    /// Also removes the associated thumbnail file.
    /// </summary>
    [RelayCommand]
    private async Task DeleteClipAsync(ReplayClip? clip)
    {
        if (clip == null) return;

        try
        {
            if (File.Exists(clip.FilePath))
                File.Delete(clip.FilePath);
            if (File.Exists(clip.ThumbnailPath))
                File.Delete(clip.ThumbnailPath);

            Clips.Remove(clip);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to delete clip: {ex.Message}");
        }
    }

    /// <summary>
    /// Opens the library directory in the system file explorer.
    /// </summary>
    [RelayCommand]
    private void OpenLibraryFolder()
    {
        try
        {
            var psi = new System.Diagnostics.ProcessStartInfo
            {
                FileName = _settings.LibraryPath,
                UseShellExecute = true
            };
            System.Diagnostics.Process.Start(psi);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to open folder: {ex.Message}");
        }
    }
}

