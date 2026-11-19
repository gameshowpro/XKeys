using GameshowPro.XKeys.Hid;

namespace GameshowPro.XKeys;

public enum LedState : byte
{
    Off = 0,
    On = 1,
    Flash = 2
}

/// <summary>Which commands <see cref="XKeysDevice{TTriggerKey}"/> sends on its own. Everything on by default; tests that must not disturb a real device turn parts off.</summary>
internal sealed record XKeysDeviceOptions(bool Identify, bool RequestDeviceInfo, bool RequestUpdate)
{
    public static XKeysDeviceOptions Default { get; } = new(true, true, true);
}

/// <summary>
/// One X-keys device as a GameshowPro incoming trigger device. It is attached to a physical device while one is connected and keeps its
/// triggers, settings and place in the list from <see cref="XKeysDevices{TTriggerKey}.GetDevices(IList{XKeysDeviceSettings}, ILoggerFactory)"/>
/// across unplugging and replugging.
/// </summary>
/// <remarks>
/// Nothing polls and this class runs no loop or timer of its own: the library's <see cref="XkDeviceMonitor"/> tells it when its device is
/// opened, and the open <see cref="XkDevice"/> raises its reports, decoded input changes and closure on the device's own thread, which is
/// where the triggers change and fire. Each trigger that fires because of an input change carries the time of the report that changed it
/// (see <see cref="XKeysIncomingTrigger"/>).
/// </remarks>
public class XKeysDevice<TTriggerKey> : IncomingTriggerDevice<TTriggerKey, XKeysIncomingTrigger, XKeysDevice<TTriggerKey>>
    where TTriggerKey : notnull, Enum
{
    private const byte REQUEST_UPDATE = 177;
    private const byte SET_LEDS = 179;
    private const byte SET_UNIT_ID = 189;
    private const byte REQUEST_DEVICE_INFO = 214;

    private const byte LED_GREEN = 6;
    private const byte LED_RED = 7;

    /// <summary>How long a write may take before it is abandoned. A healthy write takes about 2 ms.</summary>
    private const int WriteTimeoutMilliseconds = 1000;

    // _sync guards the session fields; _writeLock serializes output reports (each takes the session without holding _sync).
    private readonly Lock _sync = new();
    private readonly Lock _writeLock = new();
    private XkDevice? _session;
    private string? _lastPath;
    private (LedState Green, LedState Red)? _requestedLeds;

    /// <summary>The open session on a physical device, or null while none is connected.</summary>
    internal XkDevice? Session
    {
        get { lock (_sync) { return _session; } }
    }

    internal bool IsConnected => Session is not null;

    /// <summary>
    /// Free use of every light of the connected device (indicator LEDs, flash frequency, backlights, the Switch Interface's digital
    /// outputs): see <see cref="XkLights"/>. Null while no device is connected; take it afresh after a reconnect, because a reopened
    /// device starts in its stored light state. Writes through it do not update <see cref="GreenLedIsOn"/> or <see cref="RedLedIsOn"/>,
    /// and are not restored after a reconnect (<see cref="SetLeds"/> is).
    /// </summary>
    public XkLights? Lights => Session?.Lights;

    /// <summary>The interface the connected device was opened on, or null while none is connected. Its position in the library's device order is <see cref="XkDeviceDescriptor.Order"/>.</summary>
    public XkDeviceDescriptor? Descriptor => Session?.Descriptor;

    /// <summary>The device interface path of the open session, or null.</summary>
    internal string? CurrentPath => Session?.Path;

    /// <summary>The path of the interface this device was last attached to (or was found on), or null if it never was. A device that comes back on this path is attached here again.</summary>
    internal string? LastPath
    {
        get { lock (_sync) { return _lastPath; } }
        set { lock (_sync) { _lastPath = value; } }
    }

    /// <summary>
    /// The catalogue entry of the connected device's model, or, while none is connected, of the product ID in <see cref="Settings"/> (a
    /// generic model while that is 0 or unknown). Names its inputs: see <see cref="XkModel.InputName"/> and <see cref="XKeysIncomingTrigger.InputName"/>.
    /// </summary>
    public XkModel Model => Session?.Model ?? XkModel.ForPid((ushort)Settings.Pid);

    /// <summary>What this device sends on its own after opening, and what <see cref="XKeysDevices{TTriggerKey}"/> does with it.</summary>
    internal XKeysDeviceOptions Options { get; set; } = XKeysDeviceOptions.Default;

    /// <summary>Set when the native library cannot be used; shown as the detail of the disconnected state in place of "Not found".</summary>
    internal string? UnavailableReason { get; set; }

    /// <summary>The object that attaches this device to physical devices as they come and go, or null for a standalone device.</summary>
    internal XKeysDeviceManager<TTriggerKey>? Manager { get; set; }

    internal void UpdateState()
    {
        if (Session == null)
        {
            ServiceState.SetAll(RemoteServiceStates.Disconnected, UnavailableReason ?? "Not found", 0);
        }
        else
        {
            ServiceState.SetAll(RemoteServiceStates.Connected, "Connected", 1);
        }
    }

    /// <summary>The unit ID the device reports in every report (index 1). Zero until a report has arrived.</summary>
    public byte Id { get; internal set; }
    /// <summary>The USB product ID the device reports in its descriptor report (bytes 12 and 13, low byte first), for example
    /// 0x0426 for the first PID of an XK-12 Jog &amp; Shuttle. Zero until the descriptor report has arrived.</summary>
    public ushort Pid { get; private set; }
    /// <summary>The PID mode from the descriptor report (byte 3): which of the model's product IDs the device is using.</summary>
    public byte PidMode { get; private set; }
    /// <summary>The firmware version from the descriptor report (byte 11).</summary>
    public byte FirmwareVersion { get; private set; }
    private readonly string _namePrefix;

    /// <summary>Create an XKeysDevice</summary>
    /// <param name="namePrefix">The root of the name given to this instance.</param>
    /// <param name="settings">The settings of the device and of each of its triggers.</param>
    internal XKeysDevice(
        string namePrefix,
        XKeysDeviceSettings settings,
        ILoggerFactory loggerFactory
    )
    :
        base(
            namePrefix,
            settings.Pid,
            settings,
            loggerFactory
        )
    {
        _namePrefix = namePrefix;
        Settings = settings;
        UpdateState();
    }

    protected override ServiceState CreateServiceState()
        => new(_namePrefix); //Todo: find some way to combine with suffix from settings


    protected override XKeysIncomingTrigger TriggerFactory(IncomingTriggerSetting triggerSetting, ILoggerFactory loggerFactory)
        => new(triggerSetting, this, loggerFactory.CreateLogger($"{nameof(XKeysIncomingTrigger)}[{Index}]({triggerSetting.Key})")) { ModelProvider = () => Model };

    public XKeysDeviceSettings Settings { get; }
    public override IncomingTriggerDeviceSettingsBase BaseSettings => Settings;

    /// <summary>
    /// Take <paramref name="session"/>, which the manager has routed to this device, and start it if it is not started yet. The device's
    /// state is asked for (Generate Data), so the triggers show it; the LED state is sent again, because the device holds none of ours.
    /// </summary>
    internal void Attach(XkDevice session)
    {
        lock (_sync)
        {
            if (_session is not null)
            {
                throw new InvalidOperationException("The device already has an open session.");
            }
            _session = session;
            _lastPath = session.Path;
        }
        _logger.LogInformation("X-keys device {Model} (0x{Pid:X4}) attached at {Path}", session.Model.NameForPid(session.Pid), session.Pid, session.Path);
        UpdateState();
        try
        {
            if (session.DeliveryMode == XkDeliveryMode.None)
            {
                _ = session.Start(requestState: Options.RequestUpdate);
            }
            else if (Options.RequestUpdate)
            {
                // A session that was already running (it waited for a free device): ask for its state now that someone listens.
                _ = session.RequestState();
            }
        }
        catch (Exception ex) when (ex is ObjectDisposedException or InvalidOperationException)
        {
            // The session ended in the meantime; its Closed event detaches it.
            return;
        }
        if (Options.RequestDeviceInfo)
        {
            RequestDeviceInfo();
        }
        if (_requestedLeds is { } leds)
        {
            SetLeds(leds.Green, leds.Red);
        }
    }

    /// <summary>The session ended (on its own thread): every input is released without firing, since the device can no longer say so.</summary>
    internal void Detach(XkDevice session, XkCloseReason reason)
    {
        lock (_sync)
        {
            if (_session != session)
            {
                return;
            }
            _session = null;
        }
        if (reason == XkCloseReason.Disposed)
        {
            _logger.LogInformation("X-keys device 0x{Pid:X4} at {Path} was closed", session.Pid, session.Path);
        }
        else
        {
            _logger.LogWarning("X-keys device 0x{Pid:X4} at {Path} ended ({Reason}); it is attached again when it comes back", session.Pid, session.Path, reason);
        }
        foreach (XKeysIncomingTrigger trigger in Triggers.Values)
        {
            trigger.Release();
        }
        UpdateState();
    }

    /// <summary>Every report of the session (on its thread): the unit ID, the descriptor, and the state carried by an answer to Generate Data.</summary>
    internal void OnReport(XkDevice session, XkReportEventArgs report)
    {
        if (session != Session)
        {
            return;
        }
        ReadOnlySpan<byte> data = report.Data;
        if (data.Length > XkReport.UnitIdIndex)
        {
            Id = data[XkReport.UnitIdIndex];
        }
        switch (report.Kind)
        {
            case XkReportKind.Descriptor when XkDescriptorReport.TryParse(data, out XkDescriptorReport? descriptor):
                // Measured on an XK-12 Jog & Shuttle and an XK-12 Switch Interface (2026-11-16), as the vendor documents it.
                PidMode = descriptor.PidMode;
                FirmwareVersion = descriptor.FirmwareVersion;
                Pid = descriptor.Pid;
                break;
            case XkReportKind.Generated:
                // The device's whole state, at connection, after RequestUpdate, or after lost reports. It is state, not a press: the
                // triggers show it without firing. (A difference also comes as a resynchronization group, which OnInputs leaves alone.)
                ApplyState(XkReport.InputWord(data) & session.Model.InputMask);
                break;
        }
    }

    /// <summary>The inputs one input report changed (on the session's thread): each trigger on a changed input gets the report's stamp.</summary>
    internal void OnInputs(XkDevice session, XkInputGroup group)
    {
        if (session != Session)
        {
            return;
        }
        ApplyInputs(group);
    }

    /// <summary>Apply a decoded group; every change in it carries the same stamp, so the triggers it fires get the same time.</summary>
    internal void ApplyInputs(XkInputGroup group)
    {
        if (group.IsResynchronization)
        {
            // Applied as state with the report itself (OnReport); its time is unknown, so it fires nothing.
            ApplyState(group.PressedMask);
            return;
        }
        foreach (XkInputChange change in group.Changes)
        {
            foreach (XKeysIncomingTrigger trigger in Triggers.Values)
            {
                if (trigger.Setting.Id == change.Input)
                {
                    trigger.ApplyChange(change.IsPressed, change.Stamp);
                }
            }
        }
    }

    /// <summary>Show <paramref name="pressedMask"/> (bits of the input word) on every trigger, without firing any.</summary>
    internal void ApplyState(uint pressedMask)
    {
        foreach (XKeysIncomingTrigger trigger in Triggers.Values)
        {
            int id = trigger.Setting.Id;
            trigger.ApplyState(id is >= 0 and < XkReport.InputBitCount && (pressedMask & (1u << id)) != 0);
        }
    }

    public bool RedLedIsOn
    {
        get;
        internal set { SetProperty(ref field, value); }
    }

    public bool GreenLedIsOn
    {
        get;
        internal set { SetProperty(ref field, value); }
    }

    /// <summary>Set the two LEDs. Does nothing while no device is connected; the last request is applied again when the device comes back.</summary>
    public void SetLeds(LedState green, LedState red)
    {
        _requestedLeds = (green, red);
        if (SendCommand(SET_LEDS, LED_GREEN, (byte)green))
        {
            GreenLedIsOn = green != LedState.Off;
        }
        if (SendCommand(SET_LEDS, LED_RED, (byte)red))
        {
            RedLedIsOn = red != LedState.Off;
        }
    }

    /// <summary>Set the unit ID stored in the device. Does nothing while no device is connected.</summary>
    public void SetUnitId(byte id)
    {
        _ = SendCommand(SET_UNIT_ID, id);
    }

    /// <summary>
    /// Ask the device to report the current state of every input (Generate Data). Does nothing while no device is connected. The answer is
    /// shown on the triggers as state: it fires none, since it is not a press (and differs from what they show only if reports were lost).
    /// </summary>
    public void RequestUpdate()
    {
        _ = SendCommand(REQUEST_UPDATE);
    }

    private void RequestDeviceInfo()
    {
        _ = SendCommand(REQUEST_DEVICE_INFO);
    }

    /// <summary>
    /// Send one 36-byte output report: report ID 0, the command, then up to two arguments, the rest zero. Returns false, after logging
    /// why, when no device is connected or the device did not take it (a write that is still pending after a second is cancelled).
    /// </summary>
    private bool SendCommand(byte command, byte argument1 = 0, byte argument2 = 0)
    {
        XkDevice? session = Session;
        if (session is null)
        {
            _logger.LogDebug("Command {Command} not sent: no device is connected", command);
            return false;
        }
        if (session.OutputLength < 4)
        {
            _logger.LogWarning("Command {Command} not sent: the device has no usable output report", command);
            return false;
        }
        lock (_writeLock)
        {
            Span<byte> report = stackalloc byte[session.OutputLength];
            report.Clear();
            report[1] = command;
            report[2] = argument1;
            report[3] = argument2;
            try
            {
                int result = session.Write(report, WriteTimeoutMilliseconds, out uint win32Error);
                if (result != XkError.Ok)
                {
                    _logger.LogWarning("Command {Command} failed: {Result} (Win32 error {Win32Error})", command, XkError.Name(result), win32Error);
                }
                return result == XkError.Ok;
            }
            catch (ObjectDisposedException)
            {
                return false;
            }
        }
    }
}
