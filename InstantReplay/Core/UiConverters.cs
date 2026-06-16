using System;
using System.Globalization;
using Avalonia;
using Avalonia.Data.Converters;
using Avalonia.Media;

namespace Lag.Core;

/// <summary>
/// True when the bound value equals the converter parameter (string-compared).
/// Drives "selected" classes on segmented-control buttons (Figma design).
/// ConvertBack returns the parameter so two-way IsChecked-style bindings can set the value.
/// </summary>
public class EqualsConverter : IValueConverter
{
    public object? Convert(object? value, Type targetType, object? parameter, CultureInfo culture)
        => string.Equals(value?.ToString(), parameter?.ToString(), StringComparison.Ordinal);

    public object? ConvertBack(object? value, Type targetType, object? parameter, CultureInfo culture)
        => throw new NotSupportedException();
}

/// <summary>
/// Maps a quality "intensity tier" (0 = normal, 1 = caution, 2 = extreme) to a text brush, so
/// dropdown options for high FPS / bitrate / resolution stand out (Medal-style): tier 0 keeps the
/// inherited theme colour, tier 1 is amber, tier 2 is red.
/// </summary>
public class IntensityToBrushConverter : IValueConverter
{
    private static readonly IBrush Caution = new SolidColorBrush(Color.Parse("#E5B84C")); // amber
    private static readonly IBrush Extreme = new SolidColorBrush(Color.Parse("#E0564C")); // red

    public object? Convert(object? value, Type targetType, object? parameter, CultureInfo culture)
    {
        int tier = value is int i ? i : 0;
        return tier switch
        {
            >= 2 => Extreme,
            1 => Caution,
            _ => AvaloniaProperty.UnsetValue,   // inherit the default text colour
        };
    }

    public object? ConvertBack(object? value, Type targetType, object? parameter, CultureInfo culture)
        => throw new NotSupportedException();
}

/// <summary>Uppercases a string for display ("mp4" → "MP4").</summary>
public class StringToUpperConverter : IValueConverter
{
    public object? Convert(object? value, Type targetType, object? parameter, CultureInfo culture)
        => value?.ToString()?.ToUpperInvariant();

    public object? ConvertBack(object? value, Type targetType, object? parameter, CultureInfo culture)
        => throw new NotSupportedException();
}
