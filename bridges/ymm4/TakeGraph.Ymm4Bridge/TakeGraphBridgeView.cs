using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Data;
using System.Windows.Input;
using System.Windows.Threading;

namespace TakeGraph.Ymm4Bridge;

public sealed class TakeGraphBridgeViewModel : INotifyPropertyChanged
{
    private readonly DispatcherTimer timer;
    private readonly AnnotationPanelState panel = new();
    private CaptureHostClient? client;
    private bool refreshing;
    private bool busy;
    private string selectedDeviceId = string.Empty;
    private string selectedHotkey = "F8";
    private string hotkeyDraft = "F8";
    private bool capturingHotkey;
    private AnnotationRowViewModel? selectedAnnotation;
    private string lastError = string.Empty;

    public TakeGraphBridgeViewModel()
    {
        Devices = [];
        Hotkeys = [];
        Annotations = [];
        RecordCommand = new RelayCommand(ToggleRecordingAsync, () => CanRecord);
        JumpCommand = new RelayCommand(JumpAsync, () => CanJump);
        DismissCommand = new RelayCommand(DismissAsync, () => CanDismiss);
        ApplyHotkeyCommand = new RelayCommand(ApplyHotkeyAsync, () => CanApplyHotkey);
        CaptureHotkeyCommand = new RelayCommand(StartHotkeyCaptureAsync, () => CanRecord);
        CopyErrorCommand = new RelayCommand(CopyErrorAsync, () => CanCopyError);
        timer = new DispatcherTimer { Interval = TimeSpan.FromSeconds(1) };
        timer.Tick += async (_, _) => await RefreshAsync().ConfigureAwait(true);
        timer.Start();
        _ = RefreshAsync();
    }

    public string Status => TakeGraphToolPlugin.Current.Status;
    public string CredentialPath => BridgeCredentials.CredentialsPath;
    public string CaptureHostStatus => panel.StatusLabel;
    public string CaptureHostHint => string.IsNullOrWhiteSpace(lastError) ? panel.Hint : lastError;
    public string HotkeyLabel =>
        string.IsNullOrWhiteSpace(selectedHotkey) ? "Hotkey" : $"Hotkey: {selectedHotkey}";
    public string RecordLabel =>
        panel.Connection == CaptureHostConnection.Recording ? "Stop recording" : "Start recording";
    public string RecordingIndicator => panel.Connection == CaptureHostConnection.Recording
        ? $"REC  {AnnotationRowViewModel.FormatFrame(panel.RecordingStartFrame ?? 0, 0)}"
        : string.Empty;

    public ObservableCollection<CaptureDeviceDto> Devices { get; }
    public ObservableCollection<string> Hotkeys { get; }
    public ObservableCollection<AnnotationRowViewModel> Annotations { get; }

    public string SelectedDeviceId
    {
        get => selectedDeviceId;
        set
        {
            if (selectedDeviceId == value)
            {
                return;
            }
            selectedDeviceId = value;
            OnPropertyChanged();
            if (!string.IsNullOrWhiteSpace(value) && !refreshing && client is not null)
            {
                _ = SetDeviceAsync(value);
            }
        }
    }

    public string SelectedHotkey
    {
        get => selectedHotkey;
        set
        {
            value ??= string.Empty;
            if (selectedHotkey == value)
            {
                return;
            }
            selectedHotkey = value;
            hotkeyDraft = value;
            OnPropertyChanged();
            OnPropertyChanged(nameof(HotkeyDraft));
            OnPropertyChanged(nameof(HotkeyLabel));
            if (!string.IsNullOrWhiteSpace(value) && !refreshing && client is not null)
            {
                _ = SetHotkeyAsync(value);
            }
        }
    }

    public string HotkeyDraft
    {
        get => hotkeyDraft;
        set
        {
            value ??= string.Empty;
            if (hotkeyDraft == value)
            {
                return;
            }
            hotkeyDraft = value;
            OnPropertyChanged();
            (ApplyHotkeyCommand as RelayCommand)?.RaiseCanExecuteChanged();
        }
    }

    public string CaptureHotkeyLabel => capturingHotkey ? "Press a key..." : "Press a key";

    public AnnotationRowViewModel? SelectedAnnotation
    {
        get => selectedAnnotation;
        set
        {
            selectedAnnotation = value;
            OnPropertyChanged();
            RaiseCommands();
        }
    }

    public ICommand RecordCommand { get; }
    public ICommand JumpCommand { get; }
    public ICommand DismissCommand { get; }
    public ICommand ApplyHotkeyCommand { get; }
    public ICommand CaptureHotkeyCommand { get; }
    public ICommand CopyErrorCommand { get; }

    public event PropertyChangedEventHandler? PropertyChanged;

    internal void Refresh()
    {
        OnPropertyChanged(nameof(Status));
        OnPropertyChanged(nameof(CredentialPath));
    }

    private bool CanRecord =>
        !busy && client is not null && panel.Connection is not CaptureHostConnection.Missing;
    private bool CanJump =>
        !busy && SelectedAnnotation is not null && SelectedAnnotation.Lifecycle != "dismissed";
    private bool CanDismiss =>
        !busy && client is not null && SelectedAnnotation is { Lifecycle: "active" };
    private bool CanApplyHotkey =>
        !busy && client is not null && !string.IsNullOrWhiteSpace(hotkeyDraft);
    private bool CanCopyError => !string.IsNullOrWhiteSpace(lastError);

    private async Task RefreshAsync()
    {
        if (refreshing)
        {
            return;
        }
        refreshing = true;
        try
        {
            Refresh();
            client ??= CaptureHostClient.TryLoad();
            if (client is null)
            {
                panel.Connection = CaptureHostConnection.Missing;
                panel.Message = null;
                panel.Derive = null;
                lastError = string.Empty;
                NotifyPanel();
                return;
            }

            try
            {
                var status = await client.StatusAsync().ConfigureAwait(true);
                panel.Connection = AnnotationPanelState.FromStatus(status);
                panel.Hotkey = status.Hotkey;
                panel.ProjectId = status.ProjectId;
                panel.Message = status.State == "ymm4Disconnected"
                    ? "YMM4 composition is unavailable; listing still works."
                    : status.Message;
                panel.ActiveCaptureId = status.CaptureId;
                panel.RecordingStartFrame = status.StartFrame;
                panel.Derive = status.Derive;
                lastError = string.Empty;
                await RefreshDevicesAsync(status.DeviceId).ConfigureAwait(true);
                await RefreshHotkeysAsync(status.Hotkey).ConfigureAwait(true);
                await RefreshAnnotationsAsync().ConfigureAwait(true);
            }
            catch (Exception)
            {
                client.Dispose();
                client = null;
                panel.Connection = CaptureHostConnection.Missing;
                panel.Derive = null;
                lastError = string.Empty;
            }
            NotifyPanel();
        }
        finally
        {
            refreshing = false;
            RaiseCommands();
        }
    }

    private async Task RefreshDevicesAsync(string? selected)
    {
        if (client is null)
        {
            return;
        }
        var listed = await client.DevicesAsync().ConfigureAwait(true);
        Devices.Clear();
        foreach (var device in listed.Devices)
        {
            Devices.Add(device);
        }
        var next = selected
            ?? listed.Devices.FirstOrDefault(device => device.IsDefault)?.Id
            ?? listed.Devices.FirstOrDefault()?.Id
            ?? string.Empty;
        if (!string.Equals(selectedDeviceId, next, StringComparison.Ordinal))
        {
            selectedDeviceId = next;
            OnPropertyChanged(nameof(SelectedDeviceId));
        }
    }

    private async Task RefreshHotkeysAsync(string selected)
    {
        if (client is null)
        {
            return;
        }
        var listed = await client.HotkeysAsync().ConfigureAwait(true);
        Hotkeys.Clear();
        foreach (var hotkey in listed.Hotkeys)
        {
            Hotkeys.Add(hotkey);
        }
        var next = string.IsNullOrWhiteSpace(listed.Selected) ? selected : listed.Selected;
        if (!string.Equals(selectedHotkey, next, StringComparison.Ordinal))
        {
            selectedHotkey = next;
            hotkeyDraft = next;
            OnPropertyChanged(nameof(SelectedHotkey));
            OnPropertyChanged(nameof(HotkeyDraft));
            OnPropertyChanged(nameof(HotkeyLabel));
        }
    }

    private async Task RefreshAnnotationsAsync()
    {
        if (client is null)
        {
            return;
        }
        var scope = TakeGraphToolPlugin.Current.TryReadLiveAnnotationScope();
        if (scope is null)
        {
            Annotations.Clear();
            SelectedAnnotation = null;
            return;
        }
        panel.ProjectId = scope.ProjectId;
        var listed = await client.AnnotationsAsync(20, scope.ProjectId).ConfigureAwait(true);
        var selectedId = SelectedAnnotation?.AnnotationId;
        Annotations.Clear();
        foreach (var annotation in listed.Annotations)
        {
            Annotations.Add(new AnnotationRowViewModel(annotation));
        }
        SelectedAnnotation = Annotations.FirstOrDefault(row => row.AnnotationId == selectedId)
            ?? Annotations.FirstOrDefault();
        if (panel.Connection is not CaptureHostConnection.Recording
            and not CaptureHostConnection.Missing)
        {
            try
            {
                await TakeGraphToolPlugin.Current
                    .SyncAnnotationDecorationsAsync(listed.Annotations)
                    .ConfigureAwait(true);
            }
            catch (Exception error)
            {
                lastError = error.GetBaseException().Message;
            }
        }
    }

    private async Task ToggleRecordingAsync()
    {
        if (client is null)
        {
            return;
        }
        await RunBusyAsync(async () =>
        {
            if (panel.Connection == CaptureHostConnection.Recording)
            {
                await client.StopAsync().ConfigureAwait(true);
            }
            else
            {
                await client.StartAsync().ConfigureAwait(true);
            }
        }).ConfigureAwait(true);
    }

    private async Task JumpAsync()
    {
        if (SelectedAnnotation is null)
        {
            return;
        }
        var row = SelectedAnnotation;
        await RunBusyAsync(async () =>
        {
            await TakeGraphToolPlugin.Current.SeekAnnotationFrameAsync(
                new AnnotationSeekRequest(
                    row.ProjectId,
                    row.SceneId,
                    row.SourceFingerprint,
                    row.StartFrame)).ConfigureAwait(true);
        }).ConfigureAwait(true);
    }

    private async Task DismissAsync()
    {
        if (client is null || SelectedAnnotation is null)
        {
            return;
        }
        var id = SelectedAnnotation.AnnotationId;
        await RunBusyAsync(async () => await client.DismissAsync(id).ConfigureAwait(true))
            .ConfigureAwait(true);
    }

    private async Task SetHotkeyAsync(string hotkey)
    {
        if (client is null)
        {
            return;
        }
        try
        {
            var updated = await client.SetHotkeyAsync(hotkey).ConfigureAwait(true);
            panel.Hotkey = updated.Selected;
            selectedHotkey = updated.Selected;
            hotkeyDraft = updated.Selected;
            lastError = string.Empty;
            OnPropertyChanged(nameof(SelectedHotkey));
            OnPropertyChanged(nameof(HotkeyDraft));
            OnPropertyChanged(nameof(HotkeyLabel));
            OnPropertyChanged(nameof(CaptureHostHint));
        }
        catch (Exception error)
        {
            lastError = error.GetBaseException().Message;
            OnPropertyChanged(nameof(CaptureHostHint));
        }
    }

    private async Task SetDeviceAsync(string deviceId)
    {
        if (client is null || panel.Connection == CaptureHostConnection.Recording)
        {
            return;
        }
        try
        {
            await client.SetDeviceAsync(deviceId).ConfigureAwait(true);
        }
        catch (Exception error)
        {
            lastError = error.GetBaseException().Message;
            OnPropertyChanged(nameof(CaptureHostHint));
        }
    }

    private async Task RunBusyAsync(Func<Task> action)
    {
        busy = true;
        RaiseCommands();
        try
        {
            await action().ConfigureAwait(true);
            lastError = string.Empty;
        }
        catch (Exception error)
        {
            lastError = error.GetBaseException().Message;
        }
        finally
        {
            busy = false;
            await RefreshAsync().ConfigureAwait(true);
        }
    }

    private void NotifyPanel()
    {
        OnPropertyChanged(nameof(CaptureHostStatus));
        OnPropertyChanged(nameof(CaptureHostHint));
        OnPropertyChanged(nameof(HotkeyLabel));
        OnPropertyChanged(nameof(RecordLabel));
        OnPropertyChanged(nameof(RecordingIndicator));
    }

    private void RaiseCommands()
    {
        (RecordCommand as RelayCommand)?.RaiseCanExecuteChanged();
        (JumpCommand as RelayCommand)?.RaiseCanExecuteChanged();
        (DismissCommand as RelayCommand)?.RaiseCanExecuteChanged();
        (ApplyHotkeyCommand as RelayCommand)?.RaiseCanExecuteChanged();
        (CaptureHotkeyCommand as RelayCommand)?.RaiseCanExecuteChanged();
        (CopyErrorCommand as RelayCommand)?.RaiseCanExecuteChanged();
    }

    private Task CopyErrorAsync()
    {
        var text = ErrorClipboardText(lastError);
        if (text is not null)
        {
            Clipboard.SetText(text);
        }
        return Task.CompletedTask;
    }

    internal static string? ErrorClipboardText(string error) =>
        string.IsNullOrWhiteSpace(error) ? null : error;

    private Task ApplyHotkeyAsync()
    {
        if (string.IsNullOrWhiteSpace(hotkeyDraft))
        {
            return Task.CompletedTask;
        }
        return SetHotkeyAsync(hotkeyDraft.Trim());
    }

    private Task StartHotkeyCaptureAsync()
    {
        capturingHotkey = true;
        lastError = "Press the toggle key, with Ctrl/Alt/Shift if you want them.";
        OnPropertyChanged(nameof(CaptureHotkeyLabel));
        OnPropertyChanged(nameof(CaptureHostHint));
        return Task.CompletedTask;
    }

    internal bool TryCaptureKey(KeyEventArgs e)
    {
        if (!capturingHotkey)
        {
            return false;
        }
        var key = e.Key == Key.System ? e.SystemKey : e.Key;
        if (IsModifier(key))
        {
            return true;
        }
        capturingHotkey = false;
        OnPropertyChanged(nameof(CaptureHotkeyLabel));
        var spec = FormatCapturedHotkey(Keyboard.Modifiers, key);
        hotkeyDraft = spec;
        OnPropertyChanged(nameof(HotkeyDraft));
        _ = SetHotkeyAsync(spec);
        return true;
    }

    private static bool IsModifier(Key key) =>
        key is Key.LeftCtrl or Key.RightCtrl or Key.LeftAlt or Key.RightAlt
            or Key.LeftShift or Key.RightShift or Key.LWin or Key.RWin;

    internal static string FormatCapturedHotkey(ModifierKeys modifiers, Key key)
    {
        var parts = new List<string>();
        if (modifiers.HasFlag(ModifierKeys.Control))
        {
            parts.Add("Ctrl");
        }
        if (modifiers.HasFlag(ModifierKeys.Alt))
        {
            parts.Add("Alt");
        }
        if (modifiers.HasFlag(ModifierKeys.Shift))
        {
            parts.Add("Shift");
        }
        if (modifiers.HasFlag(ModifierKeys.Windows))
        {
            parts.Add("Win");
        }
        parts.Add(FormatKey(key));
        return string.Join("+", parts);
    }

    private static string FormatKey(Key key) => key switch
    {
        >= Key.A and <= Key.Z => key.ToString(),
        >= Key.D0 and <= Key.D9 => ((char)('0' + (key - Key.D0))).ToString(),
        >= Key.F1 and <= Key.F24 => key.ToString(),
        Key.Space => "Space",
        Key.Tab => "Tab",
        Key.Return => "Enter",
        Key.Escape => "Escape",
        Key.Back => "Backspace",
        Key.Delete => "Delete",
        Key.Insert => "Insert",
        Key.Home => "Home",
        Key.End => "End",
        Key.PageUp => "PageUp",
        Key.PageDown => "PageDown",
        Key.Pause => "Pause",
        Key.PrintScreen => "PrintScreen",
        Key.Scroll => "ScrollLock",
        Key.CapsLock => "CapsLock",
        Key.Up => "Up",
        Key.Down => "Down",
        Key.Left => "Left",
        Key.Right => "Right",
        Key.Oem3 => "`",
        Key.OemMinus => "-",
        Key.OemPlus => "=",
        Key.OemOpenBrackets => "[",
        Key.OemCloseBrackets => "]",
        Key.Oem5 => "\\",
        Key.Oem1 => ";",
        Key.OemQuotes => "'",
        Key.OemComma => ",",
        Key.OemPeriod => ".",
        Key.OemQuestion => "/",
        Key.NumPad0 => "Numpad0",
        Key.NumPad1 => "Numpad1",
        Key.NumPad2 => "Numpad2",
        Key.NumPad3 => "Numpad3",
        Key.NumPad4 => "Numpad4",
        Key.NumPad5 => "Numpad5",
        Key.NumPad6 => "Numpad6",
        Key.NumPad7 => "Numpad7",
        Key.NumPad8 => "Numpad8",
        Key.NumPad9 => "Numpad9",
        _ => key.ToString(),
    };

    private void OnPropertyChanged([CallerMemberName] string? name = null) =>
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
}

public sealed class TakeGraphBridgeView : UserControl
{
    public TakeGraphBridgeView()
    {
        var title = Heading("TakeGraph Voice Notes", 18, 0, 12);
        var bridge = BoundText(nameof(TakeGraphBridgeViewModel.Status));
        var credentialLabel = Heading("Bridge credential", 12, 12, 4);
        var credential = BoundText(nameof(TakeGraphBridgeViewModel.CredentialPath));
        var hostStatus = BoundText(nameof(TakeGraphBridgeViewModel.CaptureHostStatus));
        var hint = BoundText(nameof(TakeGraphBridgeViewModel.CaptureHostHint));
        hint.TextWrapping = TextWrapping.Wrap;
        var copyError = new Button
        {
            Content = "Copy error",
            Margin = new Thickness(8, 0, 0, 0),
            Padding = new Thickness(10, 4, 10, 4),
            VerticalAlignment = VerticalAlignment.Top,
        };
        copyError.SetBinding(Button.CommandProperty, nameof(TakeGraphBridgeViewModel.CopyErrorCommand));
        var hintRow = new DockPanel { LastChildFill = true, Margin = new Thickness(0, 0, 0, 0) };
        DockPanel.SetDock(copyError, Dock.Right);
        hintRow.Children.Add(copyError);
        hintRow.Children.Add(hint);
        var hotkeyLabel = Heading("Toggle hotkey", 12, 12, 4);
        var hotkeys = new ComboBox();
        hotkeys.SetBinding(ItemsControl.ItemsSourceProperty, nameof(TakeGraphBridgeViewModel.Hotkeys));
        hotkeys.SetBinding(ComboBox.SelectedItemProperty, new Binding(nameof(TakeGraphBridgeViewModel.SelectedHotkey))
        {
            Mode = BindingMode.TwoWay,
        });
        var custom = new TextBox { Margin = new Thickness(0, 6, 0, 0) };
        custom.SetBinding(TextBox.TextProperty, new Binding(nameof(TakeGraphBridgeViewModel.HotkeyDraft))
        {
            Mode = BindingMode.TwoWay,
            UpdateSourceTrigger = UpdateSourceTrigger.PropertyChanged,
        });
        var applyHotkey = new Button { Content = "Apply", Margin = new Thickness(0, 6, 8, 0), Padding = new Thickness(10, 4, 10, 4) };
        applyHotkey.SetBinding(Button.CommandProperty, nameof(TakeGraphBridgeViewModel.ApplyHotkeyCommand));
        var captureHotkey = BoundButton(
            nameof(TakeGraphBridgeViewModel.CaptureHotkeyCommand),
            nameof(TakeGraphBridgeViewModel.CaptureHotkeyLabel));
        captureHotkey.Margin = new Thickness(0, 6, 0, 0);
        var hotkeyActions = new StackPanel
        {
            Orientation = Orientation.Horizontal,
            Children = { applyHotkey, captureHotkey },
        };
        var recordIndicator = BoundText(nameof(TakeGraphBridgeViewModel.RecordingIndicator));
        PreviewKeyDown += (_, e) =>
        {
            if (DataContext is TakeGraphBridgeViewModel viewModel && viewModel.TryCaptureKey(e))
            {
                e.Handled = true;
            }
        };

        var deviceLabel = Heading("Microphone", 12, 12, 4);
        var devices = new ComboBox { DisplayMemberPath = nameof(CaptureDeviceDto.Name) };
        devices.SetBinding(ItemsControl.ItemsSourceProperty, nameof(TakeGraphBridgeViewModel.Devices));
        devices.SetBinding(ComboBox.SelectedValueProperty, new Binding(nameof(TakeGraphBridgeViewModel.SelectedDeviceId))
        {
            Mode = BindingMode.TwoWay,
        });
        devices.SelectedValuePath = nameof(CaptureDeviceDto.Id);

        var record = BoundButton(
            nameof(TakeGraphBridgeViewModel.RecordCommand),
            nameof(TakeGraphBridgeViewModel.RecordLabel));
        var jump = new Button { Content = "Jump to start", Margin = new Thickness(0, 8, 8, 0) };
        jump.SetBinding(Button.CommandProperty, nameof(TakeGraphBridgeViewModel.JumpCommand));
        var dismiss = new Button { Content = "Dismiss", Margin = new Thickness(0, 8, 0, 0) };
        dismiss.SetBinding(Button.CommandProperty, nameof(TakeGraphBridgeViewModel.DismissCommand));
        var actions = new StackPanel
        {
            Orientation = Orientation.Horizontal,
            Children = { jump, dismiss },
        };

        var listLabel = Heading("Recent notes", 12, 12, 4);
        var list = new ListBox
        {
            DisplayMemberPath = nameof(AnnotationRowViewModel.DisplayLabel),
            MinHeight = 120,
            MaxHeight = 240,
        };
        list.SetBinding(ItemsControl.ItemsSourceProperty, nameof(TakeGraphBridgeViewModel.Annotations));
        list.SetBinding(ListBox.SelectedItemProperty, new Binding(nameof(TakeGraphBridgeViewModel.SelectedAnnotation))
        {
            Mode = BindingMode.TwoWay,
        });

        Content = new ScrollViewer
        {
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            Content = new StackPanel
            {
                Margin = new Thickness(16),
                Children =
                {
                    title,
                    bridge,
                    credentialLabel,
                    credential,
                    hostStatus,
                    hintRow,
                    hotkeyLabel,
                    hotkeys,
                    custom,
                    hotkeyActions,
                    recordIndicator,
                    deviceLabel,
                    devices,
                    record,
                    listLabel,
                    list,
                    actions,
                },
            },
        };
    }

    private static TextBlock Heading(string text, double size, double top, double bottom) =>
        new()
        {
            Text = text,
            FontSize = size,
            FontWeight = FontWeights.SemiBold,
            Margin = new Thickness(0, top, 0, bottom),
        };

    private static TextBlock BoundText(string path)
    {
        var block = new TextBlock { TextWrapping = TextWrapping.Wrap };
        block.SetBinding(TextBlock.TextProperty, path);
        return block;
    }

    private static Button BoundButton(string commandPath, string contentPath)
    {
        var button = new Button { Margin = new Thickness(0, 8, 0, 0), Padding = new Thickness(10, 4, 10, 4) };
        button.SetBinding(Button.CommandProperty, commandPath);
        button.SetBinding(ContentControl.ContentProperty, contentPath);
        return button;
    }
}
