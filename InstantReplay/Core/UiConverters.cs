using System;
using System.Globalization;
using Avalonia.Data.Converters;

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

/// <summary>Uppercases a string for display ("mp4" → "MP4").</summary>
public class StringToUpperConverter : IValueConverter
{
    public object? Convert(object? value, Type targetType, object? parameter, CultureInfo culture)
        => value?.ToString()?.ToUpperInvariant();

    public object? ConvertBack(object? value, Type targetType, object? parameter, CultureInfo culture)
        => throw new NotSupportedException();
}
