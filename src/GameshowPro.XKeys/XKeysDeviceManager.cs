using GameshowPro.XKeys.Hid;

namespace GameshowPro.XKeys;

/// <summary>
/// Keeps a fixed list of <see cref="XKeysDevice{TTriggerKey}"/> attached to the physical devices that are present, with the library's
/// push API and nothing of its own that polls, loops or waits on a timer: an <see cref="XkDeviceMonitor"/> opens every X-keys interface
/// that is present or arrives, and reopens one that comes back after an unplug, and this class routes each opened session to a device of
/// the list. A device that is plugged in again goes back to the device it was attached to before (by interface path); otherwise a session
/// goes to a free device whose settings have its product ID, else to one whose settings have none. A session that no device can take waits,
/// started, until one becomes free.
/// </summary>
internal sealed class XKeysDeviceManager<TTriggerKey> : IDisposable
    where TTriggerKey : notnull, Enum
{
    /// <summary>The ring of each open device: far more than a person can press between two drains.</summary>
    internal const uint RingCapacity = 256;

    /// <summary>How long <see cref="WaitForInitialSessions"/> waits for the devices present at start to be opened. Opening one takes milliseconds.</summary>
    internal static readonly TimeSpan InitialOpenTimeout = TimeSpan.FromSeconds(5);

    private readonly ILogger _logger;
    // Guards everything below; never held while a session is started or written to, or while a device's state changes.
    private readonly Lock _sync = new();
    // Every session the monitor handed over that has not ended, and the device it is routed to (null: waiting for a free device).
    private readonly Dictionary<XkDevice, XKeysDevice<TTriggerKey>?> _sessions = [];
    private readonly HashSet<string> _initialPending = new(StringComparer.OrdinalIgnoreCase);
    private readonly ManualResetEventSlim _initialDone = new(true);
    // The monitor may hand over sessions before the device list exists; its handlers wait for this.
    private readonly ManualResetEventSlim _devicesReady = new(false);
    private ImmutableList<XKeysDevice<TTriggerKey>> _devices = [];
    private bool _disposed;

    public XKeysDeviceManager(ILogger logger)
    {
        _logger = logger;
        Monitor = new XkDeviceMonitor(new XkDeviceMonitorOptions
        {
            OpenDevices = true,
            RingCapacity = RingCapacity,
            OpenFlags = XkOpenFlags.SuppressDuplicates,
            // The device a session is routed to starts it and asks for its state; one that waits for a free device sends nothing.
            RequestStateOnStart = false,
        });
        Monitor.DeviceOpened += OnDeviceOpened;
        Monitor.OpenFailed += OnOpenFailed;
        Monitor.DeviceRemoved += (_, e) => _logger.LogInformation("X-keys interface 0x{Pid:X4} left: {Path}", e.Descriptor.Pid, e.Descriptor.Path);
        Monitor.EnumerationFailed += (_, e) => _logger.LogError(e.Exception, "Looking for X-keys devices failed ({Context}); trying again at the next device change", e.Context);
        Monitor.HandlerFailed += (_, e) => _logger.LogError(e.Exception, "Handling an X-keys device change failed ({Context})", e.Context);
    }

    /// <summary>The library's monitor, which this class owns. Tests simulate arrivals and removals through it.</summary>
    internal XkDeviceMonitor Monitor { get; }

    /// <summary>
    /// Start the monitor: it enumerates on this thread, so <see cref="XkDeviceMonitor.PresentDevices"/> is filled when this returns. The
    /// sessions it opens wait for <see cref="SetDevices"/>.
    /// </summary>
    /// <exception cref="XkHidException">The native library cannot be used, or the first enumeration failed.</exception>
    public IReadOnlyList<XkDeviceDescriptor> Start()
    {
        Monitor.Start();
        IReadOnlyList<XkDeviceDescriptor> present = Monitor.PresentDevices;
        lock (_sync)
        {
            foreach (XkDeviceDescriptor descriptor in present)
            {
                _ = _initialPending.Add(descriptor.Path);
            }
            if (_initialPending.Count > 0)
            {
                _initialDone.Reset();
            }
        }
        return present;
    }

    /// <summary>Give the manager the list it attaches sessions to, and let the waiting sessions through.</summary>
    public void SetDevices(ImmutableList<XKeysDevice<TTriggerKey>> devices)
    {
        lock (_sync)
        {
            _devices = devices;
        }
        foreach (XKeysDevice<TTriggerKey> device in devices)
        {
            device.Manager = this;
        }
        _devicesReady.Set();
    }

    /// <summary>Wait (up to <see cref="InitialOpenTimeout"/>) until every interface present at <see cref="Start"/> has been opened and attached, or has failed to open.</summary>
    public bool WaitForInitialSessions() => _initialDone.Wait(InitialOpenTimeout);

    /// <summary>The monitor opened an interface (on its thread). Route the session to a device, subscribe, and attach it.</summary>
    private void OnDeviceOpened(object? sender, XkDeviceOpenedEventArgs e)
    {
        _devicesReady.Wait();
        XkDevice session = e.Device;
        XKeysDevice<TTriggerKey>? owner;
        lock (_sync)
        {
            if (_disposed)
            {
                return;
            }
            owner = ChooseFreeDevice(session.Descriptor);
            _sessions[session] = owner;
        }
        session.ReportReceived += (_, report) => OwnerOf(session)?.OnReport(session, report);
        session.InputsChanged += (_, group) => OwnerOf(session)?.OnInputs(session, group);
        session.Closed += (_, closed) => OnSessionClosed(session, closed);
        session.HandlerFailed += (_, failure) => _logger.LogError(failure.Exception, "Handling a report of X-keys device 0x{Pid:X4} failed ({Context})", session.Pid, failure.Context);
        if (owner is null)
        {
            _logger.LogInformation("X-keys device 0x{Pid:X4} at {Path} is not used: every device of the list is attached. It is used when one becomes free.", session.Pid, session.Path);
            // The monitor starts it when this handler returns, so that its closure is seen.
        }
        else
        {
            AttachTo(owner, session);
        }
        CompleteInitial(session.Path);
    }

    private void OnOpenFailed(object? sender, XkOpenFailedEventArgs e)
    {
        _logger.LogWarning("X-keys device 0x{Pid:X4} at {Path} could not be opened: {Error}; trying again at the next device change", e.Descriptor.Pid, e.Descriptor.Path, XkError.Name(e.Error));
        CompleteInitial(e.Descriptor.Path);
    }

    private void CompleteInitial(string path)
    {
        lock (_sync)
        {
            if (_initialPending.Remove(path) && _initialPending.Count == 0)
            {
                _initialDone.Set();
            }
        }
    }

    private XKeysDevice<TTriggerKey>? OwnerOf(XkDevice session)
    {
        lock (_sync)
        {
            return _sessions.TryGetValue(session, out XKeysDevice<TTriggerKey>? owner) ? owner : null;
        }
    }

    /// <summary>
    /// A device of the list with no session routed to it, for <paramref name="descriptor"/>: the one last attached to the same interface path,
    /// else one whose settings have the same product ID, else one whose settings have none. Null if there is none. Call with <see cref="_sync"/> held.
    /// </summary>
    private XKeysDevice<TTriggerKey>? ChooseFreeDevice(XkDeviceDescriptor descriptor)
    {
        HashSet<XKeysDevice<TTriggerKey>> taken = [.. _sessions.Values.OfType<XKeysDevice<TTriggerKey>>()];
        List<XKeysDevice<TTriggerKey>> free = [.. _devices.Where(d => !taken.Contains(d))];
        return free.FirstOrDefault(d => string.Equals(d.LastPath, descriptor.Path, StringComparison.OrdinalIgnoreCase))
            ?? free.FirstOrDefault(d => d.LastPath is null && d.Settings.Pid == descriptor.Pid)
            ?? free.FirstOrDefault(d => d.Settings.Pid == descriptor.Pid)
            ?? free.FirstOrDefault(d => d.Settings.Pid == 0);
    }

    private void AttachTo(XKeysDevice<TTriggerKey> owner, XkDevice session)
    {
        if (owner.Settings.Pid == 0)
        {
            // Settings without a product ID take the first device's, for future persistence.
            owner.Settings.Pid = session.Pid;
        }
        try
        {
            owner.Attach(session);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Attaching X-keys device 0x{Pid:X4} at {Path} failed", session.Pid, session.Path);
        }
    }

    /// <summary>A session ended (on its thread). Detach it from its device, and give that device a session that is waiting for one, if any fits.</summary>
    private void OnSessionClosed(XkDevice session, XkClosedEventArgs closed)
    {
        XKeysDevice<TTriggerKey>? owner;
        lock (_sync)
        {
            _ = _sessions.Remove(session, out owner);
        }
        if (owner is null)
        {
            return;
        }
        owner.Detach(session, closed.Reason);
        XkDevice? waiting = null;
        lock (_sync)
        {
            if (!_disposed)
            {
                foreach ((XkDevice candidate, XKeysDevice<TTriggerKey>? candidateOwner) in _sessions)
                {
                    if (candidateOwner is null && !candidate.IsClosed && ChooseFreeDevice(candidate.Descriptor) == owner)
                    {
                        waiting = candidate;
                        _sessions[candidate] = owner;
                        break;
                    }
                }
            }
        }
        if (waiting is not null)
        {
            AttachTo(owner, waiting);
        }
    }

    /// <summary>Stop the monitor and close every session (each device goes to Disconnected, its triggers released).</summary>
    public void Dispose()
    {
        ImmutableList<XKeysDevice<TTriggerKey>> devices;
        lock (_sync)
        {
            if (_disposed)
            {
                return;
            }
            _disposed = true;
            devices = _devices;
        }
        _devicesReady.Set();
        Monitor.Dispose();
        foreach (XKeysDevice<TTriggerKey> device in devices)
        {
            device.Manager = null;
        }
    }
}
