using System;
using System.Globalization;
using Avalonia;
using Avalonia.Data.Converters;

namespace Lag.Core;

/// <summary>
/// Maps a language code ("en", "uk", ...) to its vector flag resource ("Flag_en", ...).
/// The flags are DrawingImage resources defined in App.axaml — crisp at any DPI,
/// no emoji-font or bitmap dependency.
/// </summary>
public class LanguageToFlagConverter : IValueConverter
{
    public object? Convert(object? value, Type targetType, object? parameter, CultureInfo culture)
    {
        if (value is string code &&
            Application.Current is { } app &&
            app.TryGetResource($"Flag_{code}", null, out var resource))
        {
            return resource;
        }
        return null;
    }

    public object? ConvertBack(object? value, Type targetType, object? parameter, CultureInfo culture)
        => throw new NotSupportedException();
}
