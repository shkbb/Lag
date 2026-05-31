using Avalonia;

namespace Lag.Core;

/// <summary>
/// Lightweight resolver for localized strings that live in the merged language
/// ResourceDictionary (see App.SetLanguage). Used by ViewModels for runtime status
/// messages. XAML uses {DynamicResource Key} directly, which updates live on switch.
/// </summary>
public static class Localizer
{
    /// <summary>Returns the localized string for <paramref name="key"/>, or the key itself if missing.</summary>
    public static string Get(string key)
    {
        if (Application.Current is { } app &&
            app.TryGetResource(key, null, out var value) &&
            value is string s)
        {
            return s;
        }
        return key;
    }

    /// <summary>Localized <see cref="string.Format(string, object[])"/> using the value for <paramref name="key"/>.</summary>
    public static string Format(string key, params object?[] args) => string.Format(Get(key), args);
}
