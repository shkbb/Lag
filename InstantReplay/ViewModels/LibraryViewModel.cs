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

    /// <summary>
    /// Fired when the user wants to edit a clip (context menu → Edit).
    /// The MainViewModel handles navigation to the Editor view.
    /// </summary>
    public event EventHandler<ReplayClip>? EditClipRequested;

    /// <summary>Raises <see cref="EditClipRequested"/> (called from the view's context menu).</summary>
    public void RequestEdit(ReplayClip clip)
    {
        if (clip.IsImage) return; // screenshots have nothing to trim
        EditClipRequested?.Invoke(this, clip);
    }


    private readonly SettingsViewModel _settings;

    /// <summary>All container formats the recorder can produce (must match SettingsViewModel.FormatOptions).</summary>
    private static readonly string[] VideoPatterns = ["*.mp4", "*.mkv", "*.mov", "*.avi"];

    /// <summary>Screenshot formats produced by the screenshot hotkey (shown as image cards).</summary>
    private static readonly string[] ImagePatterns = ["*.png", "*.jpg", "*.jpeg"];

    /// <summary>Enumerates every saved replay regardless of its container format.</summary>
    private static IEnumerable<string> GetVideoFiles(string dir) =>
        VideoPatterns.SelectMany(p => Directory.GetFiles(dir, p));

    /// <summary>Enumerates replays AND screenshots living in the library folder.</summary>
    private static IEnumerable<string> GetMediaFiles(string dir) =>
        VideoPatterns.Concat(ImagePatterns).SelectMany(p => Directory.GetFiles(dir, p));

    private static bool IsImageFile(string path) =>
        Path.GetExtension(path).ToLowerInvariant() is ".png" or ".jpg" or ".jpeg";

    /// <summary>
    /// Persistent thumbnail cache. Lives in %LocalAppData%\Lag\thumbnails — NOT inside the
    /// user's library folder, so no visible ".thumbnails" clutter next to their videos.
    /// </summary>
    private static string ThumbnailCacheDir => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "Lag", "thumbnails");

    /// <summary>
    /// One-time migration: removes the legacy visible ".thumbnails" folder that older versions
    /// created inside the library. Thumbnails simply regenerate into the new hidden cache.
    /// </summary>
    private void CleanupLegacyThumbnailFolder(string libraryDir)
    {
        try
        {
            string legacy = Path.Combine(libraryDir, ".thumbnails");
            if (Directory.Exists(legacy))
                Directory.Delete(legacy, recursive: true);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Library] Legacy thumbnail cleanup failed: {ex.Message}");
        }
    }

    // ───────────── Disk Space Management ─────────────

    /// <summary>Human-readable free space on the drive hosting the library (e.g. "123.4 GB").</summary>
    [ObservableProperty]
    private string _freeSpaceDisplay = "—";

    /// <summary>Used percentage of the library drive (0–100), drives the indicator bar.</summary>
    [ObservableProperty]
    private double _usedSpacePercent;

    /// <summary>Header subtitle per the design: "24 кліпи · вільно 412 GB" (localized).</summary>
    public string LibraryStats => Lag.Core.Localizer.Format("Library_Stats", Clips.Count, FreeSpaceDisplay);

    /// <summary>
    /// Refreshes the free-space indicator from the drive that hosts the library folder.
    /// </summary>
    private void UpdateFreeSpace()
    {
        try
        {
            string? root = Path.GetPathRoot(Path.GetFullPath(_settings.LibraryPath));
            if (string.IsNullOrEmpty(root)) return;

            var drive = new DriveInfo(root);
            double freeGb = drive.AvailableFreeSpace / 1_073_741_824.0;
            FreeSpaceDisplay = freeGb >= 100 ? $"{freeGb:F0} GB" : $"{freeGb:F1} GB";
            UsedSpacePercent = 100.0 * (drive.TotalSize - drive.AvailableFreeSpace) / drive.TotalSize;
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Storage] Free-space check failed: {ex.Message}");
        }
    }

    /// <summary>
    /// OPT-IN auto-cleanup: does nothing unless the user enabled it in Settings. When enabled and
    /// the total size of saved clips exceeds the user-chosen limit, deletes the OLDEST clips
    /// (by CreationTime) until the folder is back under it. Thumbnails of deleted clips are
    /// removed too. Runs on startup and before every refresh (a refresh follows every saved
    /// replay, so new clips trigger the check automatically).
    /// </summary>
    private void EnforceStorageQuota()
    {
        // Respect the user's choice — auto-cleanup is a feature you turn on, not a default.
        if (!_settings.AutoCleanupEnabled) return;

        long maxLibraryBytes = (long)_settings.SelectedStorageLimit.Gb * 1024 * 1024 * 1024;
        if (maxLibraryBytes <= 0) return;

        try
        {
            string libraryDir = _settings.LibraryPath;
            if (!Directory.Exists(libraryDir)) return;

            var files = GetVideoFiles(libraryDir)
                .Select(f => new FileInfo(f))
                .OrderBy(f => f.CreationTime) // oldest first
                .ToList();

            long totalBytes = files.Sum(f => f.Length);
            if (totalBytes <= maxLibraryBytes) return;

            foreach (var file in files)
            {
                if (totalBytes <= maxLibraryBytes) break;

                try
                {
                    long size = file.Length;
                    file.Delete();

                    string thumb = GetThumbnailCachePath(file.FullName);
                    if (File.Exists(thumb)) File.Delete(thumb);

                    totalBytes -= size;
                    Console.WriteLine($"[Storage] Quota cleanup: deleted oldest clip '{file.Name}' ({size / 1_048_576} MB).");
                }
                catch (Exception ex)
                {
                    System.Diagnostics.Debug.WriteLine($"[Storage] Failed to delete '{file.Name}': {ex.Message}");
                }
            }
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Storage] Quota enforcement failed: {ex.Message}");
        }
    }

    public LibraryViewModel(SettingsViewModel settings)
    {
        Title = "Library";
        _settings = settings;

        // Startup pass: enforce the quota and prime the free-space indicator without blocking DI.
        _ = Task.Run(() =>
        {
            EnforceStorageQuota();
            Avalonia.Threading.Dispatcher.UIThread.Post(UpdateFreeSpace);
        });
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
            // Keep the library under the size quota BEFORE scanning, so clips removed by the
            // cleanup never appear in the gallery. A refresh runs after every saved replay.
            await Task.Run(EnforceStorageQuota);

            Clips.Clear();

            string libraryDir = _settings.LibraryPath;
            if (!Directory.Exists(libraryDir))
            {
                Directory.CreateDirectory(libraryDir);
                return;
            }

            // Ensure the (hidden, LocalAppData) thumbnail cache exists; drop the legacy
            // visible ".thumbnails" folder older versions kept inside the library.
            Directory.CreateDirectory(ThumbnailCacheDir);
            CleanupLegacyThumbnailFolder(libraryDir);

            var files = GetMediaFiles(libraryDir)
                .OrderByDescending(File.GetCreationTime);

            foreach (var filePath in files)
            {
                var fileInfo = new FileInfo(filePath);
                Avalonia.Media.Imaging.Bitmap? avaloniaBitmap = null;
                TimeSpan duration = TimeSpan.Zero;
                bool isImage = IsImageFile(filePath);
                string thumbPath = isImage ? filePath : GetThumbnailCachePath(filePath);

                if (isImage)
                {
                    // Screenshots are their own thumbnail — just decode them downscaled.
                    try
                    {
                        await using var fs = File.OpenRead(filePath);
                        avaloniaBitmap = Avalonia.Media.Imaging.Bitmap.DecodeToWidth(fs, 640);
                    }
                    catch (Exception ex)
                    {
                        System.Diagnostics.Debug.WriteLine($"[Library] Failed to load screenshot {filePath}: {ex.Message}");
                    }
                }
                else
                {
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
                }

                Clips.Add(new ReplayClip
                {
                    FilePath = filePath,
                    ThumbnailPath = thumbPath,
                    Thumbnail = avaloniaBitmap,
                    Duration = duration,
                    CreatedDate = fileInfo.CreationTime,
                    FileSize = fileInfo.Length,
                    IsImage = isImage
                });
            }
        }
        finally
        {
            UpdateFreeSpace();
            OnPropertyChanged(nameof(LibraryStats));
            NotifySelectionChanged(); // rebuilt list starts with nothing selected
            IsLoading = false;
        }
    }

    /// <summary>
    /// Deterministic thumbnail path for a video: the file name is an MD5 hash of the FULL
    /// video path, so clips with identical names in different folders never collide and the
    /// cache survives library-folder changes.
    /// </summary>
    private static string GetThumbnailCachePath(string videoPath)
    {
        byte[] hash = System.Security.Cryptography.MD5.HashData(
            System.Text.Encoding.UTF8.GetBytes(videoPath.ToLowerInvariant()));
        return Path.Combine(ThumbnailCacheDir, $"{Convert.ToHexString(hash)}.jpg");
    }

    /// <summary>
    /// Opens the selected clip in the integrated video player.
    /// </summary>
    [RelayCommand]
    private void PlayClip(ReplayClip? clip)
    {
        if (clip == null) return;
        // Videos play, screenshots display — the player view handles both.
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

        NotifySelectionChanged();
    }

    // ───────────── Multi-select (bulk delete) ─────────────

    /// <summary>Any cards selected → the bulk "Delete (N)" button is shown in the header.</summary>
    public bool HasSelection => Clips.Any(c => c.IsSelected);

    /// <summary>Header button caption, e.g. "Delete (3)" (reuses the Library_Delete string).</summary>
    public string DeleteSelectedLabel =>
        $"{Lag.Core.Localizer.Get("Library_Delete")} ({Clips.Count(c => c.IsSelected)})";

    private void NotifySelectionChanged()
    {
        OnPropertyChanged(nameof(HasSelection));
        OnPropertyChanged(nameof(DeleteSelectedLabel));
    }

    /// <summary>Card hover button: toggles the clip's membership in the selection.</summary>
    [RelayCommand]
    private void ToggleSelect(ReplayClip? clip)
    {
        if (clip == null) return;
        clip.IsSelected = !clip.IsSelected;
        NotifySelectionChanged();
    }

    /// <summary>Header button: deletes every selected clip (files + thumbnails).</summary>
    [RelayCommand]
    private async Task DeleteSelectedAsync()
    {
        foreach (var clip in Clips.Where(c => c.IsSelected).ToList())
            await DeleteClipAsync(clip);
    }

    /// <summary>
    /// Opens Windows Explorer with the given clip selected ("Show in folder").
    /// </summary>
    [RelayCommand]
    private void ShowInFolder(ReplayClip? clip)
    {
        if (clip == null || !File.Exists(clip.FilePath)) return;

        try
        {
            System.Diagnostics.Process.Start("explorer.exe", $"/select,\"{clip.FilePath}\"");
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to show clip in folder: {ex.Message}");
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

