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
/// Two-way "1 − x" for int indexes. Bridges a segmented control whose VISUAL order is the
/// reverse of the bound property's value order (e.g. mic channels: UI shows [Mono, Stereo]
/// but the setting is 0 = stereo, 1 = mono), so SelectedIndex can bind directly.
/// </summary>
public class OneMinusIntConverter : IValueConverter
{
    public object? Convert(object? value, Type targetType, object? parameter, CultureInfo culture)
        => value is int i ? 1 - i : value;

    public object? ConvertBack(object? value, Type targetType, object? parameter, CultureInfo culture)
        => value is int i ? 1 - i : value;
}

/// <summary>
/// Maps a quality "intensity tier" (0 = normal, 1 = caution, 2 = extreme) to a text brush, so
/// dropdown options for high FPS / bitrate / resolution stand out: tier 0 keeps the
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

/// <summary>Maps a 0..1 level to a pixel width for a manual level meter: <c>level × parameter</c>,
/// where the parameter is the meter's track width. Lets the fill bar respond instantly (no
/// ProgressBar value-transition lag).</summary>
public class LevelToWidthConverter : IValueConverter
{
    public object Convert(object? value, Type targetType, object? parameter, CultureInfo culture)
    {
        // value can be a 0..1 level (MicLevel) or a 0..100 threshold — the parameter is the per-unit
        // pixel scale (e.g. 480 for a level, 4.8 for a 0..100 threshold across a 480px bar).
        double level = value switch { double d => d, float f => f, int i => i, _ => 0 };
        double scale = parameter switch
        {
            string s when double.TryParse(s, NumberStyles.Any, CultureInfo.InvariantCulture, out var m) => m,
            double pd => pd,
            _ => 0,
        };
        double w = level * scale;
        return w < 0 ? 0 : w;
    }

    public object? ConvertBack(object? value, Type targetType, object? parameter, CultureInfo culture)
        => throw new NotSupportedException();
}
