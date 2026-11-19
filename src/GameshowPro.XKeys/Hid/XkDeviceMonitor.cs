// Copyright (c) Barjonas LLC 2026.
using System.Diagnostics;

namespace GameshowPro.XKeys.Hid;

/// <summary>What an <see cref="XkDeviceMonitor"/> watches for and whether it opens what it finds.</summary>
public sealed record XkDeviceMonitorOptions
{
    /// <summary>The vendor; 0 means P.I. Engineering (0x05F3).</summary>
    public ushort Vid { get; init; }

    /// <summary>The usage page of the interfaces to report; 0 means any. Default: the Consumer page interface that carries the key data.</summary>
    public ushort UsagePage { get; init; } = XkEnumerator.ConsumerUsagePage;

    /// <summary>The output report length of the interfaces to report; 0 means any. Default 36.</summary>
    public ushort OutputLength { get; init; } = XkEnumerator.ConsumerOutputLength;

    /// <summary>Also report interfaces without an input report (a model's output-only PIDs). They are never opened.</summary>
    public bool IncludeInterfacesWithoutInput { get; init; }

    /// <summary>
    /// A further test for the interfaces to report, or null for all. Called on the thread that enumerates (the caller of
    /// <see cref="XkDeviceMonitor.Start"/>, then the monitor's thread); it must be quick and must not throw.
    /// </summary>
    public Func<XkDeviceDescriptor, bool>? Filter { get; init; }

    /// <summary>
    /// Open every reported interface that has an input report, hand it over with <see cref="XkDeviceMonitor.DeviceOpened"/> and start it,
    /// and open it again when it comes back after an unplug.
    /// </summary>
    public bool OpenDevices { get; init; }

    /// <summary>Ring capacity of the devices the monitor opens; 0 lets the library choose 256.</summary>
    public uint RingCapacity { get; init; }

    /// <summary>Open flags of the devices the monitor opens.</summary>
    public XkOpenFlags OpenFlags { get; init; }

    /// <summary>Whether the monitor asks each device it starts for its state (Generate Data; see <see cref="XkDevice.Start"/>). Default true.</summary>
    public bool RequestStateOnStart { get; init; } = true;
}

/// <summary>
/// Tells you, without polling and without enumerating by hand, which X-keys interfaces are present: <see cref="DeviceArrived"/> for
/// each one present at <see cref="Start"/> and for each that is plugged in later, <see cref="DeviceRemoved"/> for each that leaves.
/// With <see cref="XkDeviceMonitorOptions.OpenDevices"/> it also opens them, hands each session over with <see cref="DeviceOpened"/>,
/// starts it, and opens it again after an unplug and replug. Devices are kept and reported in the library's device order
/// (<see cref="XkDeviceDescriptor.Order"/>): the initial arrivals, the arrivals and removals found by one enumeration, and the snapshots.
/// </summary>
/// <remarks>
/// <para>
/// It sleeps on the native library's arrival/removal notification (<see cref="XkWatch"/>) and enumerates once each time it fires: no
/// timer, no settle delay. A device's interfaces arrive one at a time and each fires the notification, so an interface that could not be
/// opened at its first notification is tried again at the next; <see cref="OpenFailed"/> reports each failure. (Whether opening right at
/// arrival ever fails has not been observed: hot-plug was not tested.)
/// </para>
/// <para>
/// <b>Threading.</b> The monitor's events (<see cref="DeviceArrived"/>, <see cref="DeviceRemoved"/>, <see cref="DeviceOpened"/>,
/// <see cref="OpenFailed"/>, <see cref="EnumerationFailed"/>, <see cref="HandlerFailed"/>) are raised one at a time on the monitor's own
/// background thread ("XKeys device monitor"), in the order things were found: for an interface, <see cref="DeviceArrived"/> comes before
/// its <see cref="DeviceOpened"/>. The events of an opened device come from that device's own thread (see <see cref="XkDevice"/>), so a
/// device's <see cref="XkDevice.Closed"/> and the monitor's <see cref="DeviceRemoved"/> for the same unplug are not ordered against each
/// other. Handlers must be quick. A handler that throws is caught and reported through <see cref="HandlerFailed"/>; the monitor goes on.
/// </para>
/// <para>
/// <b>Sessions.</b> The monitor owns the devices it opens. A device is started (<see cref="XkDevice.Start"/>) right after its
/// <see cref="DeviceOpened"/> handlers return, unless a handler started it, used its pull API or disposed it. A consumer may dispose a
/// device early; the monitor then leaves that interface alone until it leaves and comes back. A session that ends by itself (removed, or
/// a read error) is opened again when its interface is present after any later arrival or removal notification. When the interface
/// leaves, the monitor ends a session that is still open with <see cref="XkCloseReason.Removed"/>. <see cref="Dispose"/> disposes every
/// device the monitor opened.
/// </para>
/// </remarks>
public sealed class XkDeviceMonitor : IDisposable
{
    private sealed class Session(XkDevice device, ulong openedAtGeneration)
    {
        public XkDevice Device { get; } = device;
        public ulong OpenedAtGeneration { get; } = openedAtGeneration;
    }

    private readonly XkDeviceMonitorOptions _options;
    // Guards the snapshots read by other threads and the start/stop state; never held while a handler runs.
    private readonly Lock _sync = new();
    private readonly ManualResetEvent _stop = new(false);
    private readonly AutoResetEvent _rescan = new(false);
    private readonly Dictionary<string, XkDeviceDescriptor> _present = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<string, Session> _sessions = new(StringComparer.OrdinalIgnoreCase);
    // Interfaces whose session the consumer disposed: left alone until they leave.
    private readonly HashSet<string> _consumerClosed = new(StringComparer.OrdinalIgnoreCase);
    private readonly HashSet<string> _everOpened = new(StringComparer.OrdinalIgnoreCase);
    private XkWatch? _watch;
    private Thread? _thread;
    private List<XkDeviceDescriptor> _initial = [];
    private bool _started;
    private bool _disposed;
    private volatile bool _stopping;

    public XkDeviceMonitor(XkDeviceMonitorOptions? options = null)
    {
        _options = options ?? new XkDeviceMonitorOptions();
    }

    public XkDeviceMonitorOptions Options => _options;

    /// <summary>An interface is present: one for each present at <see cref="Start"/> (<see cref="XkDeviceEventArgs.WasPresentAtStart"/>), then one per arrival.</summary>
    public event EventHandler<XkDeviceEventArgs>? DeviceArrived;

    /// <summary>An interface left.</summary>
    public event EventHandler<XkDeviceEventArgs>? DeviceRemoved;

    /// <summary>
    /// With <see cref="XkDeviceMonitorOptions.OpenDevices"/>: the monitor opened an interface. Subscribe to the device's events in the
    /// handler; the monitor starts the device when every handler has returned.
    /// </summary>
    public event EventHandler<XkDeviceOpenedEventArgs>? DeviceOpened;

    /// <summary>With <see cref="XkDeviceMonitorOptions.OpenDevices"/>: an interface could not be opened. It is tried again at the next notification.</summary>
    public event EventHandler<XkOpenFailedEventArgs>? OpenFailed;

    /// <summary>Enumerating after a notification failed; the monitor goes on and tries again at the next one.</summary>
    public event EventHandler<XkErrorEventArgs>? EnumerationFailed;

    /// <summary>A handler of one of the monitor's events threw.</summary>
    public event EventHandler<XkErrorEventArgs>? HandlerFailed;

    /// <summary>
    /// The interfaces present as of the last enumeration (a snapshot), in the library's device order (<see cref="XkDeviceDescriptor.Order"/>;
    /// see <see cref="XkEnumerator.Enumerate"/> for what that order does and does not keep stable).
    /// </summary>
    public IReadOnlyList<XkDeviceDescriptor> PresentDevices
    {
        get { lock (_sync) { return [.. _present.Values.Order(XkDeviceDescriptor.Order)]; } }
    }

    /// <summary>The devices the monitor opened whose sessions have not ended (a snapshot), in the order of their descriptors (<see cref="XkDeviceDescriptor.Order"/>).</summary>
    public IReadOnlyList<XkDevice> OpenDevices
    {
        get { lock (_sync) { return [.. _sessions.Values.Select(s => s.Device).Where(d => !d.IsClosed).OrderBy(d => d.Descriptor, XkDeviceDescriptor.Order)]; } }
    }

    /// <summary>For tests: replaces enumeration and the watch's generation, so that arrivals and removals can be simulated.</summary>
    internal Func<IReadOnlyList<XkDeviceDescriptor>>? EnumerateOverride { get; set; }

    internal Func<ulong>? GenerationOverride { get; set; }

    /// <summary>
    /// Start watching. Enumerates once on the calling thread (so <see cref="PresentDevices"/> is filled when this returns, and a failure
    /// throws here), then raises <see cref="DeviceArrived"/> for each present interface, and opens them, on the monitor's thread. Does not
    /// wait for any handler.
    /// </summary>
    /// <exception cref="XkHidException">The native library cannot be used, or the watch or the enumeration failed.</exception>
    /// <exception cref="InvalidOperationException">The monitor was already started.</exception>
    /// <exception cref="ObjectDisposedException">The monitor has been disposed.</exception>
    public void Start()
    {
        lock (_sync)
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            if (_started)
            {
                throw new InvalidOperationException("The monitor has already been started.");
            }
            _started = true;
        }
        try
        {
            XkHid.Require();
            // Watch first, then enumerate, so that nothing arriving in between is missed.
            _watch = XkWatch.Create(_options.Vid);
            List<XkDeviceDescriptor> initial = Enumerate();
            lock (_sync)
            {
                foreach (XkDeviceDescriptor descriptor in initial)
                {
                    _present[descriptor.Path] = descriptor;
                }
                _initial = initial;
            }
            _thread = new Thread(Loop) { IsBackground = true, Name = "XKeys device monitor" };
            _thread.Start();
        }
        catch
        {
            _watch?.Dispose();
            _watch = null;
            lock (_sync)
            {
                _present.Clear();
                _started = false;
            }
            throw;
        }
    }

    /// <summary>Enumerate again now, as if a notification had fired (for example after an <see cref="OpenFailed"/> that you know has cleared). Returns at once.</summary>
    public void Rescan()
    {
        lock (_sync)
        {
            if (_started && !_disposed)
            {
                _ = _rescan.Set();
            }
        }
    }

    /// <summary>
    /// Stop watching and dispose every device the monitor opened (each raises <see cref="XkDevice.Closed"/>). Safe to call more than once
    /// and from any thread; from another thread it returns when the monitor's thread has finished, from inside a handler of the monitor
    /// or of a device it opened at once (the rest happens on the monitor's thread).
    /// </summary>
    public void Dispose()
    {
        Thread? thread;
        bool onDeviceThread;
        lock (_sync)
        {
            // From a handler of a device this monitor opened, waiting would deadlock: the monitor's thread disposes that device, which
            // waits for its own thread.
            onDeviceThread = _sessions.Values.Any(s => s.Device.IsEventThread);
            if (_disposed)
            {
                thread = _thread;
            }
            else
            {
                _disposed = true;
                _stopping = true;
                thread = _thread;
                _ = _stop.Set();
            }
        }
        if (thread is null)
        {
            ReleaseAll();
            return;
        }
        if (thread != Thread.CurrentThread && !onDeviceThread)
        {
            thread.Join();
        }
    }

    private void Loop()
    {
        try
        {
            List<XkDeviceDescriptor> initial;
            lock (_sync)
            {
                initial = _initial;
                _initial = [];
            }
            foreach (XkDeviceDescriptor descriptor in initial)
            {
                if (_stopping)
                {
                    return;
                }
                Raise(DeviceArrived, new XkDeviceEventArgs(descriptor, wasPresentAtStart: true), nameof(DeviceArrived));
            }
            if (_options.OpenDevices)
            {
                ulong generation = Generation();
                foreach (XkDeviceDescriptor descriptor in initial)
                {
                    if (_stopping)
                    {
                        return;
                    }
                    OpenIfNeeded(descriptor, generation);
                }
            }
            WaitHandle[] handles = [_stop, _watch!.WaitHandle, _rescan];
            while (!_stopping && WaitHandle.WaitAny(handles) != 0)
            {
                Reconcile();
            }
        }
        catch (Exception ex)
        {
            // Not a handler (those are caught one by one). Report it rather than let the process die on a background thread.
            ReportFailure(EnumerationFailed, "monitor", ex);
        }
        finally
        {
            ReleaseAll();
        }
    }

    /// <summary>Enumerate, raise what left and what arrived, end sessions whose interface left, and open what needs opening.</summary>
    private void Reconcile()
    {
        // Read the generation before enumerating: a change during the enumeration then makes the next pass see a newer one.
        ulong generation = Generation();
        List<XkDeviceDescriptor> current;
        try
        {
            current = Enumerate();
        }
        catch (Exception ex)
        {
            ReportFailure(EnumerationFailed, "enumerate", ex);
            return;
        }
        List<XkDeviceDescriptor> removed = [];
        List<XkDeviceDescriptor> added = [];
        List<XkDevice> toEnd = [];
        lock (_sync)
        {
            HashSet<string> currentPaths = new(current.Select(d => d.Path), StringComparer.OrdinalIgnoreCase);
            foreach ((string path, XkDeviceDescriptor descriptor) in _present)
            {
                if (!currentPaths.Contains(path))
                {
                    removed.Add(descriptor);
                }
            }
            foreach (XkDeviceDescriptor descriptor in current)
            {
                if (!_present.ContainsKey(descriptor.Path))
                {
                    added.Add(descriptor);
                }
            }
            removed.Sort(XkDeviceDescriptor.Order);
            foreach (XkDeviceDescriptor descriptor in removed)
            {
                _ = _present.Remove(descriptor.Path);
                _ = _consumerClosed.Remove(descriptor.Path);
                if (_sessions.Remove(descriptor.Path, out Session? session))
                {
                    toEnd.Add(session.Device);
                }
            }
            foreach (XkDeviceDescriptor descriptor in added)
            {
                _present[descriptor.Path] = descriptor;
            }
        }
        foreach (XkDevice device in toEnd)
        {
            // Normally the read has already failed and this does nothing; if not, end the session as removed.
            device.Close(XkCloseReason.Removed);
        }
        foreach (XkDeviceDescriptor descriptor in removed)
        {
            if (_stopping)
            {
                return;
            }
            Raise(DeviceRemoved, new XkDeviceEventArgs(descriptor, wasPresentAtStart: false), nameof(DeviceRemoved));
        }
        foreach (XkDeviceDescriptor descriptor in added)
        {
            if (_stopping)
            {
                return;
            }
            Raise(DeviceArrived, new XkDeviceEventArgs(descriptor, wasPresentAtStart: false), nameof(DeviceArrived));
        }
        if (_options.OpenDevices)
        {
            foreach (XkDeviceDescriptor descriptor in current)
            {
                if (_stopping)
                {
                    return;
                }
                OpenIfNeeded(descriptor, generation);
            }
        }
    }

    /// <summary>
    /// Open <paramref name="descriptor"/> unless it has a live session, or the consumer closed its session, or its session ended by itself
    /// with no arrival or removal since it was opened (so that a device whose read fails at once is not reopened in a loop).
    /// </summary>
    private void OpenIfNeeded(XkDeviceDescriptor descriptor, ulong generation)
    {
        if (descriptor.InputLength == 0)
        {
            return;
        }
        bool isReopen = false;
        lock (_sync)
        {
            if (_consumerClosed.Contains(descriptor.Path))
            {
                return;
            }
            if (_sessions.TryGetValue(descriptor.Path, out Session? existing))
            {
                if (!existing.Device.IsClosed || existing.OpenedAtGeneration >= generation)
                {
                    return;
                }
                _ = _sessions.Remove(descriptor.Path);
            }
            isReopen = _everOpened.Contains(descriptor.Path);
        }
        int result;
        XkDevice? device;
        try
        {
            result = XkDevice.TryOpen(descriptor, _options.RingCapacity, _options.OpenFlags, out device);
        }
        catch (XkHidException ex)
        {
            result = ex.Code;
            device = null;
        }
        if (result != XkError.Ok || device is null)
        {
            Raise(OpenFailed, new XkOpenFailedEventArgs(descriptor, result), nameof(OpenFailed));
            return;
        }
        device.ClosedHook = OnSessionClosed;
        lock (_sync)
        {
            _sessions[descriptor.Path] = new Session(device, generation);
            _ = _everOpened.Add(descriptor.Path);
        }
        Raise(DeviceOpened, new XkDeviceOpenedEventArgs(device, isReopen), nameof(DeviceOpened));
        if (device.DeliveryMode == XkDeliveryMode.None && !device.IsClosed && !_stopping)
        {
            try
            {
                _ = device.Start(_options.RequestStateOnStart);
            }
            catch (Exception ex) when (ex is ObjectDisposedException or InvalidOperationException)
            {
                // A consumer disposed or started it on another thread in the meantime.
            }
        }
    }

    /// <summary>A session the monitor opened ended (on the thread that ended it).</summary>
    private void OnSessionClosed(XkDevice device, XkCloseReason reason)
    {
        lock (_sync)
        {
            if (_stopping || _disposed)
            {
                return;
            }
            if (reason == XkCloseReason.Disposed)
            {
                // The monitor disposes only on removal (as Removed) or when stopping, so this was the consumer.
                if (_sessions.TryGetValue(device.Path, out Session? session) && session.Device == device)
                {
                    _ = _consumerClosed.Add(device.Path);
                }
                return;
            }
            // Removed or failed: look again, so that it is reopened if it is (still or again) present after a later notification.
            _ = _rescan.Set();
        }
    }

    private ulong Generation()
    {
        Func<ulong>? generation = GenerationOverride;
        return generation is not null ? generation() : _watch!.Generation;
    }

    private List<XkDeviceDescriptor> Enumerate()
    {
        Func<IReadOnlyList<XkDeviceDescriptor>>? enumerate = EnumerateOverride;
        IReadOnlyList<XkDeviceDescriptor> all = enumerate is not null ? enumerate() : XkEnumerator.Enumerate(_options.Vid, _options.UsagePage, _options.OutputLength);
        return [.. all.Where(d => (_options.IncludeInterfacesWithoutInput || d.InputLength > 0) && (_options.Filter?.Invoke(d) ?? true)).Order(XkDeviceDescriptor.Order)];
    }

    /// <summary>Dispose every session the monitor opened and the watch. Runs once, on the monitor's thread as it ends (or in Dispose if it never started).</summary>
    private void ReleaseAll()
    {
        List<XkDevice> devices;
        lock (_sync)
        {
            _stopping = true;
            devices = [.. _sessions.Values.Select(s => s.Device)];
            _sessions.Clear();
        }
        foreach (XkDevice device in devices)
        {
            device.Dispose();
        }
        _watch?.Dispose();
        _watch = null;
    }

    private void Raise<T>(EventHandler<T>? handler, T args, string eventName)
    {
        if (handler is null)
        {
            return;
        }
        foreach (EventHandler<T> single in Delegate.EnumerateInvocationList(handler))
        {
            try
            {
                single(this, args);
            }
            catch (Exception ex)
            {
                ReportFailure(HandlerFailed, eventName, ex);
            }
        }
    }

    private void ReportFailure(EventHandler<XkErrorEventArgs>? handler, string context, Exception exception)
    {
        if (handler is null)
        {
            Trace.TraceError($"X-keys device monitor: {context} failed: {exception}");
            return;
        }
        XkErrorEventArgs args = new(context, exception);
        foreach (EventHandler<XkErrorEventArgs> single in Delegate.EnumerateInvocationList(handler))
        {
            try
            {
                single(this, args);
            }
            catch (Exception ex)
            {
                Trace.TraceError($"X-keys device monitor: a failure handler threw: {ex}");
            }
        }
    }
}
