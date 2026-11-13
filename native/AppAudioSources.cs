using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.Storage;

namespace AirSend;

public sealed partial class MainWindow
{
    private sealed record AudioApp(string Path, string Name);
    private IReadOnlyList<AudioApp> audioApps = Array.Empty<AudioApp>();
    private bool appScanFailed;
    private bool appCaptureWaiting;
    private static bool SupportsAppAudio => OperatingSystem.IsWindowsVersionAtLeast(10, 0, 22000);

    private void LocalizeAppAudio()
    {
        AudioAppSource.Header = L("Audio que quieres enviar", "Audio to send");
        RenderAudioApps();
        SetAppCaptureWaiting(appCaptureWaiting);
    }

    private async Task ReadAudioApps()
    {
        if (SupportsAppAudio)
        {
            try
            {
                var result = await NativeEngine.Invoke(new { command = "list_audio_apps" });
                audioApps = result.EnumerateArray().Select(item =>
                {
                    var path = item.GetProperty("path").GetString()!;
                    var fallback = item.GetProperty("name").GetString()!;
                    return new AudioApp(path, AppDisplayName(path, fallback));
                }).OrderBy(app => app.Name, StringComparer.CurrentCultureIgnoreCase).ToArray();
                appScanFailed = false;
            }
            catch (InvalidOperationException)
            {
                audioApps = Array.Empty<AudioApp>();
                appScanFailed = true;
            }
        }
        RenderAudioApps();
    }

    private static string AppDisplayName(string path, string fallback)
    {
        try
        {
            var description = FileVersionInfo.GetVersionInfo(path).FileDescription;
            return string.IsNullOrWhiteSpace(description) ? fallback : description;
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or ArgumentException)
        {
            return fallback;
        }
    }

    private void RenderAudioApps()
    {
        var wasApplying = applyingSettings;
        applyingSettings = true;
        try
        {
            AudioAppSource.Items.Clear();
            AddAudioApp(null, L("Todo el sistema", "Entire system"));
            foreach (var app in audioApps) AddAudioApp(app.Path, app.Name);
            if (audioOptions.application_path is { } saved &&
                !audioApps.Any(app => string.Equals(app.Path, saved, StringComparison.OrdinalIgnoreCase)))
            {
                var name = AppDisplayName(saved, Path.GetFileNameWithoutExtension(saved));
                AddAudioApp(saved, name + L(" (sin audio activo)", " (no active audio)"));
            }
        }
        finally { applyingSettings = wasApplying; }

        AppSourceHint.Text = !SupportsAppAudio
            ? L("Para elegir una aplicación necesitas Windows 11. Puedes elegir Todo el sistema.",
                "Windows 11 is required to choose an app. You can choose Entire system.")
            : appScanFailed
                ? L("No se pudieron buscar aplicaciones. Pulsa Actualizar para volver a intentarlo.",
                    "Could not look up apps. Select Refresh to try again.")
                : L("Para que aparezca una aplicación, ponla a sonar y pulsa Actualizar.",
                    "Play audio in an app and select Refresh to list it.");
        SourceHint.Text = audioOptions.application_path is null
            ? L("Elige la salida de Windows que quieres capturar.", "Choose the Windows output to capture.")
            : L("Solo se envía el audio de la aplicación elegida. Silenciar salida local afecta solo a esa aplicación.",
                "Only the selected app's audio is sent. Local mute applies only to that app.");
    }

    private void AddAudioApp(string? path, string name)
    {
        var icon = new FontIcon { Glyph = path is null ? "\uE7F5" : "\uE71D", FontSize = 16 };
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        row.Children.Add(icon);
        row.Children.Add(new TextBlock { Text = name });
        var item = new ComboBoxItem { Content = row, Tag = path };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(item, name);
        AudioAppSource.Items.Add(item);
        if (string.Equals(audioOptions.application_path, path, StringComparison.OrdinalIgnoreCase))
            AudioAppSource.SelectedItem = item;
        if (path is not null) _ = LoadAppIcon(path, row, icon);
    }

    private static async Task LoadAppIcon(string path, StackPanel row, FontIcon fallback)
    {
        try
        {
            var file = await StorageFile.GetFileFromPathAsync(path);
            using var thumbnail = await file.GetThumbnailAsync(Windows.Storage.FileProperties.ThumbnailMode.SingleItem, 32);
            if (thumbnail is null || thumbnail.Size == 0) return;
            var bitmap = new BitmapImage();
            await bitmap.SetSourceAsync(thumbnail);
            row.Children.Remove(fallback);
            row.Children.Insert(0, new Image { Source = bitmap, Width = 18, Height = 18 });
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or ArgumentException or System.Runtime.InteropServices.COMException) { }
    }

    private async void OnAudioAppSource(object sender, SelectionChangedEventArgs args)
    {
        if (!initialized || applyingSettings || busy || AudioAppSource.SelectedItem is not ComboBoxItem item) return;
        var path = item.Tag as string;
        if (string.Equals(audioOptions.application_path, path, StringComparison.OrdinalIgnoreCase)) return;
        await Run(async () =>
        {
            var options = audioOptions with { application_path = path };
            try
            {
                await NativeEngine.Invoke(new { command = "save_audio_options", options });
                audioOptions = options;
                SetAppCaptureWaiting(false);
                SetMuteStatus("off");
            }
            finally { RenderAudioApps(); }
        });
    }

    private void SetAppCaptureWaiting(bool waiting)
    {
        appCaptureWaiting = waiting;
        AppCaptureStatus.Text = waiting
            ? L("Esperando a que vuelva a sonar la aplicación elegida.", "Waiting for the selected app to play again.")
            : "";
        AppCaptureStatus.Visibility = waiting ? Visibility.Visible : Visibility.Collapsed;
    }

    private string? AppAudioError(string code) => code switch
    {
        "selected_audio_app_unavailable" => L("Pon a sonar la aplicación elegida y vuelve a intentarlo.",
            "Play audio in the selected app and try again."),
        "process_loopback_failed" => L("No se pudo capturar esa aplicación. Puedes elegir Todo el sistema en Audio.",
            "Could not capture that app. You can choose Entire system in Audio."),
        _ => null,
    };
}
