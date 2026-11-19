// Copyright (c) Barjonas LLC 2026.
namespace GameshowPro.XKeys.Hid;

/// <summary>How an <see cref="XkDevice"/> delivers its reports. A device uses one mode for its whole life, fixed by the first call that needs one.</summary>
public enum XkDeliveryMode
{
    /// <summary>Not decided yet: neither <see cref="XkDevice.Start"/> nor the pull API has been used.</summary>
    None,
    /// <summary>The consumer runs its own loop on <see cref="XkDevice.WaitHandle"/> or <see cref="XkDevice.WaitAsync"/> and <see cref="XkDevice.Read"/>.</summary>
    Pull,
    /// <summary>A library-owned thread raises the device's events (<see cref="XkDevice.Start"/>).</summary>
    Events,
}

/// <summary>Why a session ended (<see cref="XkDevice.Closed"/>).</summary>
public enum XkCloseReason
{
    /// <summary>The device went away: the read failed with <see cref="XkError.DeviceRemoved"/>, or the <see cref="XkDeviceMonitor"/> saw its interface leave.</summary>
    Removed,
    /// <summary>The read failed for another reason (<see cref="XkError.ReadFailed"/>); the Win32 error says why.</summary>
    ReadError,
    /// <summary><see cref="XkDevice.Dispose"/> was called (by the consumer, or by the <see cref="XkDeviceMonitor"/> that opened it as it stopped).</summary>
    Disposed,
}

/// <summary>One input report as the device sent it, with its stamp (<see cref="XkDevice.ReportReceived"/>).</summary>
public sealed class XkReportEventArgs : EventArgs
{
    private readonly byte[] _data;

    internal XkReportEventArgs(in XkReportEntry entry, XkReportKind kind, byte[] data)
    {
        Entry = entry;
        Stamp = XkReportStamp.From(entry);
        Kind = kind;
        _data = data;
    }

    /// <summary>The entry as the native library queued it.</summary>
    public XkReportEntry Entry { get; }

    public XkReportStamp Stamp { get; }

    public XkReportKind Kind { get; }

    /// <summary>The report bytes, index 0 being the report ID (a copy owned by these arguments).</summary>
    public ReadOnlySpan<byte> Data => _data;

    /// <summary>A new copy of <see cref="Data"/>.</summary>
    public byte[] ToArray() => (byte[])_data.Clone();
}

/// <summary>A session ended (<see cref="XkDevice.Closed"/>).</summary>
public sealed class XkClosedEventArgs : EventArgs
{
    internal XkClosedEventArgs(XkCloseReason reason, int error, uint win32Error, IReadOnlyList<int> pressedInputs)
    {
        Reason = reason;
        Error = error;
        Win32Error = win32Error;
        PressedInputs = pressedInputs;
    }

    public XkCloseReason Reason { get; }

    /// <summary>The <see cref="XkError"/> code of the failed read, or <see cref="XkError.Ok"/> when the session was disposed.</summary>
    public int Error { get; }

    /// <summary>The Win32 error behind a failed read, else 0.</summary>
    public uint Win32Error { get; }

    /// <summary>The inputs that were down when the session ended. No release will be reported for them; release them yourself if that matters.</summary>
    public IReadOnlyList<int> PressedInputs { get; }
}

/// <summary>An event handler threw, or the monitor could not enumerate. The exception was caught; nothing else was affected.</summary>
public sealed class XkErrorEventArgs : EventArgs
{
    internal XkErrorEventArgs(string context, Exception exception)
    {
        Context = context;
        Exception = exception;
    }

    /// <summary>The event whose handler threw (for example "InputsChanged"), or what failed (for example "enumerate").</summary>
    public string Context { get; }

    public Exception Exception { get; }
}

/// <summary>An interface arrived or left (<see cref="XkDeviceMonitor.DeviceArrived"/>, <see cref="XkDeviceMonitor.DeviceRemoved"/>).</summary>
public sealed class XkDeviceEventArgs : EventArgs
{
    internal XkDeviceEventArgs(XkDeviceDescriptor descriptor, bool wasPresentAtStart)
    {
        Descriptor = descriptor;
        WasPresentAtStart = wasPresentAtStart;
    }

    public XkDeviceDescriptor Descriptor { get; }

    /// <summary>The interface was already there when the monitor started (an arrival from the initial set, not a plug-in).</summary>
    public bool WasPresentAtStart { get; }

    /// <summary>The catalog entry for the interface's product ID.</summary>
    public XkModel Model => XkModel.ForPid(Descriptor.Pid);
}

/// <summary>The monitor opened a device for you (<see cref="XkDeviceMonitor.DeviceOpened"/>).</summary>
public sealed class XkDeviceOpenedEventArgs : EventArgs
{
    internal XkDeviceOpenedEventArgs(XkDevice device, bool isReopen)
    {
        Device = device;
        IsReopen = isReopen;
    }

    /// <summary>The open session, not yet started. Subscribe to its events in the handler; the monitor starts it when the handler returns.</summary>
    public XkDevice Device { get; }

    /// <summary>The same interface had a session before that ended (for example it was unplugged and plugged in again).</summary>
    public bool IsReopen { get; }
}

/// <summary>The monitor could not open an interface (<see cref="XkDeviceMonitor.OpenFailed"/>). It tries again at the next arrival or removal, or <see cref="XkDeviceMonitor.Rescan"/>.</summary>
public sealed class XkOpenFailedEventArgs : EventArgs
{
    internal XkOpenFailedEventArgs(XkDeviceDescriptor descriptor, int error)
    {
        Descriptor = descriptor;
        Error = error;
    }

    public XkDeviceDescriptor Descriptor { get; }

    /// <summary>The <see cref="XkError"/> code from <see cref="XkDevice.TryOpen"/>.</summary>
    public int Error { get; }
}
