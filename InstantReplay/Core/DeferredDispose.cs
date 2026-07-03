using System;
using Avalonia.Threading;

namespace Lag.Core;

/// <summary>
/// Disposes UI-bound resources (bitmaps) a safe moment LATER instead of immediately.
/// Disposing a Bitmap that an Image / the compositor still references crashes the render
/// pass (Bitmap.Size NRE + native AV in the crash log) — the swap-then-dispose patterns
/// around the app all funnel through here now.
/// </summary>
public static class DeferredDispose
{
    public static void Later(IDisposable? d, int ms = 3000)
    {
        if (d == null) return;
        void Arm() => DispatcherTimer.RunOnce(() => { try { d.Dispose(); } catch { } }, TimeSpan.FromMilliseconds(ms));
        if (Dispatcher.UIThread.CheckAccess()) Arm();
        else Dispatcher.UIThread.Post(Arm);
    }
}
