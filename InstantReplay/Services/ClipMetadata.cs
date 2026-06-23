using System;
using System.IO;
using System.Text.Json;

namespace Lag.Services;

/// <summary>
/// Per-clip metadata the file itself can't carry: the game/app that was captured, its exe (for
/// icon lookup), the favourite star and the "edited in the editor" flag. Stored as a sidecar JSON
/// next to the clip (<c>{clip}.json</c>) — self-contained, survives library-folder moves, and needs
/// no central index to stay in sync. The library scan ignores <c>.json</c> files, so sidecars never
/// show up as clips.
/// </summary>
public sealed class ClipMetadata
{
    /// <summary>Friendly name of the captured game/app (null/empty = desktop or unknown).</summary>
    public string? Game { get; set; }

    /// <summary>Captured process exe (e.g. "cs2.exe"), for icon lookup. Null for desktop.</summary>
    public string? Exe { get; set; }

    /// <summary>Marked as a favourite (the star).</summary>
    public bool Favorite { get; set; }

    /// <summary>Produced/overwritten by the built-in editor.</summary>
    public bool Edited { get; set; }
}

/// <summary>Reads/writes the <see cref="ClipMetadata"/> sidecar for a clip. Never throws.
/// Sidecars live in a HIDDEN cache (<c>%LocalAppData%\Lag\metadata</c>), keyed by a hash of the
/// clip path — NOT next to the user's clips, so the library folder stays clean (same idea as the
/// thumbnail cache). Sidecars older builds wrote next to the clip are auto-migrated on read.</summary>
public static class ClipMetadataStore
{
    private static readonly JsonSerializerOptions Options = new() { WriteIndented = true };

    private static string CacheDir => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Lag", "metadata");

    /// <summary>Hidden cache path for a clip's metadata (hash of the full clip path).</summary>
    private static string CachePath(string clipPath)
    {
        byte[] hash = System.Security.Cryptography.MD5.HashData(
            System.Text.Encoding.UTF8.GetBytes(clipPath.ToLowerInvariant()));
        return Path.Combine(CacheDir, $"{Convert.ToHexString(hash)}.json");
    }

    /// <summary>Legacy location (next to the clip) — older builds wrote here; migrated away on read.</summary>
    private static string LegacyPath(string clipPath) => clipPath + ".json";

    /// <summary>Loads the metadata, or null if there is none / it can't be read. A legacy next-to-clip
    /// sidecar is migrated into the cache (and the clutter removed) the first time it's read.</summary>
    public static ClipMetadata? Read(string clipPath)
    {
        try
        {
            string cache = CachePath(clipPath);
            if (File.Exists(cache))
                return JsonSerializer.Deserialize<ClipMetadata>(File.ReadAllText(cache));

            string legacy = LegacyPath(clipPath);
            if (File.Exists(legacy))
            {
                var meta = JsonSerializer.Deserialize<ClipMetadata>(File.ReadAllText(legacy));
                if (meta != null) Write(clipPath, meta);   // writes to the cache AND deletes the legacy file
                return meta;
            }
        }
        catch { /* best-effort */ }
        return null;
    }

    /// <summary>Writes (overwrites) the metadata into the cache and removes any legacy next-to-clip file.</summary>
    public static void Write(string clipPath, ClipMetadata meta)
    {
        try
        {
            Directory.CreateDirectory(CacheDir);
            File.WriteAllText(CachePath(clipPath), JsonSerializer.Serialize(meta, Options));
            string legacy = LegacyPath(clipPath);
            if (File.Exists(legacy)) { try { File.Delete(legacy); } catch { } }   // de-clutter the library folder
        }
        catch (Exception ex) { System.Diagnostics.Debug.WriteLine($"[ClipMeta] write failed: {ex.Message}"); }
    }

    /// <summary>Reads-modifies-writes the metadata (creating it if missing).</summary>
    public static void Update(string clipPath, Action<ClipMetadata> mutate)
    {
        var meta = Read(clipPath) ?? new ClipMetadata();
        mutate(meta);
        Write(clipPath, meta);
    }

    public static void SetFavorite(string clipPath, bool favorite) => Update(clipPath, m => m.Favorite = favorite);
    public static void SetEdited(string clipPath, bool edited) => Update(clipPath, m => m.Edited = edited);

    /// <summary>Removes the metadata for a clip (cache + any legacy next-to-clip file).</summary>
    public static void Delete(string clipPath)
    {
        try { string c = CachePath(clipPath); if (File.Exists(c)) File.Delete(c); } catch { }
        try { string l = LegacyPath(clipPath); if (File.Exists(l)) File.Delete(l); } catch { }
    }
}
