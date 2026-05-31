using CommunityToolkit.Mvvm.ComponentModel;

namespace Lag.Core;

/// <summary>
/// Base class for all ViewModels, providing <see cref="INotifyPropertyChanged"/>
/// support via CommunityToolkit.Mvvm source generators.
/// </summary>
public abstract partial class ViewModelBase : ObservableObject
{
    private string _title = string.Empty;

    /// <summary>
    /// Display title for this view, shown in navigation or window chrome.
    /// </summary>
    public string Title
    {
        get => _title;
        protected set => SetProperty(ref _title, value);
    }
}
