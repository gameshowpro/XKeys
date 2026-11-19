// Copyright (c) Barjonas LLC 2026.
namespace GameshowPro.XKeys.Hid;

/// <summary>One HID interface found by <see cref="XkEnumerator"/>.</summary>
public sealed record XkDeviceDescriptor(
    string Path,
    ushort Vid,
    ushort Pid,
    ushort Version,
    ushort UsagePage,
    ushort Usage,
    ushort InputLength,
    ushort OutputLength,
    ushort FeatureLength,
    string Manufacturer,
    string Product)
{
    /// <summary>
    /// The library's device order (see <see cref="XkEnumerator.Enumerate"/>): by <see cref="Pid"/>, then by <see cref="Path"/> compared
    /// ordinally and case-insensitively. Null sorts first.
    /// </summary>
    public static IComparer<XkDeviceDescriptor> Order { get; } = Comparer<XkDeviceDescriptor>.Create(Compare);

    /// <summary>
    /// The key this interface sorts by, for display: the product ID as four hex digits, a space, and the path in upper case. Ordering these
    /// strings ordinally gives the same order as <see cref="Order"/>.
    /// </summary>
    public string SortKey => $"{Pid:X4} {Path.ToUpperInvariant()}";

    private static int Compare(XkDeviceDescriptor? a, XkDeviceDescriptor? b)
    {
        if (ReferenceEquals(a, b))
        {
            return 0;
        }
        if (a is null)
        {
            return -1;
        }
        if (b is null)
        {
            return 1;
        }
        int byPid = a.Pid.CompareTo(b.Pid);
        return byPid != 0 ? byPid : string.Compare(a.Path, b.Path, StringComparison.OrdinalIgnoreCase);
    }
}

/// <summary>Finds P.I. Engineering HID interfaces. Allocates only managed memory; the native call releases everything it touches before it returns.</summary>
public static unsafe class XkEnumerator
{
    public const ushort PiEngineeringVid = 0x05F3;
    /// <summary>The interface the library uses: the Consumer page (0x0C) interface, whose output report is 36 bytes.</summary>
    public const ushort ConsumerUsagePage = 0x0C;
    public const ushort ConsumerOutputLength = 36;

    /// <summary>
    /// The present interfaces of vendor <paramref name="vid"/> (0: P.I. Engineering) whose usage page and output report length match
    /// (0: any). Interfaces with no input report are included; their <see cref="XkDeviceDescriptor.InputLength"/> is 0.
    /// </summary>
    /// <remarks>
    /// <para>
    /// The list is in the library's device order, <see cref="XkDeviceDescriptor.Order"/>: by product ID, then by device interface path
    /// compared ordinally and case-insensitively. It does not depend on the order in which Windows enumerates interfaces or in which the
    /// devices were plugged in, and it needs nothing but the enumeration (no device is asked anything). Consumers that pick a device by
    /// index should use this order (<see cref="XkDeviceMonitor.PresentDevices"/> keeps the same one). <see cref="XkDeviceDescriptor.SortKey"/>
    /// shows the key.
    /// </para>
    /// <para>
    /// What is stable: the same set of devices on the same USB ports gives the same order after a restart, a re-enumeration or an unplug
    /// and replug, because the path of a USB device without a serial number embeds an instance ID that Windows derives from the port (hub and
    /// port number) it is plugged into. What is not: adding or removing a device shifts the index of every device that sorts after it;
    /// moving a device to another USB port (or hub) can change its position among devices with the same product ID; changing a device's PID
    /// mode changes its product ID and so its position. Whether Windows keeps the instance ID across a replug on the same port was not
    /// tested on hardware here (hot-plug was out of bounds); that is the documented behavior for devices without a serial number.
    /// </para>
    /// </remarks>
    /// <exception cref="XkHidException">The native library is unavailable or the enumeration failed.</exception>
    public static IReadOnlyList<XkDeviceDescriptor> Enumerate(ushort vid = 0, ushort usagePage = ConsumerUsagePage, ushort outputLength = ConsumerOutputLength)
    {
        XkHid.Require();
        XkDeviceInfo[] buffer = new XkDeviceInfo[8];
        while (true)
        {
            int result;
            uint count;
            fixed (XkDeviceInfo* p = buffer)
            {
                result = XkHid.Enumerate(vid, usagePage, outputLength, p, (uint)buffer.Length, out count);
            }
            if (result == XkError.BufferTooSmall)
            {
                buffer = new XkDeviceInfo[Math.Max(count, (uint)buffer.Length * 2)];
                continue;
            }
            if (result != XkError.Ok)
            {
                throw new XkHidException("enumerate", result);
            }
            List<XkDeviceDescriptor> list = new((int)count);
            for (int i = 0; i < count; i++)
            {
                list.Add(ToDescriptor(ref buffer[i]));
            }
            list.Sort(XkDeviceDescriptor.Order);
            return list;
        }
    }

    private static XkDeviceDescriptor ToDescriptor(ref XkDeviceInfo info)
    {
        fixed (XkDeviceInfo* p = &info)
        {
            return new XkDeviceDescriptor(
                FixedString(p->Path, XkDeviceInfo.PathChars),
                p->Vid, p->Pid, p->Version, p->UsagePage, p->Usage, p->InputLen, p->OutputLen, p->FeatureLen,
                FixedString(p->Manufacturer, XkDeviceInfo.StringChars),
                FixedString(p->Product, XkDeviceInfo.StringChars));
        }
    }

    private static string FixedString(char* chars, int capacity)
    {
        ReadOnlySpan<char> span = new(chars, capacity);
        int end = span.IndexOf('\0');
        return new string(end < 0 ? span : span[..end]);
    }
}

/// <summary>
/// An open device. The native library's read thread stamps each input report the moment it arrives (<see cref="XkReportEntry.Qpc"/>)
/// and queues it in a ring. A device delivers that ring in one of two ways, fixed for its life by the first call that needs one
/// (<see cref="DeliveryMode"/>):
/// <list type="bullet">
/// <item><b>Events</b> (nothing to poll): subscribe, then call <see cref="Start"/>. A library-owned thread raises <see cref="ReportReceived"/>,
/// <see cref="InputsChanged"/>, <see cref="JogMoved"/>, <see cref="ShuttleMoved"/> and finally <see cref="Closed"/>.</item>
/// <item><b>Pull</b>: the consumer's own loop waits on <see cref="WaitHandle"/> or <see cref="WaitAsync"/> and drains with <see cref="Read"/>.</item>
/// </list>
/// Using the pull API after <see cref="Start"/>, or calling <see cref="Start"/> after the pull API, throws <see cref="InvalidOperationException"/>.
/// </summary>
/// <remarks>
/// <para>
/// <b>Threading contract (events).</b> Every event of one device is raised on that device's own background thread (named
/// "XKeys events 0xPPPP"), one at a time and in report order, never two at once; different devices use different threads, so their
/// events may run concurrently. For each report the order is <see cref="ReportReceived"/>, then <see cref="InputsChanged"/>,
/// <see cref="JogMoved"/>, <see cref="ShuttleMoved"/> (each only if the report produced one). Handlers must be quick: while one runs, the
/// device's reports wait in the ring (256 entries by default; the time stamps are taken natively and are not delayed), and if the ring
/// overflows the device asks for its state again (Generate Data) and reports the difference as a resynchronization. A handler that
/// throws is caught: the exception is passed to <see cref="HandlerFailed"/> (on the same thread), the event's other handlers still run,
/// and the reader goes on; an exception from a <see cref="HandlerFailed"/> handler is written to <see cref="System.Diagnostics.Trace"/>
/// and dropped. <see cref="Closed"/> is raised exactly once, last, on the same thread, when the device is removed, its read fails or it
/// is disposed; after it the native session has been released and <see cref="Write"/> throws <see cref="ObjectDisposedException"/>.
/// <see cref="Dispose"/> may be called from any thread, including from inside a handler: from another thread it returns only after
/// <see cref="Closed"/> has been raised and no further event will come (so do not call it while holding a lock a handler needs); from
/// inside a handler it returns at once, the event being raised finishes for its other subscribers, and <see cref="Closed"/> follows.
/// </para>
/// <para>
/// <b>Pull.</b> <see cref="Read"/> must only be called from one thread at a time. No event is raised, <see cref="Closed"/> included: the
/// consumer sees the error entry that ends a session itself. Dispose only after the consumer loop has stopped.
/// </para>
/// <para>
/// In both modes <see cref="Write"/> calls may overlap each other and the events; a running call holds the handle, so dispose waits for it.
/// </para>
/// </remarks>
public sealed unsafe class XkDevice : IDisposable
{
    private const int ModeNone = (int)XkDeliveryMode.None;
    private const int ModePull = (int)XkDeliveryMode.Pull;
    private const int ModeEvents = (int)XkDeliveryMode.Events;
    private const int EventBatch = 32;

    private readonly XkDeviceHandle _handle;
    private readonly XkOwnedEvent _event;
    // Guards the mode and the close state below; never held while a handler runs or a thread is joined.
    private readonly Lock _lifecycle = new();
    private int _mode;
    private Thread? _eventThread;
    private ManualResetEvent? _stopEvents;
    private volatile bool _stopRequested;
    private bool _closeRequested;
    private bool _eventThreadDone;
    private XkCloseReason _requestedReason = XkCloseReason.Disposed;
    private XkCloseReason? _closeReason;
    private XkInputDecoder? _decoder;

    private XkDevice(XkDeviceHandle handle, XkOwnedEvent signal, XkDeviceDescriptor descriptor, int inputLength, int outputLength)
    {
        _handle = handle;
        _event = signal;
        Descriptor = descriptor;
        Path = descriptor.Path;
        Pid = descriptor.Pid;
        Model = XkModel.ForPid(descriptor.Pid);
        InputLength = inputLength;
        OutputLength = outputLength;
        Lights = new XkLights(this);
    }

    /// <summary>The interface this session was opened on.</summary>
    public string Path { get; }
    public ushort Pid { get; }
    /// <summary>The input report length, report ID included.</summary>
    public int InputLength { get; }
    /// <summary>The output report length, report ID included.</summary>
    public int OutputLength { get; }

    /// <summary>The enumeration result this session was opened from.</summary>
    public XkDeviceDescriptor Descriptor { get; }

    /// <summary>The catalog entry for <see cref="Pid"/> (a generic model if the PID is unknown); it decodes the events.</summary>
    public XkModel Model { get; }

    /// <summary>The typed lighting commands for this device (one instance per device); see <see cref="XkLights"/>. Raw reports remain available through <see cref="Write"/>.</summary>
    public XkLights Lights { get; }

    /// <summary>How this device delivers reports; fixed by the first call to <see cref="Start"/> or to the pull API.</summary>
    public XkDeliveryMode DeliveryMode => (XkDeliveryMode)Volatile.Read(ref _mode);

    /// <summary>Why the session ended, or null while it is open. Set before <see cref="Closed"/> is raised.</summary>
    public XkCloseReason? CloseReason
    {
        get { lock (_lifecycle) { return _closeReason; } }
    }

    /// <summary>The session has ended (see <see cref="CloseReason"/>).</summary>
    public bool IsClosed => CloseReason is not null;

    /// <summary>
    /// Every input report, raw and stamped, before it is decoded. Raised on the device's event thread; see the threading contract in the
    /// remarks of <see cref="XkDevice"/>.
    /// </summary>
    public event EventHandler<XkReportEventArgs>? ReportReceived;

    /// <summary>
    /// Inputs that one report changed, decoded with <see cref="Model"/>: one group per report, every change in it with the same stamp. Not
    /// raised for the initial state (the answer to the Generate Data request <see cref="Start"/> sends); see <see cref="XkInputDecoder"/>.
    /// </summary>
    public event EventHandler<XkInputGroup>? InputsChanged;

    /// <summary>A jog wheel click (models with a jog wheel).</summary>
    public event EventHandler<XkJogStep>? JogMoved;

    /// <summary>The shuttle ring moved (models with a shuttle).</summary>
    public event EventHandler<XkShuttleMove>? ShuttleMoved;

    /// <summary>The session ended: raised exactly once, last, by a started device. Not raised in pull mode or if <see cref="Start"/> was never called.</summary>
    public event EventHandler<XkClosedEventArgs>? Closed;

    /// <summary>A handler of one of this device's events threw. Raised on the device's event thread, right after the handler that threw.</summary>
    public event EventHandler<XkErrorEventArgs>? HandlerFailed;

    /// <summary>Whether the calling thread is this device's event thread (a handler is running).</summary>
    internal bool IsEventThread => Thread.CurrentThread == Volatile.Read(ref _eventThread);

    /// <summary>For <see cref="XkDeviceMonitor"/>: told after the session has ended, on the thread that ended it.</summary>
    internal Action<XkDevice, XkCloseReason>? ClosedHook { get; set; }

    /// <summary>
    /// Signaled (auto-reset) after every entry the read thread queues, including one it had to drop. Wraps a duplicate of the native
    /// event that this object owns and closes. Pull API: fixes <see cref="DeliveryMode"/> to <see cref="XkDeliveryMode.Pull"/>.
    /// </summary>
    /// <exception cref="InvalidOperationException">The device was started (<see cref="Start"/>).</exception>
    public WaitHandle WaitHandle
    {
        get
        {
            EnterPull();
            return _event;
        }
    }

    /// <summary>
    /// Completes when <see cref="WaitHandle"/> is signaled (the read thread queued something), or false when <paramref name="cancellationToken"/>
    /// is canceled. The signal is auto-reset, so one completion covers everything queued before it: drain with <see cref="Read"/> until it
    /// returns 0, then wait again. Use either this or the wait handle from the one consumer, not both at once. Pull API.
    /// </summary>
    /// <exception cref="InvalidOperationException">The device was started (<see cref="Start"/>).</exception>
    public Task<bool> WaitAsync(CancellationToken cancellationToken = default)
    {
        EnterPull();
        return XkWait.WaitAsync(_event, cancellationToken);
    }

    /// <summary>
    /// Open <paramref name="descriptor"/>'s interface. Returns the <see cref="XkError"/> code: <see cref="XkError.Ok"/> with
    /// <paramref name="device"/> set, or an error with it null.
    /// </summary>
    /// <param name="ringCapacity">Entries in the ring; 0 lets the library choose 256.</param>
    /// <param name="flags">Zero or more <see cref="XkOpenFlags"/>.</param>
    /// <param name="timestampOffset">Index of the 4-byte big-endian device timestamp in the input report; null uses the library's table for the PID, negative means none.</param>
    public static int TryOpen(XkDeviceDescriptor descriptor, uint ringCapacity, XkOpenFlags flags, out XkDevice? device, int? timestampOffset = null)
    {
        XkHid.Require();
        device = null;
        int result;
        XkDeviceHandle handle;
        fixed (char* path = descriptor.Path)
        {
            // A string is NUL-terminated in memory, which is what the library expects.
            result = XkHid.Open(path, ringCapacity, (uint)flags, timestampOffset ?? XkHid.TimestampOffset(descriptor.Pid), out handle);
        }
        if (result != XkError.Ok)
        {
            handle.Dispose();
            return result;
        }
        nint eventHandle = XkHid.EventHandle(handle);
        if (eventHandle == 0)
        {
            handle.Dispose();
            return XkError.OutOfMemory;
        }
        device = new XkDevice(handle, new XkOwnedEvent(eventHandle), descriptor, XkHid.InputLen(handle), XkHid.OutputLen(handle));
        return XkError.Ok;
    }

    /// <summary>As <see cref="TryOpen"/>, throwing an <see cref="XkHidException"/> on failure.</summary>
    public static XkDevice Open(XkDeviceDescriptor descriptor, uint ringCapacity = 0, XkOpenFlags flags = XkOpenFlags.None, int? timestampOffset = null)
    {
        int result = TryOpen(descriptor, ringCapacity, flags, out XkDevice? device, timestampOffset);
        return result == XkError.Ok ? device! : throw new XkHidException("open", result);
    }

    /// <summary>
    /// Start raising this device's events from a library-owned thread (see the threading contract in the remarks of <see cref="XkDevice"/>).
    /// Subscribe first: reports that arrive before a handler is attached are not raised to it. Fixes <see cref="DeliveryMode"/> to
    /// <see cref="XkDeliveryMode.Events"/>.
    /// </summary>
    /// <param name="requestState">
    /// Send Generate Data (command 177, which changes nothing on the device) once the thread runs, so that the device's current state
    /// becomes the baseline at once. Without a baseline the first report that arrives serves as one, and a press it carries is not reported.
    /// </param>
    /// <returns>The <see cref="XkError"/> code of the Generate Data write, or <see cref="XkError.Ok"/> if none was requested.</returns>
    /// <exception cref="InvalidOperationException">The device was already started, or the pull API has been used.</exception>
    /// <exception cref="ObjectDisposedException">The device has been disposed or its session has ended.</exception>
    public int Start(bool requestState = true)
    {
        lock (_lifecycle)
        {
            ObjectDisposedException.ThrowIf(_closeRequested, this);
            if (_mode != ModeNone)
            {
                throw new InvalidOperationException(_mode == ModeEvents
                    ? "The device has already been started."
                    : "The device is in pull mode (WaitHandle, WaitAsync or Read was used); a device uses one delivery mode for its life.");
            }
            _mode = ModeEvents;
            _decoder = new XkInputDecoder(Model);
            _stopEvents = new ManualResetEvent(false);
            _eventThread = new Thread(EventLoop) { IsBackground = true, Name = $"XKeys events 0x{Pid:X4}" };
            _eventThread.Start();
        }
        return requestState ? RequestState() : XkError.Ok;
    }

    /// <summary>
    /// Ask the device for its current state: Generate Data (command 177), which changes nothing on the device. The device answers with one
    /// report of kind <see cref="XkReportKind.Generated"/> (at most one per 29 ms, measured). Returns the <see cref="XkError"/> code of the
    /// write, <see cref="XkError.DeviceRemoved"/> if the session has ended.
    /// </summary>
    public int RequestState(int timeoutMilliseconds = XkLights.DefaultTimeoutMilliseconds)
    {
        if (OutputLength < 2)
        {
            return XkError.WriteLength;
        }
        Span<byte> report = stackalloc byte[OutputLength];
        report.Clear();
        report[1] = XkReport.GenerateDataCommand;
        try
        {
            return Write(report, timeoutMilliseconds, out _);
        }
        catch (ObjectDisposedException)
        {
            return XkError.DeviceRemoved;
        }
    }

    /// <summary>Move up to <paramref name="destination"/>.Length queued entries, oldest first, into <paramref name="destination"/>. One thread at a time. Pull API.</summary>
    /// <returns>The number of entries moved.</returns>
    /// <exception cref="InvalidOperationException">The device was started (<see cref="Start"/>).</exception>
    public int Read(Span<XkReportEntry> destination)
    {
        EnterPull();
        return ReadCore(destination);
    }

    /// <summary>Reports dropped because the ring was full, since it was opened.</summary>
    public ulong DroppedCount => XkHid.Dropped(_handle);

    /// <summary>
    /// Write one output report (<paramref name="report"/>.Length must equal <see cref="OutputLength"/>, report ID byte included) and
    /// wait up to <paramref name="timeoutMilliseconds"/> for it to complete.
    /// </summary>
    /// <returns>The <see cref="XkError"/> code; <paramref name="win32Error"/> is the Win32 error behind a failure, else 0.</returns>
    public int Write(ReadOnlySpan<byte> report, int timeoutMilliseconds, out uint win32Error)
    {
        if (report.IsEmpty)
        {
            win32Error = 0;
            return XkError.WriteLength;
        }
        fixed (byte* p = report)
        {
            return XkHid.Write(_handle, p, (uint)report.Length, (uint)timeoutMilliseconds, out win32Error);
        }
    }

    /// <summary>
    /// End the session: cancel the read, stop the native thread and free the ring. Safe to call more than once and from any thread. A
    /// started device raises <see cref="Closed"/> with <see cref="XkCloseReason.Disposed"/> (unless it had already ended); see the
    /// threading contract in the remarks of <see cref="XkDevice"/> for when this returns.
    /// </summary>
    public void Dispose() => Close(XkCloseReason.Disposed);

    /// <summary>End the session for <paramref name="reason"/> (the monitor ends one whose interface left with <see cref="XkCloseReason.Removed"/>).</summary>
    internal void Close(XkCloseReason reason)
    {
        Thread? thread;
        bool releaseHere = false;
        lock (_lifecycle)
        {
            if (!_closeRequested)
            {
                _closeRequested = true;
                _requestedReason = reason;
                if (_eventThread is null)
                {
                    _closeReason = reason;
                    releaseHere = true;
                }
                else if (!_eventThreadDone)
                {
                    _stopRequested = true;
                    _ = _stopEvents!.Set();
                }
            }
            thread = _eventThread;
        }
        if (releaseHere)
        {
            ReleaseNative();
            InvokeClosedHook(reason);
            return;
        }
        // The event thread raises Closed and releases the session on its way out. From inside a handler it cannot be waited for.
        if (thread is not null && thread != Thread.CurrentThread)
        {
            thread.Join();
        }
    }

    private void EnterPull()
    {
        if (Volatile.Read(ref _mode) == ModePull)
        {
            return;
        }
        lock (_lifecycle)
        {
            if (_mode == ModeEvents)
            {
                throw new InvalidOperationException("The device was started and raises events; WaitHandle, WaitAsync and Read are for a device in pull mode.");
            }
            _mode = ModePull;
        }
    }

    private int ReadCore(Span<XkReportEntry> destination)
    {
        if (destination.IsEmpty)
        {
            return 0;
        }
        fixed (XkReportEntry* p = destination)
        {
            return XkHid.Read(_handle, p, destination.Length);
        }
    }

    /// <summary>The device's event thread: sleeps until the native read thread queues something, then drains the ring and raises the events.</summary>
    private void EventLoop()
    {
        XkReportEntry[] entries = new XkReportEntry[EventBatch];
        WaitHandle[] handles = [_event, _stopEvents!];
        XkCloseReason? failure = null;
        int error = XkError.Ok;
        uint win32Error = 0;
        ulong dropped = 0;
        try
        {
            while (!_stopRequested && WaitHandle.WaitAny(handles) == 0)
            {
                int count;
                while (!_stopRequested && (count = ReadCore(entries)) > 0)
                {
                    for (int i = 0; i < count && !_stopRequested; i++)
                    {
                        if (entries[i].IsError)
                        {
                            // Always the last entry of a session.
                            failure = entries[i].Error == XkError.DeviceRemoved ? XkCloseReason.Removed : XkCloseReason.ReadError;
                            error = entries[i].Error;
                            win32Error = entries[i].Win32Error;
                            return;
                        }
                        Dispatch(in entries[i]);
                    }
                }
                ulong nowDropped = XkHid.Dropped(_handle);
                if (nowDropped != dropped && !_stopRequested)
                {
                    // Reports were lost while handlers were busy: ask for the state, and the decoder reports the difference as a resynchronization.
                    dropped = nowDropped;
                    _ = RequestState();
                }
            }
        }
        catch (Exception ex)
        {
            // Not a handler (those are caught one by one): the native calls or the wait failed. End the session rather than die silently.
            ReportHandlerFailure("reader", ex);
            failure ??= XkCloseReason.ReadError;
        }
        finally
        {
            FinishEvents(failure, error, win32Error);
        }
    }

    private void Dispatch(in XkReportEntry entry)
    {
        byte[] data = new byte[Math.Min((int)entry.Length, XkReportEntry.MaxDataLength)];
        _ = entry.CopyDataTo(data);
        XkReportStamp stamp = XkReportStamp.From(entry);
        XkDecodeResult result = _decoder!.Decode(data, stamp);
        if (ReportReceived is { } reportReceived)
        {
            Raise(reportReceived, new XkReportEventArgs(entry, result.Kind, data), nameof(ReportReceived));
        }
        if (result.Inputs is { } inputs && !_stopRequested)
        {
            Raise(InputsChanged, inputs, nameof(InputsChanged));
        }
        if (result.Jog is { } jog && !_stopRequested)
        {
            Raise(JogMoved, jog, nameof(JogMoved));
        }
        if (result.Shuttle is { } shuttle && !_stopRequested)
        {
            Raise(ShuttleMoved, shuttle, nameof(ShuttleMoved));
        }
    }

    private void FinishEvents(XkCloseReason? failure, int error, uint win32Error)
    {
        XkCloseReason reason;
        lock (_lifecycle)
        {
            _eventThreadDone = true;
            _closeRequested = true;
            reason = failure ?? _requestedReason;
            _closeReason = reason;
        }
        IReadOnlyList<int> held = _decoder?.Reset() ?? [];
        Raise(Closed, new XkClosedEventArgs(reason, failure is null ? XkError.Ok : error, win32Error, held), nameof(Closed));
        ReleaseNative();
        InvokeClosedHook(reason);
    }

    private void ReleaseNative()
    {
        _handle.Dispose();
        _event.Dispose();
        _stopEvents?.Dispose();
    }

    private void InvokeClosedHook(XkCloseReason reason)
    {
        try
        {
            ClosedHook?.Invoke(this, reason);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Trace.TraceError($"X-keys device 0x{Pid:X4}: the monitor's close notification failed: {ex}");
        }
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
                ReportHandlerFailure(eventName, ex);
            }
        }
    }

    private void ReportHandlerFailure(string context, Exception exception)
    {
        EventHandler<XkErrorEventArgs>? handler = HandlerFailed;
        if (handler is null)
        {
            System.Diagnostics.Trace.TraceError($"X-keys device 0x{Pid:X4}: a {context} handler threw: {exception}");
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
                System.Diagnostics.Trace.TraceError($"X-keys device 0x{Pid:X4}: a HandlerFailed handler threw: {ex}");
            }
        }
    }
}

/// <summary>
/// Reports, without a window and without polling, when P.I. Engineering HID interfaces arrive or leave. Signals
/// <see cref="WaitHandle"/> (auto-reset); <see cref="Generation"/> counts the arrivals and removals so far.
/// </summary>
public sealed class XkWatch : IDisposable
{
    private readonly XkWatchHandle _handle;
    private readonly XkOwnedEvent _event;

    private XkWatch(XkWatchHandle handle, XkOwnedEvent signal)
    {
        _handle = handle;
        _event = signal;
    }

    /// <summary>Start watching vendor <paramref name="vid"/> (0: P.I. Engineering).</summary>
    /// <exception cref="XkHidException">The native library is unavailable or could not start the watch.</exception>
    public static XkWatch Create(ushort vid = 0)
    {
        XkHid.Require();
        XkWatchHandle handle = XkHid.WatchCreate(vid);
        if (handle.IsInvalid)
        {
            handle.Dispose();
            throw new XkHidException($"{XkHid.LibraryFileName}: could not start the device watch (xkhid_watch_create returned null)");
        }
        nint eventHandle = XkHid.WatchEventHandle(handle);
        if (eventHandle == 0)
        {
            handle.Dispose();
            throw new XkHidException($"{XkHid.LibraryFileName}: could not duplicate the watch's event handle (xkhid_watch_event_handle returned null)");
        }
        return new XkWatch(handle, new XkOwnedEvent(eventHandle));
    }

    public WaitHandle WaitHandle => _event;

    /// <summary>Completes when the watch is signaled (an arrival or removal), or false when <paramref name="cancellationToken"/> is canceled.</summary>
    public Task<bool> WaitAsync(CancellationToken cancellationToken = default) => XkWait.WaitAsync(_event, cancellationToken);

    /// <summary>Arrivals plus removals counted since the watch was made.</summary>
    public ulong Generation => XkHid.WatchGeneration(_handle);

    public void Dispose()
    {
        _handle.Dispose();
        _event.Dispose();
    }
}
