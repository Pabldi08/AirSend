using System;
using System.Linq;
using System.Net;
using System.Text.Json;
using System.Threading.Tasks;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.ApplicationModel.DataTransfer;
using Windows.System;
using Automation = Microsoft.UI.Xaml.Automation.AutomationProperties;

namespace AirSend;

public sealed partial class MainWindow
{
    private bool syncingBuffer;
    private bool updatingBuffer;
    private bool showingBufferWait;
    private bool recoveringStatusShown;
    private double previousVolume = 20;
    private ToastService toast = null!;
    private string? copiedReceiverId;
    private const int ConnectionWarningThreshold = 1;
    private long nextDiagnostics;

    private void InitializeExperience()
    {
        toast = new ToastService(ToastHost, ToastMessage, ToastAction);
        Root.PreviewKeyDown += OnPlaybackKey;
    }

    private void LocalizeExperience()
    {
        AdvancedAudio.Header = L("Avanzado", "Advanced");
        EditingInfo.Message = L("Detén la reproducción para cambiar estos ajustes.", "Stop playback to change these settings.");
        StopAndEdit.Content = L("Parar y editar", "Stop and edit");
        BufferHint.Text = L("Un búfer mayor ayuda a evitar cortes, pero aumenta el retraso.",
            "A larger buffer helps prevent dropouts, but adds delay.");
        BufferTitle.Text = L("Retraso de reproducción", "Playback delay");
        MultipleLabel.Text = L("Reproducir en varios dispositivos", "Play on multiple devices");
        MultipleDescription.Text = L("Añade otro altavoz mientras escuchas.", "Add another speaker while listening.");
        StereoPairsDescription.Text = L("Agrupa los altavoces que ya emparejaste en la app Casa.", "Show speakers paired in the Home app together.");
        LegacyTimingDescription.Text = L("Pruébala si un altavoz no conecta.", "Try this if a speaker cannot connect.");
        MuteLocalLabel.Text = L("Silenciar salida local", "Mute local output");
        MuteLocalDescription.Text = L("Deja de oír el audio en el PC mientras se envía al altavoz.", "Mute PC audio while sending it to the speaker.");
        ReducedBufferLabel.Text = L("Búfer local reducido", "Reduced local buffer");
        ReducedBufferDescription.Text = L("Reduce la espera en el PC, con más riesgo de cortes.", "Reduces delay on the PC, with a higher chance of dropouts.");
        foreach (var (control, label) in new[] { (Multiple, MultipleLabel), (StereoPairs, StereoPairsLabel),
                     (LegacyTiming, LegacyTimingLabel), (MuteLocal, MuteLocalLabel),
                     (ReducedBuffer, ReducedBufferLabel) })
            Automation.SetName(control, label.Text);

        Automation.SetName(Buffer, L("Retraso de reproducción", "Playback delay"));
        Automation.SetName(Volume, L("Volumen general", "Overall volume"));
        ToolTipService.SetToolTip(LegacyTiming, L("Cambia cómo AirSend sincroniza el audio con el altavoz.", "Changes how AirSend synchronizes audio with the speaker."));
        ToolTipService.SetToolTip(Buffer, BufferHint.Text);
        DiagnosticPreview.Header = L("Qué incluye el informe", "What the report contains");
        PreviewText.Text = L("Versión de AirSend, formato de audio, búfer solicitado, tiempos locales de envío y recuentos de errores. El informe omite nombres, direcciones y credenciales.",
            "AirSend version, audio format, requested buffer, local send timings and error counts. The report omits names, addresses and credentials.");
        GroupEditingInfo.Message = EditingInfo.Message;
        StopAndEditGroups.Content = StopAndEdit.Content;
        OpenDiagnostics.Content = L("Ver diagnóstico", "View diagnostics");
        RequestedBufferLabel.Text = L("Búfer solicitado", "Requested buffer");
        EncoderBufferLabel.Text = L("Búfer del codificador", "Encoder buffer");
        SendDelayLabel.Text = L("Retardo de envío habitual", "Typical send delay");
        SendIssuesLabel.Text = L("Problemas de envío", "Send issues");
        ToolTipService.SetToolTip(EncoderBufferLabel, L("Audio que espera en el PC antes de ser enviado al altavoz.",
            "Audio waiting on the PC before it is sent to the speaker."));
        ToolTipService.SetToolTip(SendDelayLabel, L("El 95 % de los envíos locales tarda este tiempo o menos. El retraso audible puede ser distinto.",
            "95% of local sends take this long or less. Audible delay can differ."));
        ToolTipService.SetToolTip(ConnectionHealth, L("Esta sesión registró bloques descartados, fallos de envío o falta de audio a tiempo.",
            "This session recorded dropped blocks, send failures or audio arriving too late."));
        nextDiagnostics = 0;
    }

    private void RefreshExperience()
    {
        var playing = active.Count > 0 || recovering;
        EditingInfo.IsOpen = playing;
        GroupEditingInfo.IsOpen = playing;
        StopAndEdit.IsEnabled = StopAndEditGroups.IsEnabled = !busy;
        Buffer.IsEnabled = !busy;
        UpdateBufferAction();
        PlaybackChip.Visibility = playing || pendingResume is not null ? Visibility.Visible : Visibility.Collapsed;
        PlaybackState.Text = pendingResume is not null ? L("En espera", "On standby") : recovering || updatingBuffer
            ? L("Reconectando…", "Reconnecting…")
            : L("Reproduciendo", "Playing");
        if (!playing || recovering) ConnectionWarning.Visibility = Visibility.Collapsed;
        var muted = Volume.Value == 0;
        VolumeIcon.Visibility = muted ? Visibility.Collapsed : Visibility.Visible;
        MutedIcon.Visibility = muted ? Visibility.Visible : Visibility.Collapsed;
        Volume.Opacity = muted ? .5 : 1;
        var muteName = Volume.Value == 0 ? L("Restaurar volumen", "Restore volume") : L("Silenciar altavoz", "Mute speaker");
        Automation.SetName(MuteReceiver, muteName);
        ToolTipService.SetToolTip(MuteReceiver, muteName);
        MuteReceiver.IsEnabled = !busy && output is not null;
        GroupHint.Visibility = playing ? Visibility.Collapsed : Visibility.Visible;
        GroupHint.Text = GroupFirst.Items.Count < 2 ? L("Necesitas dos HomePods disponibles.", "Two available HomePods are required.")
            : L("Elige dos HomePods distintos. Si uno falla, se detiene el grupo.", "Choose two different HomePods. If one fails, the group stops.");
        EndpointError.Text = string.IsNullOrWhiteSpace(Endpoint.Text) || ValidEndpoint(Endpoint.Text) ? ""
            : L("Introduce una IP válida y, si añades un puerto, usa un número del 1 al 65535.", "Enter a valid IP address and, if included, a port from 1 to 65535.");
        EndpointError.Visibility = EndpointError.Text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
    }

    private void SyncBuffer()
    {
        syncingBuffer = true;
        try
        {
            Buffer.Value = latency;
            UpdateBufferAction();
        }
        finally { syncingBuffer = false; }
    }

    private void UpdateBufferAction()
    {
        var applied = Buffer.Value == latency;
        var remaining = latencyAvailableAt - Environment.TickCount64;
        Buffer.Header = $"{L("Límite de búfer solicitado", "Requested buffer limit")} · {Buffer.Value:0} ms";
        ApplyBuffer.Content = applied ? L("Aplicado", "Applied") : L("Aplicar", "Apply");
        ApplyBuffer.IsEnabled = !busy && pendingResume is null && !updatingBuffer && !applied && remaining <= 0;
        if (!busy && !updatingBuffer && showingBufferWait)
        {
            if (!applied && remaining > 0) ShowBufferWait(remaining);
            else
            {
                showingBufferWait = false;
                BufferStatus.Text = "";
            }
        }
    }

    private async void OnApplyBuffer(object sender, RoutedEventArgs args)
    {
        if (!ApplyBuffer.IsEnabled) return;
        var requested = (uint)Buffer.Value;
        showingBufferWait = false;

        var playing = active.Count != 0 || recovering;
        updatingBuffer = true;
        BufferProgress.IsActive = true;
        BufferProgress.Visibility = Visibility.Visible;
        BufferStatus.Text = playing ? L("Reconectando…", "Reconnecting…") : L("Guardando…", "Saving…");
        try
        {
            await Run(async () =>
            {
                try
                {
                    await NativeEngine.Invoke(new { command = "confirm_latency", latency_ms = requested });
                    latency = requested;
                    latencyAvailableAt = Environment.TickCount64 + 10000;
                    if (output is not null) await PatchPreferences(output, new { latency_ms = requested });
                    BufferStatus.Text = L($"Aplicado: {requested} ms.", $"Applied: {requested} ms.");
                }
                catch (InvalidOperationException error) when (error.Message.StartsWith("latency_cooldown:", StringComparison.Ordinal))
                {
                    var remaining = (long)(await NativeEngine.Invoke(new { command = "get_latency_cooldown_ms" })).GetUInt64();
                    latencyAvailableAt = Environment.TickCount64 + remaining;
                    ShowBufferWait(remaining);
                }
                catch
                {
                    BufferStatus.Text = L("No se pudo aplicar el retraso.", "Could not apply the delay.");
                    var status = await NativeEngine.Invoke(new { command = "get_session_status" });
                    if (status.GetProperty("status").GetString() != "streaming") active.Clear();
                    throw;
                }
            });
        }
        finally
        {
            updatingBuffer = false;
            BufferProgress.IsActive = false;
            BufferProgress.Visibility = Visibility.Collapsed;
            RefreshExperience();
        }
    }

    private void ShowBufferWait(long milliseconds)
    {
        showingBufferWait = true;
        var seconds = Math.Max(1, Math.Ceiling(milliseconds / 1000d));
        BufferStatus.Text = L($"Espera {seconds:0} s para aplicar otro retraso.",
            $"Wait {seconds:0} s before applying another delay.");
    }

    private async void OnStopAndEdit(object sender, RoutedEventArgs args) => await Run(Stop);

    private void OnMuteReceiver(object sender, RoutedEventArgs args)
    {
        if (Volume.Value == 0) Volume.Value = previousVolume;
        else { previousVolume = Volume.Value; Volume.Value = 0; }
    }

    private void OnPlaybackKey(object sender, KeyRoutedEventArgs args)
    {
        if (args.Key != VirtualKey.Space || args.KeyStatus.WasKeyDown || !Play.IsEnabled) return;
        if (FocusManager.GetFocusedElement(Root.XamlRoot) is not (null or Grid or TextBlock or StackPanel or ScrollViewer)) return;
        args.Handled = true;
        OnPlay(sender, args);
    }

    private static bool ValidIp(string value)
    {
        if (value.Contains('%') || value.Contains('[') || value.Contains(']')) return false;
        if (value.Contains(':')) return IPAddress.TryParse(value, out _);
        var octets = value.Split('.');
        return octets.Length == 4 && octets.All(o => o.Length is >= 1 and <= 3
            && (o.Length == 1 || o[0] != '0') && o.All(char.IsAsciiDigit) && byte.TryParse(o, out _));
    }

    private static bool ValidEndpoint(string input)
    {
        var value = input.Trim();
        if (ValidIp(value)) return true;
        var separator = value.LastIndexOf(':');
        if (separator < 1) return false;
        var address = value[..separator];
        var port = value[(separator + 1)..];
        if (port.Length == 0 || !port.All(char.IsAsciiDigit) || !ushort.TryParse(port, out var parsed) || parsed == 0) return false;
        if (address.StartsWith('[') && address.EndsWith(']'))
            return address.Contains(':') && ValidIp(address[1..^1]);
        return !address.Contains(':') && ValidIp(address);
    }

    private void OnEndpointChanged(object sender, TextChangedEventArgs args)
    {
        if (initialized) UpdateControls();
    }

    private void CopyAddress(Receiver receiver, Button button)
    {
        var data = new DataPackage();
        data.SetText(receiver.Address());
        Clipboard.SetContent(data);
        toast.Show(L("Dirección copiada", "Address copied"), TimeSpan.FromSeconds(2), dismiss: () =>
        {
            copiedReceiverId = null;
            // Discovery can rebuild the cards while the toast is open.
            foreach (Border card in Receivers.Children)
                if (card.Child is Grid row && row.Children[2] is Button copy
                    && copy.Content is SymbolIcon icon && icon.Symbol == Symbol.Accept)
                    icon.Symbol = Symbol.Copy;
        });
        copiedReceiverId = receiver.Id;
        if (button.Content is SymbolIcon symbol) symbol.Symbol = Symbol.Accept;
    }

    private void OnOpenDiagnostics(object sender, RoutedEventArgs args) => Navigation.SelectedItem = DiagnosticsNav;

    private async Task RefreshDiagnostics()
    {
        var showing = DiagnosticsView.Visibility == Visibility.Visible;
        if ((!showing && active.Count == 0) || Environment.TickCount64 < nextDiagnostics) return;
        nextDiagnostics = Environment.TickCount64 + 3000;
        var data = await NativeEngine.Invoke(new { command = "export_audio_diagnostics" });
        using var document = JsonDocument.Parse(data.GetString()!);
        var sessions = document.RootElement.GetProperty("sessions").EnumerateArray().ToArray();
        if (sessions.Length == 0)
        {
            TransportDetails.Text = "";
            RequestedBufferValue.Text = EncoderBufferValue.Text = SendDelayValue.Text = SendIssuesValue.Text = "—";
            RecentErrors.Text = L("Sin errores recientes", "No recent errors");
            ConnectionWarning.Visibility = Visibility.Collapsed;
            return;
        }
        var latest = sessions[^1];
        var outputs = latest.GetProperty("outputs").EnumerateArray().ToArray();
        var delay = outputs.Select(o => o.GetProperty("capture_delivery_to_udp_p95_ms"))
            .Where(v => v.ValueKind == JsonValueKind.Number).Select(v => v.GetDouble()).DefaultIfEmpty(-1).Max();
        var transports = outputs.Where(o => o.GetProperty("transport_scope").GetString() != "group")
            .Concat(outputs.Where(o => o.GetProperty("transport_scope").GetString() == "group").Take(1)).ToArray();
        var sendErrors = transports.Sum(o => o.GetProperty("send_errors").GetInt64());
        var discarded = transports.Sum(o => o.GetProperty("input_queue_drops").GetInt64() + o.GetProperty("stale_drops").GetInt64());
        var underruns = transports.Sum(o => o.GetProperty("encoder_underruns").GetInt64());
        var losses = sendErrors + discarded + underruns;
        var reconnections = document.RootElement.GetProperty("reconnections").GetUInt64();
        TransportDetails.Text = L(
            $"Cola vacía: {underruns} · Bloques descartados: {discarded} · Fallos UDP: {sendErrors}\nReconexiones desde que abriste AirSend: {reconnections}",
            $"Empty buffer: {underruns} · Discarded blocks: {discarded} · UDP errors: {sendErrors}\nReconnections since opening AirSend: {reconnections}");
        var errors = sessions.Reverse().Select(s => s.GetProperty("last_failure_stage").GetString())
            .Where(code => !string.IsNullOrWhiteSpace(code)).Take(3).Select(DescribeSessionError).ToArray();
        var requested = latest.GetProperty("requested_receiver_latency_ms").GetUInt32();
        var encoder = outputs.Select(o => o.GetProperty("encoder_buffer_ms").GetDouble()).DefaultIfEmpty(-1).Max();
        RequestedBufferValue.Text = $"{requested} ms";
        EncoderBufferValue.Text = encoder < 0 ? "—" : $"{encoder:0.#} ms";
        SendDelayValue.Text = delay < 0 ? "—" : $"{delay:0.#} ms";
        SendIssuesValue.Text = outputs.Length == 0 ? "—" : $"{losses}";
        RecentErrors.Text = errors.Length == 0 ? L("Sin errores recientes", "No recent errors")
            : L("Últimos errores", "Recent errors") + "\n• " + string.Join("\n• ", errors);

        var warn = active.Count > 0 && !recovering
            && latest.GetProperty("status").GetString() == "streaming" && losses >= ConnectionWarningThreshold;
        ConnectionWarning.Visibility = warn ? Visibility.Visible : Visibility.Collapsed;
        if (warn)
            ConnectionHealth.Text = losses == 1 ? L("Conexión inestable · 1 incidencia", "Unstable connection · 1 issue")
                : L($"Conexión inestable · {losses} incidencias", $"Unstable connection · {losses} issues");
    }

    private string DescribeSessionError(string? code) => code switch
    {
        "capture_interrupted" => L("Se interrumpió la captura de audio.", "Audio capture was interrupted."),
        "feedback_failed" => L("El altavoz dejó de responder.", "The speaker stopped responding."),
        "transport_failed" => L("Falló el envío de audio.", "Audio delivery failed."),
        _ => L("La sesión de audio se interrumpió.", "The audio session was interrupted.")
    };
}
