using System;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace AirSend;

internal sealed class ToastService
{
    private readonly Border host;
    private readonly TextBlock message;
    private readonly Button action;
    private readonly DispatcherTimer timer = new();
    private Action? onDismiss;
    private Action? onAction;

    internal ToastService(Border host, TextBlock message, Button action)
    {
        this.host = host;
        this.message = message;
        this.action = action;
        timer.Tick += (_, _) => Dismiss();
        action.Click += (_, _) =>
        {
            var callback = onAction;
            Dismiss();
            callback?.Invoke();
        };
    }

    internal void Show(string text, TimeSpan duration, Action? dismiss = null,
        string? actionLabel = null, Action? invoke = null)
    {
        Dismiss();
        onDismiss = dismiss;
        onAction = invoke;
        action.Content = actionLabel;
        action.Visibility = invoke is null ? Visibility.Collapsed : Visibility.Visible;
        host.IsHitTestVisible = invoke is not null;
        host.Visibility = Visibility.Visible;
        message.Text = text;
        timer.Interval = duration;
        timer.Start();
    }

    private void Dismiss()
    {
        timer.Stop();
        host.Visibility = Visibility.Collapsed;
        message.Text = "";
        onAction = null;
        var callback = onDismiss;
        onDismiss = null;
        callback?.Invoke();
    }
}
