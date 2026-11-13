using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.Json;
using System.Threading.Tasks;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace AirSend;

public sealed partial class MainWindow : Window
{
    private sealed record Device(string Id, string Name, string Kind, string Host,
        string? HardwareId, string[] Addresses, int Port, bool AirPlay2, bool Available, string? Pair, string? GroupName, bool Leader)
    {
        internal bool IsRaop => Id.Contains("._raop._tcp.", StringComparison.OrdinalIgnoreCase);
        internal bool IsAirPlay => Id.Contains("._airplay._tcp.", StringComparison.OrdinalIgnoreCase);
        internal string Label => IsRaop && Name.Split('@', 2) is [var prefix, var name]
            && prefix.Length is >= 12 and <= 17 && prefix.All(c => Uri.IsHexDigit(c) || c is ':' or '-')
            ? name : Name;
        internal string Group => IsRaop || IsAirPlay
            ? HardwareId is not null ? $"hardware:{HardwareId}" : $"host:{Host.ToLowerInvariant()}|{Label.ToLowerInvariant()}"
            : Id;
    }

    private sealed record Receiver(string Id, string Name, Device[] Routes, Receiver[]? Members = null)
    {
        public string Address(bool shorten = false)
        {
            var route = Routes[0];
            var address = route.Addresses.FirstOrDefault(a => !a.Contains(':'))
                ?? route.Addresses.FirstOrDefault() ?? route.Host;
            if (!address.Contains(':')) return $"{address}:{route.Port}";
            if (shorten && address.Length > 22) address = $"{address[..12]}…{address[^8..]}";
            return $"[{address}]:{route.Port}";
        }

        public string Description(bool shorten)
        {
            if (Members is not null) return string.Join(" + ", Members.Select(member => member.Name));
            return $"{Routes[0].Kind} · {Address(shorten)}"
                + (Routes.Any(d => d.AirPlay2) ? " · AirPlay 2" : "");
        }
    }

    private readonly DispatcherTimer refresh = new() { Interval = TimeSpan.FromSeconds(1) };
    private readonly DispatcherTimer volumeDelay = new() { Interval = TimeSpan.FromMilliseconds(180) };
    private readonly Dictionary<string, (string Ip, int Port)> active = new();
    private readonly TrayIcon tray;
    private Receiver? output;
    private (string Ip, int Port)? connectedRoute;
    private bool english;
    private bool busy;
    private bool quitting;
    private bool quitRequested;
    private bool recovering;
    private bool refreshing;
    private bool initialized;
    private ulong? deviceRevision;
    private Receiver[] receivers = Array.Empty<Receiver>();
    private Device[] discovered = Array.Empty<Device>();

    public MainWindow()
    {
        InitializeComponent();
        InitializeExperience();
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(WindowTitleBar);
        if (Microsoft.UI.Windowing.AppWindowTitleBar.IsCustomizationSupported())
        {
            AppWindow.TitleBar.ButtonBackgroundColor = Microsoft.UI.Colors.Transparent;
            AppWindow.TitleBar.ButtonInactiveBackgroundColor = Microsoft.UI.Colors.Transparent;
        }
        Root.ActualThemeChanged += (_, _) => UpdateCaptionTheme();
        AppWindow.Resize(new Windows.Graphics.SizeInt32(1120, 790));
        AppWindow.SetIcon(Path.Combine(AppContext.BaseDirectory, "AirSend.ico"));
        tray = new TrayIcon(WinRT.Interop.WindowNative.GetWindowHandle(this),
            Path.Combine(AppContext.BaseDirectory, "AirSend.ico"),
            () => DispatcherQueue.TryEnqueue(ToggleWindow),
            () => DispatcherQueue.TryEnqueue(async () => await Run(Stop)),
            () => DispatcherQueue.TryEnqueue(async () => await Quit()));
        AppWindow.Closing += async (_, args) =>
        {
            if (quitting) return;
            args.Cancel = true;
            if (tray.IsAvailable) HideToTray();
            else await Quit();
        };

        english = System.Globalization.CultureInfo.CurrentUICulture.TwoLetterISOLanguageName != "es";
        SyncBuffer();
        Volume.Value = 20;
        Localize();
        initialized = true;
        refresh.Tick += OnRefresh;
        volumeDelay.Tick += async (_, _) =>
        {
            if (busy) return;
            volumeDelay.Stop();
            await Run(async () =>
            {
                var value = Volume.Value / 100;
                if (active.Count != 0) await NativeEngine.Invoke(new { command = "set_stream_volume", volume = value });
                await NativeEngine.Invoke(new { command = "save_volume", volume = value });
                if (output is not null) await PatchPreferences(output, new { volume = value });
            });
        };
        Root.Loaded += OnLoaded;
    }

    private async void OnLoaded(object sender, RoutedEventArgs args)
    {
        Root.Loaded -= OnLoaded;
        var root = Root.XamlRoot;
        UpdateMinimumWindowSize(root.RasterizationScale);
        root.Changed += (changedRoot, _) => UpdateMinimumWindowSize(changedRoot.RasterizationScale);
        refresh.Start();
        await Run(async () => { await LoadSettings(); await Bootstrap(); });
    }

    private void UpdateMinimumWindowSize(double scale)
    {
        if (AppWindow.Presenter is not Microsoft.UI.Windowing.OverlappedPresenter presenter) return;
        var width = (int)Math.Ceiling(960 * scale);
        var height = (int)Math.Ceiling(620 * scale);
        if (presenter.PreferredMinimumWidth != width) presenter.PreferredMinimumWidth = width;
        if (presenter.PreferredMinimumHeight != height) presenter.PreferredMinimumHeight = height;
    }

    private void ToggleWindow()
    {
        if (AppWindow.Presenter is Microsoft.UI.Windowing.OverlappedPresenter presenter &&
            presenter.State == Microsoft.UI.Windowing.OverlappedPresenterState.Minimized)
        {
            AppWindow.Show();
            presenter.Restore();
            Activate();
            UpdateRefreshTimer();
            return;
        }
        if (AppWindow.IsVisible) HideToTray();
        else
        {
            AppWindow.Show();
            Activate();
            UpdateRefreshTimer();
        }
    }

    private void HideToTray()
    {
        AppWindow.Hide();
        UpdateRefreshTimer();
    }

    private void UpdateRefreshTimer()
    {
        if (!quitting && !refreshing && (AppWindow.IsVisible || active.Count != 0))
            refresh.Start();
        else
            refresh.Stop();
    }

    private async Task Quit()
    {
        if (quitting) return;
        if (busy) { quitRequested = true; return; }
        quitting = true;
        refresh.Stop();
        volumeDelay.Stop();
        try { await NativeEngine.Invoke(new { command = "shutdown" }); }
        catch (Exception error) { System.Diagnostics.Trace.TraceError(error.ToString()); }
        finally
        {
            tray.Dispose();
            Close();
        }
    }

    private string L(string es, string en) => english ? en : es;

    private void Localize()
    {
        SpeakersNav.Content = SpeakersTitle.Text = L("Altavoces", "Speakers");
        AudioNav.Content = AudioTitle.Text = "Audio";
        DiagnosticsNav.Content = DiagnosticsTitle.Text = L("Diagnóstico", "Diagnostics");
        ScanLabel.Text = L("Buscar dispositivos", "Scan devices");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(Scan, ScanLabel.Text);
        MultipleLabel.Text = L("Reproducir en varios dispositivos", "Play on multiple devices");
        ManualTitle.Text = L("Añadir un altavoz manualmente", "Add a speaker manually");
        Endpoint.Header = L("Dirección IP y puerto opcional", "IP address and optional port");
        ReceiverName.Header = L("Nombre (opcional)", "Name (optional)");
        AddManual.Content = L("Añadir", "Add");
        LocalizeAudio();
        LocalizeExperience();
        DiagnosticsDescription.Text = L("Exporta métricas de captura y envío sin nombres ni direcciones de los receptores.",
            "Export capture and forwarding metrics without receiver names or addresses.");
        ExportDiagnostics.Content = L("Exportar diagnóstico", "Export diagnostics");
        CurrentOutput.Text = L("Salida actual", "Current output");
        LocalizePreferences();
        RenderReceiverRows();
        tray.ShowLabel = L("Mostrar / ocultar ventana", "Show / hide window");
        tray.QuitLabel = L("Salir", "Quit");
        tray.StopLabel = L("Parar reproducción", "Stop playback");
        UpdateControls();
    }

    private void UpdateControls()
    {
        OutputName.Text = output?.Name ?? L("Ningún altavoz conectado", "No speaker connected");
        var canStop = active.Count != 0 || recovering || pendingResume is not null;
        tray.CanStop = canStop && !busy;
        Play.Content = canStop ? L("Parar", "Stop") : L("Enviar audio del PC", "Send PC audio");
        Play.Style = (Style)Root.Resources[canStop ? "StopAudioStyle" : "SendAudioStyle"];
        Play.IsEnabled = !busy && (output is not null || canStop);
        Resume.Visibility = pendingResume is not null ? Visibility.Visible : Visibility.Collapsed;
        Resume.IsEnabled = !busy;
        Scan.IsEnabled = !busy;
        AddManual.IsEnabled = !busy && ValidEndpoint(Endpoint.Text);
        UpdateReceiverActions();
        RefreshExperience();
        UpdateAudioControls();
        Volume.Header = $"{L("Volumen general", "Overall volume")} · {Volume.Value:0}%";
    }

    private async Task Run(Func<Task> work)
    {
        if (busy || quitting) return;
        busy = true;
        UpdateControls();
        try { await work(); }
        catch (Exception error) when (cancelRequested && error.Message == "session_cancelled") { }
        catch (InvalidOperationException error) when (error.Message == "receiver_in_use")
        {
            Status.Text = L("El altavoz está en uso por otro dispositivo.", "The speaker is in use by another device.");
        }
        catch (InvalidOperationException error) when (AppAudioError(error.Message) is not null)
        {
            Status.Text = AppAudioError(error.Message);
        }
        catch (InvalidOperationException error) when (AirPlayError(error.Message) is { } message)
        {
            Status.Text = message;
        }
        catch (Exception error) { Status.Text = error.Message; }
        finally
        {
            cancelRequested = false;
            busy = false;
            UpdateControls();
            UpdateRefreshTimer();
            if (quitRequested) await Quit();
        }
    }

    private void OnNavigate(NavigationView _, NavigationViewSelectionChangedEventArgs args)
    {
        if (SpeakersView is null || AudioView is null || DiagnosticsView is null) return;
        var section = (args.SelectedItem as NavigationViewItem)?.Tag as string;
        SpeakersView.Visibility = section == "speakers" ? Visibility.Visible : Visibility.Collapsed;
        AudioView.Visibility = section == "audio" ? Visibility.Visible : Visibility.Collapsed;
        DiagnosticsView.Visibility = section == "diagnostics" ? Visibility.Visible : Visibility.Collapsed;
    }

    private void OnBufferChanged(object sender, Microsoft.UI.Xaml.Controls.Primitives.RangeBaseValueChangedEventArgs args)
    {
        if (!initialized || applyingSettings || syncingBuffer) return;
        BufferStatus.Text = "";
        if (Buffer.Value != latency && Environment.TickCount64 < latencyAvailableAt)
            ShowBufferWait(latencyAvailableAt - Environment.TickCount64);
        UpdateBufferAction();
    }

    private void OnVolumeChanged(object sender, Microsoft.UI.Xaml.Controls.Primitives.RangeBaseValueChangedEventArgs args)
    {
        if (!initialized || applyingSettings) return;
        UpdateControls();
        volumeDelay.Stop();
        volumeDelay.Start();
    }

    private async void OnScan(object sender, RoutedEventArgs args) => await Run(async () =>
    {
        ScanProgress.IsActive = true;
        ScanProgress.Visibility = Visibility.Visible;
        try
        {
            await NativeEngine.Invoke(new { command = "start_discovery_stream" });
            await Task.Delay(1200);
            await ReadDevices();
        }
        finally
        {
            ScanProgress.IsActive = false;
            ScanProgress.Visibility = Visibility.Collapsed;
        }
    });

    private async void OnRefresh(object? sender, object args)
    {
        if (busy || quitting || (!AppWindow.IsVisible && active.Count == 0)) return;
        refresh.Stop();
        refreshing = true;
        try
        {
            if (active.Count != 0 || recovering || pendingResume is not null)
                await ReadSession();
            if (AppWindow.IsVisible) UpdateControls();
            if (!busy && AppWindow.IsVisible)
            {
                await ReadDevices();
                await RefreshDiagnostics();
            }
        }
        catch (Exception error) { Status.Text = error.Message; }
        finally
        {
            refreshing = false;
            UpdateRefreshTimer();
        }
    }

    private async Task<Receiver[]> ReadDevices()
    {
        var result = await NativeEngine.Invoke(new { command = "devices", since = deviceRevision });
        if (result.ValueKind == JsonValueKind.Null || quitting) return receivers;
        var revision = result.GetProperty("revision").GetUInt64();
        if (deviceRevision is ulong current && current >= revision) return receivers;
        deviceRevision = revision;
        discovered = result.GetProperty("devices").EnumerateArray().Select(ReadDevice).ToArray();
        RenderReceivers();
        return receivers;
    }

    private void RenderReceivers()
    {
        var individuals = discovered.GroupBy(d => d.Group)
            .Select(group =>
            {
                var primary = group.FirstOrDefault(d => d.IsAirPlay) ?? group.First();
                return new Receiver(group.Key, primary.Label, group.OrderByDescending(d => d.Available)
                    .ThenByDescending(d => primary.Kind == "homepod" ? d.IsAirPlay : d.IsRaop)
                    .ThenBy(d => d.Id).ToArray());
            }).OrderByDescending(r => r.Routes.Any(d => d.Kind == "homepod")).ThenBy(r => r.Name).ThenBy(r => r.Id).ToArray();
        UpdateGroupChoices(individuals.Where(r => r.Routes.Any(d => d.Kind == "homepod" && d.Available)).ToArray());
        var list = individuals.ToList();
        if (StereoPairs.IsOn == true)
        {
            foreach (var group in individuals.Where(r => r.Routes.Any(d => d.Kind == "homepod" && d.Pair is not null))
                         .GroupBy(r => r.Routes.First(d => d.Kind == "homepod" && d.Pair is not null).Pair!))
            {
                var members = group.OrderByDescending(r => r.Routes.Any(d => d.Leader)).ThenBy(r => r.Id).ToArray();
                var name = members.SelectMany(r => r.Routes).FirstOrDefault(d => d.GroupName is not null)?.GroupName
                    ?? string.Join(" + ", members.Select(r => r.Name));
                list.RemoveAll(r => members.Contains(r));
                list.Add(new Receiver("stereo:" + group.Key, name, members.Select(r => r.Routes[0]).ToArray(), members));
            }
        }
        receivers = list.ToArray();
        RenderReceiverRows();
        UpdateControls();
    }

    private void RenderReceiverRows()
    {
        Receivers.Children.Clear();
        foreach (var receiver in receivers)
        {
            var label = new StackPanel { Spacing = 3, VerticalAlignment = VerticalAlignment.Center };
            label.Children.Add(new TextBlock
            {
                Text = receiver.Name,
                FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                TextTrimming = TextTrimming.CharacterEllipsis
            });
            var details = new TextBlock
            {
                Text = IsAvailable(receiver) ? receiver.Description(true)
                    : $"{receiver.Description(true)} · {L("No disponible", "Unavailable")}",
                FontSize = 12,
                Opacity = .72,
                TextTrimming = TextTrimming.CharacterEllipsis
            };
            ToolTipService.SetToolTip(details, receiver.Description(false));
            label.Children.Add(details);
            var action = new Button { Tag = receiver, VerticalAlignment = VerticalAlignment.Center };
            action.Click += OnConnect;
            var row = new Grid { ColumnSpacing = 12 };
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            Grid.SetColumn(action, 1);
            var copy = new Button
            {
                Content = new SymbolIcon(copiedReceiverId == receiver.Id ? Symbol.Accept : Symbol.Copy), VerticalAlignment = VerticalAlignment.Center
            };
            var copyLabel = L("Copiar dirección de ", "Copy address for ") + receiver.Name;
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(copy, copyLabel);
            ToolTipService.SetToolTip(copy, copyLabel);
            copy.Click += (_, _) => CopyAddress(receiver, copy);
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            Grid.SetColumn(copy, 2);
            row.Children.Add(label);
            row.Children.Add(action);
            row.Children.Add(copy);
            Receivers.Children.Add(new Border { Child = row });
        }
    }

    private static bool IsAvailable(Receiver receiver) =>
        receiver.Members is not null ? IsCompletePair(receiver) : receiver.Routes.Any(route => route.Available);

    private void UpdateReceiverActions()
    {
        foreach (Border card in Receivers.Children)
        {
            var row = (Grid)card.Child;
            var action = (Button)row.Children[1];
            var receiver = (Receiver)action.Tag;
            var connected = receiver.Id == output?.Id || active.ContainsKey(receiver.Id);
            var label = connected ? L("Desconectar", "Disconnect") : L("Conectar", "Connect");
            action.Content = label;
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(action, $"{label} {receiver.Name}");
            action.Style = (Style)Root.Resources[connected ? "ReceiverDisconnectStyle" : "ReceiverConnectStyle"];
            action.IsEnabled = !busy && (connected || (!synchronized && IsAvailable(receiver)));
            card.Style = (Style)Root.Resources[connected ? "ConnectedReceiverCardStyle" : "ReceiverCardStyle"];
        }
    }

    private static Device ReadDevice(JsonElement d) => new(
        d.GetProperty("id").GetString()!, d.GetProperty("name").GetString()!, d.GetProperty("kind").GetString()!,
        d.GetProperty("host").GetString()!, d.GetProperty("hardware_id").GetString(),
        d.GetProperty("addresses").EnumerateArray().Select(a => a.GetString()!).ToArray(),
        d.GetProperty("port").GetInt32(), d.GetProperty("supports_airplay2").GetBoolean(),
        d.GetProperty("available").GetBoolean(), d.GetProperty("tight_sync_id").GetString(),
        d.GetProperty("group_name").GetString(), d.GetProperty("is_group_leader").GetBoolean());

    private async Task Bootstrap()
    {
        await NativeEngine.Invoke(new { command = "start_discovery_stream" });
        var last = await NativeEngine.Invoke(new { command = "get_last_device" });
        if (last.ValueKind != JsonValueKind.Object || last.GetProperty("ip").GetString() is not { Length: > 0 } ip) return;
        var port = last.GetProperty("port").GetInt32();
        var name = last.GetProperty("name").GetString();
        Receiver? receiver = null;
        var deadline = System.Diagnostics.Stopwatch.StartNew();
        do
        {
            receiver = (await ReadDevices()).FirstOrDefault(r => r.Routes.Any(d => d.Addresses.Contains(ip)));
            if (receiver is null) await Task.Delay(250);
        } while (receiver is null && deadline.Elapsed < TimeSpan.FromSeconds(6));
        if (receiver is null)
        {
            var added = ReadDevice(await NativeEngine.Invoke(new { command = "add_manual_device", ip, port, name }));
            receiver = (await ReadDevices()).First(r => r.Id == added.Group);
        }
        output = receiver;
        connectedRoute = (ip, port);
        await LoadReceiverSettings(receiver, true);
    }

    private async void OnAddManual(object sender, RoutedEventArgs args) => await Run(async () =>
    {
        var ip = Endpoint.Text.Trim();
        if (!ValidEndpoint(ip)) return;
        await NativeEngine.Invoke(new { command = "add_manual_device", ip, name = string.IsNullOrWhiteSpace(ReceiverName.Text) ? null : ReceiverName.Text.Trim() });
        Endpoint.Text = ReceiverName.Text = "";
        await ReadDevices();
    });

    private async Task<(string Ip, int Port)> TryRoutes(Receiver receiver, string command, (string Ip, int Port)? preferred)
    {
        var preferences = await GetPreferences(receiver);
        var routes = receiver.Routes.Where(route => route.Available).SelectMany(route =>
            route.Addresses.OrderBy(address => address.Contains(':')).Select(address => (Ip: address, route.Port, route.Id)))
            .Distinct().OrderByDescending(route => (route.Ip, route.Port) == preferred).ToArray();
        Exception? lastError = null;
        foreach (var (ip, port, id) in routes)
        {
            try
            {
                await NativeEngine.Invoke(new { command, ip, port, device_id = id, name = receiver.Name, volume = preferences.GetProperty("volume").GetDouble(), latency_ms = command == "add_streaming" ? latency : preferences.GetProperty("latency_ms").GetUInt32() });
                return (ip, port);
            }
            catch (InvalidOperationException error) when (!cancelRequested)
            {
                lastError = error;
                if (command != "connect_device" && error.Message is not ("stream: timeout" or "stream: transport")) break;
            }
        }
        throw lastError ?? new InvalidOperationException(L("El receptor no tiene dirección IP.", "The receiver has no IP address."));
    }

    private async Task Stream(Receiver receiver, string command, (string Ip, int Port)? preferred = null)
    {
        if (receiver.Members is not null)
        {
            if (command == "add_streaming")
                throw new InvalidOperationException(L("Detén la reproducción para añadir una pareja estéreo.", "Stop playback before adding a stereo pair."));
            await StartGroup(receiver.Members, receiver);
            return;
        }
        var route = await WithCancellation(() => TryRoutes(receiver, command, preferred));
        if (cancelRequested) return;
        if (command == "start_streaming") active.Clear();
        active[receiver.Id] = route;
        if (command == "add_streaming") return;
        output = receiver;
        connectedRoute = route;
        await LoadReceiverSettings(receiver, command == "start_streaming");
        await NativeEngine.Invoke(new { command = "save_last_device", ip = route.Ip, port = route.Port, name = receiver.Name });
        Status.Text = "";
    }

    private async Task Stop()
    {
        await NativeEngine.Invoke(new { command = "stop_streaming" });
        await NativeEngine.Invoke(new { command = "disconnect_device" });
        active.Clear();
        pendingResume = null;
        standbyRoutes = null;
        recovering = synchronized = false;
        ReceiverVolumes.Children.Clear();
        receiverVolumeSignature = null;
        SetMuteStatus("off");
        SetAppCaptureWaiting(false);
        Status.Text = "";
    }

    private async void OnPlay(object sender, RoutedEventArgs args) => await Run(async () =>
    {
        if (active.Count != 0 || recovering || pendingResume is not null) await Stop();
        else if (output is not null) await Stream(output, "start_streaming", connectedRoute);
    });

    private async void OnResume(object sender, RoutedEventArgs args) => await Run(async () =>
    {
        if (pendingResume is null) return;
        try
        {
            await WithCancellation(() => NativeEngine.Invoke(new { command = "resume_streaming" }));
        }
        catch (InvalidOperationException error) when (!cancelRequested && error.Message == "receiver_in_use")
        {
            Status.Text = L("El altavoz está en uso por otro dispositivo.", "The speaker is in use by another device.");
            return;
        }
        catch (InvalidOperationException error) when (!cancelRequested)
        {
            Status.Text = AppAudioError(error.Message) ?? AirPlayError(error.Message) ??
                L("No se pudo reanudar. Comprueba el altavoz y vuelve a intentarlo.", "Could not resume. Check the speaker and try again.");
            return;
        }
        if (cancelRequested) return;
        if (standbyRoutes is not null)
            foreach (var (id, route) in standbyRoutes) active[id] = route;
        pendingResume = null;
        standbyRoutes = null;
        Status.Text = "";
    });

    private string? AirPlayError(string code) => code switch
    {
        "stream: authentication" => L("El altavoz rechazó la conexión. Revisa los permisos de AirPlay en Casa.",
            "The speaker rejected the connection. Check AirPlay permissions in Home."),
        "stream: ptp_negotiation" => L("No se pudo sincronizar el reloj del altavoz.",
            "Could not synchronize the speaker clock."),
        "stream: timeout" => L("El altavoz tardó demasiado en responder.", "The speaker took too long to respond."),
        "stream: transport" => L("Se perdió la conexión con el altavoz.", "The connection to the speaker was lost."),
        _ when code.StartsWith("stream: ", StringComparison.Ordinal) =>
            L("El altavoz no pudo iniciar la reproducción.", "The speaker could not start playback."),
        _ => null,
    };

    private async void OnConnect(object sender, RoutedEventArgs args) => await Run(async () =>
    {
        if (sender is not Button { Tag: Receiver receiver }) return;
        if (pendingResume is not null) await Stop();
        if (synchronized)
        {
            if (receiver.Id == output?.Id || active.ContainsKey(receiver.Id))
            {
                await Stop();
                if (receiver.Id == output?.Id)
                {
                    output = null;
                    connectedRoute = null;
                    await NativeEngine.Invoke(new { command = "clear_last_device" });
                }
            }
            return;
        }
        if (active.Remove(receiver.Id, out var route))
        {
            try { await NativeEngine.Invoke(new { command = "remove_streaming", ip = route.Ip, port = route.Port }); }
            catch { active[receiver.Id] = route; throw; }
            if (output?.Id == receiver.Id)
            {
                var remaining = active.FirstOrDefault();
                output = remaining.Key is null ? null : (await ReadDevices()).FirstOrDefault(r => r.Id == remaining.Key);
                connectedRoute = output is null ? null : remaining.Value;
            }
            if (output is not null) await LoadReceiverSettings(output, false);
            else await NativeEngine.Invoke(new { command = "clear_last_device" });
            if (active.Count == 0) Status.Text = "";
        }
        else if (output?.Id == receiver.Id)
        {
            await NativeEngine.Invoke(new { command = "disconnect_device" });
            output = null;
            connectedRoute = null;
            await NativeEngine.Invoke(new { command = "clear_last_device" });
        }
        else if (active.Count == 0)
        {
            if (receiver.Members is not null && !IsCompletePair(receiver)) throw new InvalidOperationException(L("Pareja estéreo incompleta.", "Incomplete stereo pair."));
            connectedRoute = receiver.Members is null
                ? await WithCancellation(() => TryRoutes(receiver, "connect_device", null)) : null;
            if (cancelRequested) return;
            output = receiver;
            await LoadReceiverSettings(receiver, true);
        }
        else if (Multiple.IsOn == true) await Stream(receiver, "add_streaming");
        else if (await Ask(L($"¿Cambiar la reproducción a {receiver.Name}?", $"Switch playback to {receiver.Name}?")))
            await Stream(receiver, "start_streaming");
    });

    private async Task<bool> Ask(string title) => await new ContentDialog
    {
        XamlRoot = Root.XamlRoot,
        Title = title,
        PrimaryButtonText = L("Confirmar", "Confirm"),
        PrimaryButtonStyle = (Style)Application.Current.Resources["RoundedButtonStyle"],
        CloseButtonText = L("Cancelar", "Cancel"),
        CloseButtonStyle = (Style)Application.Current.Resources["RoundedButtonStyle"]
    }.ShowAsync() == ContentDialogResult.Primary;

    private async void OnMultiple(object sender, RoutedEventArgs args)
    {
        if (!initialized || applyingSettings || busy) return;
        await Run(async () =>
        {
            var enable = Multiple.IsOn == true;
            if (!enable && active.Count > 1)
            {
                if (!await Ask(L("¿Continuar solo en la salida actual?", "Continue on the current output only?")))
                {
                    Multiple.IsOn = true;
                    return;
                }
                try
                {
                    var keep = output?.Id ?? active.Keys.First();
                    foreach (var (id, route) in active.ToArray())
                    {
                        if (id == keep) continue;
                        await NativeEngine.Invoke(new { command = "remove_streaming", ip = route.Ip, port = route.Port });
                        active.Remove(id);
                    }
                }
                catch { Multiple.IsOn = true; throw; }
            }
            await NativeEngine.Invoke(new { command = "save_multi_device", enabled = enable });
        });
    }

    private async void OnExportDiagnostics(object sender, RoutedEventArgs args) => await Run(async () =>
    {
        var report = await NativeEngine.Invoke(new { command = "export_audio_diagnostics" });
        var picker = new Windows.Storage.Pickers.FileSavePicker { SuggestedFileName = "airsend-diagnostics" };
        WinRT.Interop.InitializeWithWindow.Initialize(picker, WinRT.Interop.WindowNative.GetWindowHandle(this));
        picker.FileTypeChoices.Add("JSON", new List<string> { ".json" });
        var file = await picker.PickSaveFileAsync();
        if (file is null) return;
        await Windows.Storage.FileIO.WriteTextAsync(file, report.GetString()!);
        var folder = await file.GetParentAsync();
        toast.Show(L("Guardado en ", "Saved to ") + file.Path, TimeSpan.FromSeconds(8),
            actionLabel: L("Abrir carpeta", "Open folder"),
            invoke: folder is null ? null : async () =>
            {
                try
                {
                    if (!await Windows.System.Launcher.LaunchFolderAsync(folder))
                        toast.Show(L("No se pudo abrir la carpeta.", "Could not open the folder."), TimeSpan.FromSeconds(3));
                }
                catch (Exception)
                {
                    toast.Show(L("No se pudo abrir la carpeta.", "Could not open the folder."), TimeSpan.FromSeconds(3));
                }
            });
    });
}
