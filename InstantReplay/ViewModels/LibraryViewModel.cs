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

    // ───────────── Filtering / grouping (the visible library) ─────────────

    /// <summary>Search box text — matches a clip's name or game (case-insensitive).</summary>
    [ObservableProperty]
    private string _searchText = string.Empty;

    /// <summary>Favorites toggle in the filter row — when on, only starred clips show.</summary>
    [ObservableProperty]
    private bool _favoritesOnly;

    /// <summary>Active game filter (null = All). Set by clicking a game chip.</summary>
    private string? _gameFilter;

    /// <summary>Distinct games present across the clips — rendered as filter chips.</summary>
    public ObservableCollection<GameChip> GameChips { get; } = new();

    /// <summary>Clips bucketed by day (Today / Yesterday / date) after filtering — the grid source.</summary>
    public ObservableCollection<ClipGroup> Groups { get; } = new();

    /// <summary>True when there ARE clips but the current filter/search hides them all.</summary>
    [ObservableProperty]
    private bool _hasNoResults;

    /// <summary>Header clip count, e.g. "24 CLIPS" (mono, uppercase).</summary>
    public string ClipCountLabel => Lag.Core.Localizer.Format("Library_ClipCount", Clips.Count);

    partial void OnSearchTextChanged(string value) { ClearSelection(); RebuildGroups(); }
    partial void OnFavoritesOnlyChanged(bool value) { ClearSelection(); RebuildGroups(); }

    /// <summary>Game chip click: toggle the filter (click the active chip → back to All), then regroup.</summary>
    [RelayCommand]
    private void SetGameFilter(string? label)
    {
        _gameFilter = string.Equals(_gameFilter, label, StringComparison.OrdinalIgnoreCase) ? null : label;
        foreach (var ch in GameChips)
            ch.IsActive = string.Equals(ch.FilterKey, _gameFilter, StringComparison.OrdinalIgnoreCase);
        ClearSelection();
        RebuildGroups();
    }

    /// <summary>Favorites chip click: flip the favourites-only filter (regroups via the property handler).</summary>
    [RelayCommand]
    private void ToggleFavoritesOnly() => FavoritesOnly = !FavoritesOnly;

    /// <summary>Sentinel filter key for the "Desktop" chip (clips with no detected game). Deliberately not
    /// a valid game name so it can't collide with one.</summary>
    private const string DesktopFilterKey = "desktop";

    /// <summary>Rebuilds the game-chip list from the games actually present, preserving the active filter.
    /// Adds a "Desktop" chip for clips captured off the desktop (no game).</summary>
    private void RebuildGameChips()
    {
        var games = Clips.Where(c => c.HasGame).Select(c => c.Game!)
            .Distinct(StringComparer.OrdinalIgnoreCase)
            .OrderBy(g => g, StringComparer.CurrentCultureIgnoreCase)
            .ToList();
        bool desktopPresent = Clips.Any(c => !c.HasGame);

        // Drop a stale filter whose game (or the Desktop category) no longer exists.
        if (_gameFilter == DesktopFilterKey) { if (!desktopPresent) _gameFilter = null; }
        else if (_gameFilter != null && !games.Contains(_gameFilter, StringComparer.OrdinalIgnoreCase))
            _gameFilter = null;

        GameChips.Clear();
        foreach (var g in games)
            GameChips.Add(new GameChip(g, string.Equals(g, _gameFilter, StringComparison.OrdinalIgnoreCase)));
        if (desktopPresent)
            GameChips.Add(new GameChip(Localizer.Get("Library_Desktop"), _gameFilter == DesktopFilterKey, DesktopFilterKey));
    }

    /// <summary>Applies search + game + favourites filters, then buckets the result by day into <see cref="Groups"/>.</summary>
    private void RebuildGroups()
    {
        string q = (SearchText ?? string.Empty).Trim();

        IEnumerable<ReplayClip> filtered = Clips;
        if (FavoritesOnly)
            filtered = filtered.Where(c => c.IsFavorite);
        if (_gameFilter == DesktopFilterKey)
            filtered = filtered.Where(c => !c.HasGame);
        else if (_gameFilter != null)
            filtered = filtered.Where(c => string.Equals(c.Game, _gameFilter, StringComparison.OrdinalIgnoreCase));
        if (q.Length > 0)
            filtered = filtered.Where(c =>
                c.DisplayName.Contains(q, StringComparison.OrdinalIgnoreCase) ||
                (c.Game?.Contains(q, StringComparison.OrdinalIgnoreCase) ?? false));

        var list = filtered.ToList();
        var today = DateTime.Today;
        var ci = System.Globalization.CultureInfo.CurrentCulture;

        Groups.Clear();
        foreach (var bucket in list.GroupBy(c => c.CreatedDate.Date).OrderByDescending(g => g.Key))
        {
            DateTime date = bucket.Key;
            string label, sub;
            if (date == today)
            {
                label = Lag.Core.Localizer.Get("Library_Today");
                sub = date.ToString("d MMM", ci);
            }
            else if (date == today.AddDays(-1))
            {
                label = Lag.Core.Localizer.Get("Library_Yesterday");
                sub = date.ToString("d MMM", ci);
            }
            else
            {
                label = date.ToString("d MMMM", ci);
                sub = date.ToString("dddd", ci);
            }

            var group = new ClipGroup
            {
                Label = label.ToUpper(ci), // section headings are uppercase in the design
                Sub = sub,
                CountLabel = Lag.Core.Localizer.Format("Library_GroupCount", bucket.Count()),
            };
            foreach (var c in bucket) group.Items.Add(c);
            Groups.Add(group);
        }

        HasNoResults = Clips.Count > 0 && Groups.Count == 0;
    }

    /// <summary>Recomputes chips, groups and the header count after the clip set changes.</summary>
    private void RebuildView()
    {
        RebuildGameChips();
        RebuildGroups();
        OnPropertyChanged(nameof(ClipCountLabel));
    }

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

    /// <summary>All container formats the recorder can produce (must match SettingsViewModel.FormatOptions),
    /// plus *.gif from the editor's "Export as GIF" — GIFs are animated, so they're treated as videos
    /// (thumbnail + play) rather than still images.</summary>
    private static readonly string[] VideoPatterns = ["*.mp4", "*.mkv", "*.mov", "*.avi", "*.gif"];

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

    /// <summary>Drive usage as "used / total", e.g. "25.1 / 80 GB" (next to the indicator bar).</summary>
    [ObservableProperty]
    private string _usedSpaceDisplay = "—";

    /// <summary>Filled width (px) of the 120px usage bar, derived from <see cref="UsedSpacePercent"/>.</summary>
    public double UsedSpaceBarWidth => Math.Clamp(UsedSpacePercent, 0, 100) / 100.0 * 120.0;

    partial void OnUsedSpacePercentChanged(double value) => OnPropertyChanged(nameof(UsedSpaceBarWidth));

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
            double totalGb = drive.TotalSize / 1_073_741_824.0;
            double usedGb = totalGb - freeGb;
            FreeSpaceDisplay = freeGb >= 100 ? $"{freeGb:F0} GB" : $"{freeGb:F1} GB";
            UsedSpacePercent = 100.0 * (drive.TotalSize - drive.AvailableFreeSpace) / drive.TotalSize;
            UsedSpaceDisplay = $"{usedGb:F1} / {totalGb:F0} GB";
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
                    Lag.Services.ClipMetadataStore.Delete(file.FullName);

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
    /// <summary>Cancels an in-flight refresh so a newer one supersedes it (re-entrancy guard).</summary>
    private System.Threading.CancellationTokenSource? _refreshCts;

    [RelayCommand]
    private async Task RefreshAsync()
    {
        // A refresh runs on NavigateToLibrary AND after every saved replay, so two can overlap.
        // Two refreshes mutating the UI-bound Clips collection at once desync the ItemsControl's
        // container generator → ArgumentOutOfRangeException (the crash that froze the whole app and
        // lagged the cursor). Cancel any in-flight refresh; only the newest publishes its result.
        _refreshCts?.Cancel();
        var cts = new System.Threading.CancellationTokenSource();
        _refreshCts = cts;
        var token = cts.Token;

        IsLoading = true;
        try
        {
            string libraryDir = _settings.LibraryPath;

            // ALL scanning, ffprobe and thumbnail generation runs OFF the UI thread, building a local
            // list. The UI-bound collection is then touched exactly once, on the UI thread — this
            // both removes the crash (no concurrent/again-on-UI mutation mid-virtualization) and the
            // lag (heavy ffmpeg/bitmap work no longer runs on continuations of the UI thread).
            var built = await Task.Run(async () =>
            {
                EnforceStorageQuota(); // keep under the size quota before scanning

                var result = new List<ReplayClip>();
                if (!Directory.Exists(libraryDir)) { Directory.CreateDirectory(libraryDir); return result; }

                Directory.CreateDirectory(ThumbnailCacheDir);
                CleanupLegacyThumbnailFolder(libraryDir);

                var files = GetMediaFiles(libraryDir).OrderByDescending(File.GetCreationTime);
                foreach (var filePath in files)
                {
                    if (token.IsCancellationRequested) break;

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
                            if (!File.Exists(thumbPath) || File.GetLastWriteTime(thumbPath) < fileInfo.LastWriteTime)
                            {
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

                    // Per-clip sidecar metadata (game/app, favourite, edited). "-edit" in the name is
                    // a fallback "edited" signal for clips the editor named that way without a sidecar.
                    var meta = Lag.Services.ClipMetadataStore.Read(filePath);
                    bool edited = (meta?.Edited ?? false) ||
                        Path.GetFileNameWithoutExtension(filePath).Contains("-edit", StringComparison.OrdinalIgnoreCase);

                    result.Add(new ReplayClip
                    {
                        FilePath = filePath,
                        ThumbnailPath = thumbPath,
                        Thumbnail = avaloniaBitmap,
                        Duration = duration,
                        CreatedDate = fileInfo.CreationTime,
                        FileSize = fileInfo.Length,
                        IsImage = isImage,
                        Game = meta?.Game,
                        Exe = meta?.Exe,
                        IsFavorite = meta?.Favorite ?? false,
                        IsEdited = edited
                    });
                }
                return result;
            }, token);

            // A newer refresh started while we were building → drop this (stale) result untouched.
            if (token.IsCancellationRequested || !ReferenceEquals(_refreshCts, cts)) return;

            // Publish to the UI-bound collection — on the UI thread (continuation of the await), once.
            Clips.Clear();
            foreach (var c in built) Clips.Add(c);
            _selectionAnchor = null; // clips are fresh instances — old selection/anchor is gone

            // Rebuild the filter chips + day groups from the freshly published clips.
            RebuildView();
        }
        catch (OperationCanceledException) { /* superseded by a newer refresh */ }
        finally
        {
            // Only the newest refresh owns the shared UI state / loading flag.
            if (ReferenceEquals(_refreshCts, cts))
            {
                UpdateFreeSpace();
                OnPropertyChanged(nameof(LibraryStats));
                NotifySelectionChanged(); // rebuilt list starts with nothing selected
                IsLoading = false;
            }
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

    /// <summary>Card hover overlay "Edit" button: opens the clip in the editor (no-op for screenshots).</summary>
    [RelayCommand]
    private void RequestEditClip(ReplayClip? clip)
    {
        if (clip != null) RequestEdit(clip);
    }

    /// <summary>Card star button: toggles the favourite flag and persists it to the clip's sidecar.</summary>
    [RelayCommand]
    private void ToggleFavorite(ReplayClip? clip)
    {
        if (clip == null) return;
        clip.IsFavorite = !clip.IsFavorite;
        Lag.Services.ClipMetadataStore.SetFavorite(clip.FilePath, clip.IsFavorite);
        RebuildGroups(); // un-starring while "Favorites" is on should drop it from view
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
            Lag.Services.ClipMetadataStore.Delete(clip.FilePath);

            Clips.Remove(clip);
            RebuildView(); // drop it from the groups + refresh chips/count
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to delete clip: {ex.Message}");
        }

        NotifySelectionChanged();
    }

    // ───────────── Multi-select (Explorer-style) ─────────────

    /// <summary>Anchor clip for Shift-range selection (the last clip a plain/Ctrl click landed on).</summary>
    private ReplayClip? _selectionAnchor;

    /// <summary>Any cards selected → the bulk "Delete (N)" + "Clear" buttons show, and we're in
    /// selection mode (a plain card click then toggles selection instead of playing).</summary>
    public bool HasSelection => Clips.Any(c => c.IsSelected);

    /// <summary>Inverse of <see cref="HasSelection"/> — bound by the card hover overlay so the
    /// play/star/edit actions hide while a selection is in progress.</summary>
    public bool IsNotSelecting => !HasSelection;

    /// <summary>Header button caption, e.g. "Delete (3)" (reuses the Library_Delete string).</summary>
    public string DeleteSelectedLabel =>
        $"{Lag.Core.Localizer.Get("Library_Delete")} ({Clips.Count(c => c.IsSelected)})";

    private void NotifySelectionChanged()
    {
        OnPropertyChanged(nameof(HasSelection));
        OnPropertyChanged(nameof(IsNotSelecting));
        OnPropertyChanged(nameof(DeleteSelectedLabel));
    }

    /// <summary>The clips in the exact order they are displayed (groups top-to-bottom, cards
    /// within a group) — the order Shift-range selection walks over.</summary>
    private List<ReplayClip> VisibleClips() => Groups.SelectMany(g => g.Items).ToList();

    /// <summary>Click the corner circle / a card while selecting / Ctrl+click: flip this clip's
    /// membership (accumulating — never clears the rest) and make it the new range anchor.</summary>
    public void ToggleSelectClip(ReplayClip clip)
    {
        clip.IsSelected = !clip.IsSelected;
        if (clip.IsSelected) _selectionAnchor = clip;
        NotifySelectionChanged();
    }

    /// <summary>Shift+click: select the contiguous run between the anchor and this clip
    /// (replacing the selection). With no anchor it just adds this clip.</summary>
    public void SelectRange(ReplayClip clip)
    {
        var order = VisibleClips();
        int to = order.IndexOf(clip);
        if (to < 0) return;
        int from = _selectionAnchor != null ? order.IndexOf(_selectionAnchor) : -1;
        if (from < 0) { clip.IsSelected = true; _selectionAnchor = clip; NotifySelectionChanged(); return; }

        int lo = Math.Min(from, to), hi = Math.Max(from, to);
        for (int i = 0; i < order.Count; i++) order[i].IsSelected = i >= lo && i <= hi;
        NotifySelectionChanged(); // anchor stays put for subsequent shift-clicks
    }

    /// <summary>Ctrl+A: select every clip currently shown.</summary>
    [RelayCommand]
    private void SelectAll()
    {
        foreach (var c in VisibleClips()) c.IsSelected = true;
        NotifySelectionChanged();
    }

    /// <summary>Esc / click on empty space: drop the whole selection.</summary>
    [RelayCommand]
    private void ClearSelection()
    {
        foreach (var c in Clips) c.IsSelected = false;
        _selectionAnchor = null;
        NotifySelectionChanged();
    }

    /// <summary>Header button: deletes every selected clip (files + thumbnails).</summary>
    [RelayCommand]
    private async Task DeleteSelectedAsync()
    {
        foreach (var clip in Clips.Where(c => c.IsSelected).ToList())
            await DeleteClipAsync(clip);
        _selectionAnchor = null;
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

/// <summary>A game filter chip in the library header (label + live active state for styling).</summary>
public partial class GameChip : ObservableObject
{
    public string Label { get; }

    /// <summary>The value passed to the filter command. Equals <see cref="Label"/> for a game chip; a fixed
    /// sentinel for the "Desktop" chip (so a real game literally named "Desktop" can't collide with it).</summary>
    public string FilterKey { get; }

    [ObservableProperty]
    private bool _isActive;

    public GameChip(string label, bool active, string? filterKey = null)
    {
        Label = label;
        FilterKey = filterKey ?? label;
        _isActive = active;
    }
}

/// <summary>A day-bucket of clips (Today / Yesterday / a date) rendered as one library section.</summary>
public class ClipGroup
{
    /// <summary>Section heading, e.g. "Today" or "18 June".</summary>
    public string Label { get; init; } = string.Empty;

    /// <summary>Secondary heading text (the date for Today/Yesterday, the weekday otherwise).</summary>
    public string Sub { get; init; } = string.Empty;

    /// <summary>Localized "N clips" count shown at the right of the section header.</summary>
    public string CountLabel { get; init; } = string.Empty;

    /// <summary>The clips in this day bucket.</summary>
    public ObservableCollection<ReplayClip> Items { get; init; } = new();
}

