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

/// <summary>Reads/writes the <see cref="ClipMetadata"/> sidecar for a clip. Never throws.</summary>
public static class ClipMetadataStore
{
    private static readonly JsonSerializerOptions Options = new() { WriteIndented = true };

    /// <summary>Sidecar path for a clip: the full clip path + ".json" (so "a.mkv" → "a.mkv.json").</summary>
    public static string SidecarPath(string clipPath) => clipPath + ".json";

    /// <summary>Loads the sidecar, or null if there is none / it can't be read.</summary>
    public static ClipMetadata? Read(string clipPath)
    {
        try
        {
            string p = SidecarPath(clipPath);
            return File.Exists(p) ? JsonSerializer.Deserialize<ClipMetadata>(File.ReadAllText(p)) : null;
        }
        catch { return null; }
    }

    /// <summary>Writes (overwrites) the sidecar for a clip.</summary>
    public static void Write(string clipPath, ClipMetadata meta)
    {
        try { File.WriteAllText(SidecarPath(clipPath), JsonSerializer.Serialize(meta, Options)); }
        catch (Exception ex) { System.Diagnostics.Debug.WriteLine($"[ClipMeta] write failed: {ex.Message}"); }
    }

    /// <summary>Reads-modifies-writes the sidecar (creating it if missing).</summary>
    public static void Update(string clipPath, Action<ClipMetadata> mutate)
    {
        var meta = Read(clipPath) ?? new ClipMetadata();
        mutate(meta);
        Write(clipPath, meta);
    }

    public static void SetFavorite(string clipPath, bool favorite) => Update(clipPath, m => m.Favorite = favorite);
    public static void SetEdited(string clipPath, bool edited) => Update(clipPath, m => m.Edited = edited);

    /// <summary>Removes the sidecar (call when the clip is deleted).</summary>
    public static void Delete(string clipPath)
    {
        try { string p = SidecarPath(clipPath); if (File.Exists(p)) File.Delete(p); }
        catch { /* best-effort */ }
    }
}
