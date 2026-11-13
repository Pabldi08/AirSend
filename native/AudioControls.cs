using System;
using System.Collections.Generic;
using System.Linq;
using System.Text.Json;
using System.Threading.Tasks;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;

namespace AirSend;

public sealed partial class MainWindow
{
    private sealed record AudioOutput(string? Id, string Name);
    private sealed record AudioOptions(string? device_id, bool mute_local, bool legacy_timing, string? application_path);
    private sealed record StandbyOptions(bool enabled, int minutes, int threshold_dbfs);

    private AudioOptions audioOptions = new(null, false, false, null);
    private StandbyOptions standbyOptions = new(false, 5, -65);
    private string? pendingResume;
    private Dictionary<string, (string Ip, int Port)>? standbyRoutes;
    private uint latency = 3000;
    private long latencyAvailableAt;
    private long refreshVolumesAt;
    private bool applyingSettings;
    private bool synchronized;
    private bool cancelRequested;
    private bool cancellable;
    private string? receiverVolumeSignature;

    private void LocalizeAudio()
    {
        CaptureSource.Text = L("Fuente de audio de Windows", "Windows audio source");
        LocalizeAppAudio();
        RefreshAudio.Content = L("Actualizar", "Refresh");
        LegacyTimingLabel.Text = L("Usar temporización de audio antigua", "Use legacy audio timing");
        LegacyTimingDescription.Text = L("Pruébala si un altavoz no conecta. Solo afecta a la reproducción individual.",
            "Try this if a speaker cannot connect. Applies to single-speaker playback.");
        MuteLocalLabel.Text = L("Silenciar salida local", "Mute local output");
        MuteLocalDescription.Text = L("Comprueba que la captura conserva la señal. Restaura el sonido al parar o si se pierde la señal.",
            "Checks that capture keeps its signal. Restores local sound when playback stops or the signal is lost.");
        BufferTitle.Text = L("Búfer de reproducción", "Playback buffering");
        ReducedBufferLabel.Text = L("Búfer local reducido", "Reduced local buffering");
        ReducedBufferDescription.Text = L("Puede reducir el retardo y aumentar los cortes. Detén la reproducción para cambiarlo.",
            "May reduce delay and increase dropouts. Stop playback before changing it.");
        GroupsTitle.Text = L("Parejas y sincronización", "Pairs and synchronization");
        StereoPairsLabel.Text = L("Mostrar parejas configuradas en Apple Home", "Show pairs configured in Apple Home");
        GroupHint.Text = L("Selecciona dos HomePods. Si falla un miembro, se detiene todo el grupo.",
            "Select two HomePods. If one member fails, the whole group stops.");
        GroupPlay.Content = L("Probar reproducción sincronizada", "Test synchronized playback");
        StandbyLabel.Text = L("Liberar el altavoz durante el silencio", "Release the speaker when silent");
        StandbyMinutes.Header = L("Tiempo de silencio (minutos)", "Silence duration (minutes)");
        StandbyThreshold.Header = L("Sensibilidad al silencio (dB)", "Silence sensitivity (dB)");
        Resume.Content = L("Reanudar", "Resume");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(StandbyEnabled, StandbyLabel.Text);
        CancelOperation.Content = L("Cancelar operación", "Cancel operation");
        SetMuteStatus("off");
    }

    private void UpdateAudioControls()
    {
        var idle = !busy && active.Count == 0 && !recovering && pendingResume is null;
        AudioSource.IsEnabled = MuteLocal.IsEnabled = idle && audioOptions.application_path is null;
        LegacyTiming.IsEnabled = ReducedBuffer.IsEnabled = idle;
        AudioAppSource.IsEnabled = idle && (SupportsAppAudio || audioOptions.application_path is not null);
        RefreshAudio.IsEnabled = !busy;
        StereoPairs.IsEnabled = GroupFirst.IsEnabled = GroupSecond.IsEnabled = idle;
        GroupPlay.IsEnabled = idle && GroupChoice(GroupFirst) is Receiver a && GroupChoice(GroupSecond) is Receiver b && a.Id != b.Id;
        Multiple.IsEnabled = !busy && !synchronized && !recovering;
        StandbyEnabled.IsEnabled = idle;
        StandbyMinutes.IsEnabled = StandbyThreshold.IsEnabled = idle && StandbyEnabled.IsOn;
        CancelOperation.Visibility = cancellable ? Visibility.Visible : Visibility.Collapsed;
        CancelOperation.IsEnabled = !cancelRequested;
    }

    private async Task LoadSettings()
    {
        applyingSettings = true;
        try
        {
            await LoadPreferences();
            Localize();
            Multiple.IsOn = (await NativeEngine.Invoke(new { command = "get_multi_device" })).GetBoolean();
            ReducedBuffer.IsOn = (await NativeEngine.Invoke(new { command = "get_experimental_local_buffer" })).GetBoolean();
            var volume = await NativeEngine.Invoke(new { command = "get_volume" });
            Volume.Value = volume.ValueKind == JsonValueKind.Number ? volume.GetDouble() * 100 : 20;
            var savedLatency = await NativeEngine.Invoke(new { command = "get_latency" });
            latency = savedLatency.ValueKind == JsonValueKind.Number ? savedLatency.GetUInt32() : 3000;
            SyncBuffer();
            latencyAvailableAt = Environment.TickCount64 + (long)(await NativeEngine.Invoke(new { command = "get_latency_cooldown_ms" })).GetUInt64();
            var value = await NativeEngine.Invoke(new { command = "get_audio_options" });
            audioOptions = new AudioOptions(value.GetProperty("device_id").GetString(),
                value.GetProperty("mute_local").GetBoolean(), value.GetProperty("legacy_timing").GetBoolean(),
                value.TryGetProperty("application_path", out var appPath) ? appPath.GetString() : null);
            LegacyTiming.IsOn = audioOptions.legacy_timing;
            MuteLocal.IsOn = audioOptions.mute_local;
            var standby = await NativeEngine.Invoke(new { command = "get_standby_options" });
            standbyOptions = new StandbyOptions(standby.GetProperty("enabled").GetBoolean(),
                standby.GetProperty("minutes").GetInt32(), standby.GetProperty("threshold_dbfs").GetInt32());
            ShowStandbyOptions();
            try { await ReadAudioOutputs(); }
            catch (InvalidOperationException error)
            {
                MuteStatus.Text = error.Message;
                MuteStatus.Visibility = Visibility.Visible;
            }
            await ReadAudioApps();
        }
        finally { applyingSettings = false; }
    }

    private async Task ReadAudioOutputs()
    {
        var value = await NativeEngine.Invoke(new { command = "list_audio_outputs" });
        var outputs = new List<AudioOutput> { new(null, L("Salida predeterminada", "Default output")) };
        foreach (var item in value.EnumerateArray())
        {
            var name = item.GetProperty("name").GetString()!;
            if (item.GetProperty("is_default").GetBoolean()) name += L(" (predeterminada)", " (default)");
            outputs.Add(new AudioOutput(item.GetProperty("id").GetString(), name));
        }
        if (audioOptions.device_id is not null && outputs.All(o => o.Id != audioOptions.device_id))
            outputs.Add(new AudioOutput(audioOptions.device_id, L("Salida no disponible", "Unavailable output")));
        var wasApplying = applyingSettings;
        applyingSettings = true;
        try
        {
            AudioSource.Items.Clear();
            foreach (var output in outputs)
            {
                var item = new ComboBoxItem { Content = output.Name, Tag = output };
                AudioSource.Items.Add(item);
                if (output.Id == audioOptions.device_id) AudioSource.SelectedItem = item;
            }
        }
        finally { applyingSettings = wasApplying; }
    }

    private async void OnRefreshAudio(object sender, RoutedEventArgs args) => await Run(async () =>
    {
        await ReadAudioApps();
        await ReadAudioOutputs();
    });

    private async void OnAudioSource(object sender, SelectionChangedEventArgs args)
    {
        if (initialized && !applyingSettings && AudioSource.SelectedItem is ComboBoxItem { Tag: AudioOutput source } && source.Id != audioOptions.device_id)
            await Run(SaveAudioOptions);
    }

    private async void OnAudioOption(object sender, RoutedEventArgs args)
    {
        if (!initialized || applyingSettings || busy) return;
        await Run(SaveAudioOptions);
    }

    private async Task SaveAudioOptions()
    {
        var options = new AudioOptions(((AudioSource.SelectedItem as ComboBoxItem)?.Tag as AudioOutput)?.Id,
            MuteLocal.IsOn == true, LegacyTiming.IsOn == true, audioOptions.application_path);
        try
        {
            await NativeEngine.Invoke(new { command = "save_audio_options", options });
            audioOptions = options;
            SetMuteStatus("off");
        }
        catch
        {
            MuteLocal.IsOn = audioOptions.mute_local;
            LegacyTiming.IsOn = audioOptions.legacy_timing;
            await ReadAudioOutputs();
            throw;
        }
    }

    private async void OnReducedBuffer(object sender, RoutedEventArgs args)
    {
        if (!initialized || applyingSettings || busy) return;
        await Run(async () =>
        {
            var enabled = ReducedBuffer.IsOn == true;
            try { await NativeEngine.Invoke(new { command = "save_experimental_local_buffer", enabled }); }
            catch { ReducedBuffer.IsOn = !enabled; throw; }
        });
    }

    private Task<JsonElement> GetPreferences(Receiver receiver) =>
        NativeEngine.Invoke(new { command = "get_receiver_preferences", receiver_id = receiver.Id });

    private Task<JsonElement> PatchPreferences(Receiver receiver, object patch) =>
        NativeEngine.Invoke(new { command = "patch_receiver_preferences", receiver_id = receiver.Id, patch });

    private async Task LoadReceiverSettings(Receiver receiver, bool loadLatency)
    {
        var preferences = await GetPreferences(receiver);
        applyingSettings = true;
        try
        {
            Volume.Value = preferences.GetProperty("volume").GetDouble() * 100;
            if (loadLatency)
            {
                latency = preferences.GetProperty("latency_ms").GetUInt32();
                SyncBuffer();
            }
        }
        finally { applyingSettings = false; }
    }

    private void ShowStandbyOptions()
    {
        var wasApplying = applyingSettings;
        applyingSettings = true;
        StandbyEnabled.IsOn = standbyOptions.enabled;
        StandbyMinutes.Value = standbyOptions.minutes;
        StandbyThreshold.Value = standbyOptions.threshold_dbfs;
        applyingSettings = wasApplying;
    }

    private async void OnStandbyOption(object sender, RoutedEventArgs args) => await SaveStandbyOptions();

    private async void OnStandbyNumber(NumberBox sender, NumberBoxValueChangedEventArgs args) => await SaveStandbyOptions();

    private async Task SaveStandbyOptions()
    {
        if (!initialized || applyingSettings || busy) return;
        await Run(async () =>
        {
            var options = new StandbyOptions(StandbyEnabled.IsOn,
                double.IsFinite(StandbyMinutes.Value) ? (int)Math.Round(StandbyMinutes.Value) : standbyOptions.minutes,
                double.IsFinite(StandbyThreshold.Value) ? (int)Math.Round(StandbyThreshold.Value) : standbyOptions.threshold_dbfs);
            try
            {
                await NativeEngine.Invoke(new { command = "save_standby_options", options });
                standbyOptions = options;
            }
            finally { ShowStandbyOptions(); }
        });
    }

    private void OnStereoPairs(object sender, RoutedEventArgs args)
    {
        if (initialized && !applyingSettings && !busy) RenderReceivers();
    }

    private static Receiver? GroupChoice(ComboBox box) => (box.SelectedItem as ComboBoxItem)?.Tag as Receiver;

    private void UpdateGroupChoices(Receiver[] choices)
    {
        var first = GroupChoice(GroupFirst)?.Id;
        var second = GroupChoice(GroupSecond)?.Id;
        FillGroupChoices(GroupFirst, choices, choices.FirstOrDefault(r => r.Id == first) ?? choices.FirstOrDefault());
        var firstId = GroupChoice(GroupFirst)?.Id;
        FillGroupChoices(GroupSecond, choices, choices.FirstOrDefault(r => r.Id == second && r.Id != firstId)
            ?? choices.FirstOrDefault(r => r.Id != firstId));
    }

    private static void FillGroupChoices(ComboBox box, Receiver[] choices, Receiver? selected)
    {
        box.Items.Clear();
        foreach (var receiver in choices)
        {
            var item = new ComboBoxItem { Content = receiver.Name, Tag = receiver };
            box.Items.Add(item);
            if (receiver.Id == selected?.Id) box.SelectedItem = item;
        }
    }

    private void OnGroupChoice(object sender, SelectionChangedEventArgs args)
    {
        if (initialized) UpdateAudioControls();
    }

    private static Device? GroupRoute(Receiver receiver, Receiver? pair) =>
        receiver.Routes.FirstOrDefault(route => route.Available && route.Addresses.Length != 0
            && (pair is null || pair.Id == "stereo:" + route.Pair));

    private static bool IsCompletePair(Receiver receiver) => receiver.Members is { Length: 2 } members
        && members.All(member => GroupRoute(member, receiver) is not null);

    private async void OnGroupPlay(object sender, RoutedEventArgs args) => await Run(async () =>
    {
        if (GroupChoice(GroupFirst) is Receiver first && GroupChoice(GroupSecond) is Receiver second && first.Id != second.Id)
            await StartGroup(new[] { first, second }, null);
    });

    private async Task StartGroup(Receiver[] members, Receiver? pair)
    {
        if (pair is not null && !IsCompletePair(pair))
            throw new InvalidOperationException(L("Pareja estéreo incompleta.", "Incomplete stereo pair."));
        var routes = members.Select(member => GroupRoute(member, pair)
            ?? throw new InvalidOperationException(L("Receptor no disponible.", "Receiver unavailable."))).ToArray();
        await WithCancellation(() => NativeEngine.Invoke(new
        {
            command = "start_group_streaming",
            device_ids = routes.Select(route => route.Id).ToArray(),
            stereo_pair = pair is not null,
            latency_ms = latency
        }));
        if (cancelRequested) return;
        active.Clear();
        for (var index = 0; index < members.Length; index++)
        {
            var route = routes[index];
            var address = route.Addresses.FirstOrDefault(a => !a.Contains(':')) ?? route.Addresses.First();
            active[members[index].Id] = (address, route.Port);
        }
        output = pair ?? members[0];
        connectedRoute = active[members[0].Id];
        synchronized = true;
        await LoadReceiverSettings(output, false);
        Status.Text = "";
    }

    private async Task<T> WithCancellation<T>(Func<Task<T>> operation)
    {
        cancelRequested = false;
        cancellable = true;
        UpdateAudioControls();
        try { return await operation(); }
        finally
        {
            cancellable = false;
            UpdateAudioControls();
        }
    }

    private async void OnCancelOperation(object sender, RoutedEventArgs args)
    {
        cancelRequested = true;
        UpdateAudioControls();
        try { await Stop(); }
        catch (Exception error) { Status.Text = error.Message; }
    }

    private void SetMuteStatus(string? status)
    {
        MuteStatus.Text = status switch
        {
            "waiting_signal" => L("Esperando audio para probar el silencio.", "Waiting for audio to test mute."),
            "checking" => L("Comprobando captura con salida silenciada…", "Checking capture while muted…"),
            "unsupported" => L("La salida se ha restaurado porque se perdió la señal.", "Local output restored because capture lost signal."),
            "user_changed" => L("Se respeta el cambio manual de volumen o silencio.", "Your manual volume or mute change is respected."),
            _ => ""
        };
        MuteStatus.Visibility = MuteStatus.Text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
    }

    private async Task ReadSession()
    {
        var status = await NativeEngine.Invoke(new { command = "get_session_status" });
        if (busy || quitting) return;
        SetMuteStatus(status.GetProperty("mute_status").GetString());
        SetAppCaptureWaiting(status.TryGetProperty("source_waiting", out var waiting) && waiting.GetBoolean());
        var failure = await NativeEngine.Invoke(new { command = "take_error" });
        if (failure.ValueKind == JsonValueKind.Object &&
            failure.GetProperty("generation").GetUInt64() == status.GetProperty("generation").GetUInt64())
        {
            active.Clear();
            synchronized = recovering = false;
            ReceiverVolumes.Children.Clear();
            receiverVolumeSignature = null;
            Status.Text = L("Se interrumpió la reproducción: ", "Playback was interrupted: ") + failure.GetProperty("code").GetString();
            return;
        }
        var phase = status.GetProperty("status").GetString();
        if (phase is "standby" or "resume_required")
        {
            if (pendingResume is null)
            {
                pendingResume = phase;
                standbyRoutes = new(active);
                active.Clear();
                ReceiverVolumes.Children.Clear();
                receiverVolumeSignature = null;
                Status.Text = phase == "standby"
                    ? L("AirSend ha liberado el altavoz. Pulsa Reanudar para volver a emitir.", "AirSend released the speaker. Select Resume to start sending audio.")
                    : L("Se interrumpió la reproducción. Pulsa Reanudar para volver a emitir.", "Playback was interrupted. Select Resume to start sending audio.");
            }
            recovering = false;
            return;
        }
        recovering = phase == "recovering";
        if (recovering)
        {
            recoveringStatusShown = true;
            return;
        }
        if (status.GetProperty("status").GetString() == "streaming")
        {
            if (recoveringStatusShown) { Status.Text = ""; recoveringStatusShown = false; }
            if (AppWindow.IsVisible && Environment.TickCount64 >= refreshVolumesAt)
            {
                refreshVolumesAt = Environment.TickCount64 + 3000;
                await ReadReceiverVolumes();
            }
        }
    }

    private async Task ReadReceiverVolumes()
    {
        var value = await NativeEngine.Invoke(new { command = "get_receiver_volumes" });
        var signature = value.GetRawText();
        if (busy || active.Count == 0 || signature == receiverVolumeSignature) return;
        if (FocusManager.GetFocusedElement(Root.XamlRoot) is Slider focused && ReceiverVolumes.Children.Contains(focused)) return;
        receiverVolumeSignature = signature;
        ReceiverVolumes.Children.Clear();
        if (value.GetArrayLength() <= 1) return;
        foreach (var item in value.EnumerateArray())
        {
            var ip = item.GetProperty("ip").GetString()!;
            var port = item.GetProperty("port").GetInt32();
            var label = discovered.FirstOrDefault(route => route.Port == port && route.Addresses.Contains(ip))?.Label
                ?? $"{ip}:{port}";
            var readable = item.GetProperty("readable").GetBoolean();
            var slider = new Slider
            {
                Header = readable ? label : $"{label} · {L("valor estimado", "estimated value")}",
                Minimum = 0, Maximum = 100, StepFrequency = 1,
                Value = item.GetProperty("volume").GetDouble() * 100,
            };
            var timer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(180) };
            timer.Tick += async (_, _) =>
            {
                timer.Stop();
                if (active.Count == 0 || quitting) return;
                try
                {
                    await NativeEngine.Invoke(new { command = "set_receiver_volume", ip, port, volume = slider.Value / 100 });
                }
                catch (Exception error) { Status.Text = error.Message; }
            };
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(slider, L("Volumen de ", "Volume for ") + label);
            slider.ValueChanged += (_, _) => { timer.Stop(); timer.Start(); };
            ReceiverVolumes.Children.Add(slider);
        }
    }
}
