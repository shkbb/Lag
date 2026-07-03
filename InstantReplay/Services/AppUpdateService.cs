using System;
using System.Diagnostics;
using System.IO;
using System.Net.Http;
using System.Reflection;
using System.Text.Json;
using System.Threading.Tasks;
using Microsoft.Win32;

namespace Lag.Services;

/// <summary>How this copy of the app is deployed — decides what "update" means for it.</summary>
public enum InstallKind
{
    /// <summary>Loose build (bin/, portable zip, IDE run) — never auto-updates.</summary>
    DevOrPortable,

    /// <summary>Installed by the Inno Setup installer (uninstall key matches our folder).</summary>
    Installed,

    /// <summary>Running out of the legacy Velopack layout (%LocalAppData%\Lag\current) —
    /// updating means MIGRATING onto the installer (which cleans the old layout up).</summary>
    LegacyVelopack,
}

/// <summary>A published release the app can move to.</summary>
public sealed record UpdateInfo(Version Version, string SetupUrl);

/// <summary>
/// The app's own update pipeline over GitHub Releases + the Inno Setup installer (replaces
/// Velopack): detects how this copy is deployed, asks the GitHub API for the latest release,
/// downloads Lag-win-Setup.exe and hands off to a silent install that relaunches the app
/// (/VERYSILENT /AUTOSTART). Legacy Velopack installs take the same path even at an equal
/// version — the installer migrates them and removes the old layout.
/// </summary>
public static class AppUpdateService
{
    private const string ApiLatestUrl = "https://api.github.com/repos/shkbb/Lag/releases/latest";
    private const string SetupAssetName = "Lag-win-Setup.exe";

    /// <summary>AppId declared in installer/Lag.iss; Inno appends "_is1" to form the uninstall key.</summary>
    private const string InnoUninstallSubKey = @"Software\Microsoft\Windows\CurrentVersion\Uninstall\Lag_is1";

    public static InstallKind DetectInstall()
    {
        string exeDir = AppContext.BaseDirectory.TrimEnd(Path.DirectorySeparatorChar);

        // Legacy Velopack layout: ...\Lag\current\Lag.exe with the updater stub one level up.
        try
        {
            string? parent = Path.GetDirectoryName(exeDir);
            if (parent != null
                && exeDir.EndsWith("current", StringComparison.OrdinalIgnoreCase)
                && File.Exists(Path.Combine(parent, "Update.exe")))
                return InstallKind.LegacyVelopack;
        }
        catch { /* fall through */ }

        // Inno install: the uninstall key's InstallLocation is the folder we run from.
        // Per-user installs land in HKCU, per-machine in HKLM — check both.
        try
        {
            foreach (var root in new[] { Registry.CurrentUser, Registry.LocalMachine })
            {
                using var key = root.OpenSubKey(InnoUninstallSubKey);
                if (key?.GetValue("InstallLocation") is string loc && !string.IsNullOrEmpty(loc)
                    && string.Equals(loc.TrimEnd(Path.DirectorySeparatorChar), exeDir,
                                     StringComparison.OrdinalIgnoreCase))
                    return InstallKind.Installed;
            }
        }
        catch { /* fall through */ }

        return InstallKind.DevOrPortable;
    }

    /// <summary>The running build's version (stamped by the release recipe via -p:Version).
    /// Null when it can't be read — callers should treat that as "don't update".</summary>
    public static Version? CurrentVersion()
    {
        try
        {
            string? info = Assembly.GetEntryAssembly()?
                .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion;
            if (string.IsNullOrEmpty(info)) return null;

            // "1.7.0", "1.7.0+g23ea66d" or "1.7.0-beta" — the leading dotted part is the version.
            int cut = info.IndexOfAny(new[] { '+', '-' });
            if (cut > 0) info = info[..cut];
            return Version.TryParse(info, out var v) ? Normalize(v) : null;
        }
        catch { return null; }
    }

    /// <summary>Latest published release with a setup asset, or null (no release / offline / no asset).</summary>
    public static async Task<UpdateInfo?> GetLatestAsync()
    {
        try
        {
            using var http = NewHttp();
            using var doc = JsonDocument.Parse(await http.GetStringAsync(ApiLatestUrl));

            string? tag = doc.RootElement.GetProperty("tag_name").GetString();
            if (string.IsNullOrEmpty(tag)) return null;
            if (!Version.TryParse(tag.TrimStart('v', 'V'), out var version)) return null;

            foreach (var asset in doc.RootElement.GetProperty("assets").EnumerateArray())
            {
                if (string.Equals(asset.GetProperty("name").GetString(), SetupAssetName,
                                  StringComparison.OrdinalIgnoreCase)
                    && asset.GetProperty("browser_download_url").GetString() is { Length: > 0 } url)
                    return new UpdateInfo(Normalize(version), url);
            }
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[AppUpdate] release lookup failed: {ex.Message}");
        }
        return null;
    }

    /// <summary>
    /// Downloads the installer and starts it silently; the installer closes any leftover app
    /// instance, installs over the existing location (or migrates a Velopack layout), and
    /// relaunches the app (/AUTOSTART). Returns true when the handoff started — THE CALLER
    /// MUST EXIT THE PROCESS immediately after.
    /// </summary>
    public static async Task<bool> DownloadAndInstallAsync(UpdateInfo update)
    {
        try
        {
            string setupPath = Path.Combine(Path.GetTempPath(), $"Lag-Setup-{update.Version}.exe");

            using (var http = NewHttp())
            await using (var src = await http.GetStreamAsync(update.SetupUrl))
            await using (var dst = File.Create(setupPath))
                await src.CopyToAsync(dst);

            Process.Start(new ProcessStartInfo
            {
                FileName = setupPath,
                Arguments = "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /AUTOSTART",
                UseShellExecute = true,
            });
            Console.WriteLine($"[AppUpdate] handed off to installer {update.Version} — exiting.");
            return true;
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[AppUpdate] download/install failed: {ex.Message}");
            return false;
        }
    }

    private static HttpClient NewHttp()
    {
        var http = new HttpClient { Timeout = TimeSpan.FromMinutes(5) };
        // The GitHub API rejects requests without a User-Agent.
        http.DefaultRequestHeaders.UserAgent.ParseAdd("Lag-Updater");
        return http;
    }

    /// <summary>"1.7" and "1.7.0" must compare equal — pad the missing parts with zeros.</summary>
    private static Version Normalize(Version v) =>
        new(v.Major, Math.Max(0, v.Minor), Math.Max(0, v.Build), Math.Max(0, v.Revision));
}
