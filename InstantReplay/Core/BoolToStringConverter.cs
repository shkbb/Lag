using Avalonia.Data.Converters;
using System.Globalization;

namespace Lag.Core;

/// <summary>
/// Converts a boolean value to one of two strings.
/// Usage in AXAML: Converter={StaticResource BoolToStringConverter} ConverterParameter='TrueText|FalseText'
/// </summary>
public class BoolToStringConverter : IValueConverter
{
    public static readonly BoolToStringConverter Instance = new();

    public object? Convert(object? value, Type targetType, object? parameter, CultureInfo culture)
    {
        if (value is bool b && parameter is string param)
        {
            var parts = param.Split('|');
            if (parts.Length == 2)
                return b ? parts[0] : parts[1];
        }
        return value?.ToString();
    }

    public object? ConvertBack(object? value, Type targetType, object? parameter, CultureInfo culture)
    {
        throw new NotSupportedException();
    }
}
