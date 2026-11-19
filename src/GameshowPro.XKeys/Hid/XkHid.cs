// Copyright (c) Barjonas LLC 2026.
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.Marshalling;
using System.Runtime.Versioning;
using Microsoft.Win32.SafeHandles;

// No run-time marshalling stubs: every type that crosses the boundary is blittable, so the binding is trimming- and AOT-safe.
[assembly: DisableRuntimeMarshalling]
// xkhid.dll is built for Windows only.
[assembly: SupportedOSPlatform("windows")]

namespace GameshowPro.XKeys.Hid;

/// <summary>
/// The raw exports of <c>xkhid.dll</c> (crate <c>GameshowPro.XKeys.Native</c>), as source-generated P/Invoke. Nothing here may be
/// called before <see cref="TryLoad"/> or <see cref="Require"/> has succeeded: they load the DLL, check that it matches this
/// binding, and point the runtime's resolver at it. Use <see cref="XkDevice"/>, <see cref="XkWatch"/> and <see cref="XkEnumerator"/>
/// rather than these.
/// </summary>
internal static unsafe partial class XkHid
{
    /// <summary>The name the exports are imported from; the loader resolves it to <see cref="LibraryFileName"/>.</summary>
    public const string LibraryName = "xkhid";
    public const string LibraryFileName = "xkhid.dll";
    /// <summary>The <c>ABI_VERSION</c> this binding was written for. A DLL reporting another version is not used.</summary>
    public const uint AbiVersion = 1;

    [LibraryImport(LibraryName, EntryPoint = "xkhid_abi_version")]
    internal static partial uint AbiVersionExport();

    [LibraryImport(LibraryName, EntryPoint = "xkhid_entry_size")]
    internal static partial uint EntrySizeExport();

    [LibraryImport(LibraryName, EntryPoint = "xkhid_info_size")]
    internal static partial uint InfoSizeExport();

    [LibraryImport(LibraryName, EntryPoint = "xkhid_timestamp_offset")]
    internal static partial int TimestampOffset(ushort pid);

    [LibraryImport(LibraryName, EntryPoint = "xkhid_enumerate")]
    internal static partial int Enumerate(ushort vid, ushort usagePage, ushort outputLen, XkDeviceInfo* output, uint max, out uint count);

    [LibraryImport(LibraryName, EntryPoint = "xkhid_open")]
    internal static partial int Open(char* path, uint ringCapacity, uint flags, int timestampOffset, out XkDeviceHandle device);

    [LibraryImport(LibraryName, EntryPoint = "xkhid_close")]
    internal static partial void Close(nint device);

    [LibraryImport(LibraryName, EntryPoint = "xkhid_event_handle")]
    internal static partial nint EventHandle(XkDeviceHandle device);

    [LibraryImport(LibraryName, EntryPoint = "xkhid_read")]
    internal static partial int Read(XkDeviceHandle device, XkReportEntry* entries, int max);

    [LibraryImport(LibraryName, EntryPoint = "xkhid_dropped")]
    internal static partial ulong Dropped(XkDeviceHandle device);

    [LibraryImport(LibraryName, EntryPoint = "xkhid_write")]
    internal static partial int Write(XkDeviceHandle device, byte* data, uint len, uint timeoutMs, out uint win32Error);

    [LibraryImport(LibraryName, EntryPoint = "xkhid_input_len")]
    internal static partial int InputLen(XkDeviceHandle device);

    [LibraryImport(LibraryName, EntryPoint = "xkhid_output_len")]
    internal static partial int OutputLen(XkDeviceHandle device);

    [LibraryImport(LibraryName, EntryPoint = "xkhid_watch_create")]
    internal static partial XkWatchHandle WatchCreate(ushort vid);

    [LibraryImport(LibraryName, EntryPoint = "xkhid_watch_event_handle")]
    internal static partial nint WatchEventHandle(XkWatchHandle watch);

    [LibraryImport(LibraryName, EntryPoint = "xkhid_watch_generation")]
    internal static partial ulong WatchGeneration(XkWatchHandle watch);

    [LibraryImport(LibraryName, EntryPoint = "xkhid_watch_destroy")]
    internal static partial void WatchDestroy(nint watch);
}

/// <summary>An open device session (<c>Device*</c>). Closing cancels the read, joins the native thread and frees the ring.</summary>
internal sealed class XkDeviceHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    public XkDeviceHandle() : base(ownsHandle: true) { }

    protected override bool ReleaseHandle()
    {
        XkHid.Close(handle);
        return true;
    }
}

/// <summary>A device arrival/removal watch (<c>Watch*</c>).</summary>
internal sealed class XkWatchHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    public XkWatchHandle() : base(ownsHandle: true) { }

    protected override bool ReleaseHandle()
    {
        XkHid.WatchDestroy(handle);
        return true;
    }
}

/// <summary>A wait handle that owns, and closes when disposed, a duplicate event handle from the native library.</summary>
internal sealed class XkOwnedEvent : WaitHandle
{
    public XkOwnedEvent(nint handle) => SafeWaitHandle = new SafeWaitHandle(handle, ownsHandle: true);
}

/// <summary>The native library cannot be used: it is missing, does not match this binding, or a call failed.</summary>
public sealed class XkHidException : Exception
{
    public XkHidException(string message) : base(message) { }

    internal XkHidException(string operation, int code, uint win32Error = 0)
        : base($"{XkHid.LibraryFileName}: {operation} failed with {XkError.Name(code)} ({code}){(win32Error != 0 ? $", Win32 error {win32Error}" : string.Empty)}")
    {
        Code = code;
        Win32Error = win32Error;
    }

    /// <summary>The <see cref="XkError"/> code, or -1 when the failure was not a native result.</summary>
    public int Code { get; } = -1;
    public uint Win32Error { get; }
}

/// <summary>
/// The native library <c>xkhid.dll</c> as a whole: whether it can be used, and what it knows. It is loaded from the directory of this
/// assembly or its <c>runtimes/win-x64/native</c> folder (where NuGet puts the package's native asset), never from elsewhere on the
/// search path, and checked against this binding (every export, ABI version, structure sizes) before anything is called.
/// </summary>
public static class XkHidLibrary
{
    public const string FileName = XkHid.LibraryFileName;

    /// <summary>The ABI version this binding was written for; a DLL that reports another is refused.</summary>
    public const uint AbiVersion = XkHid.AbiVersion;

    /// <summary>
    /// Load the DLL if needed and check it. Returns false, with a reason that names the DLL, when it is missing or does not match.
    /// The answer is the same for the rest of the process.
    /// </summary>
    public static bool TryLoad([System.Diagnostics.CodeAnalysis.NotNullWhen(false)] out string? unavailableReason)
        => XkHid.TryLoad(out unavailableReason);

    /// <summary>As <see cref="TryLoad"/>, throwing an <see cref="XkHidException"/> that names the DLL when it cannot be used (fail fast).</summary>
    public static void Require() => XkHid.Require();

    /// <summary>Where the 4-byte big-endian device timestamp sits in the input report of the model with this PID, or -1 if unknown.</summary>
    public static int TimestampOffsetForPid(ushort pid)
    {
        XkHid.Require();
        return XkHid.TimestampOffset(pid);
    }
}

/// <summary>Awaitable form of waiting on one of the native auto-reset events.</summary>
internal static class XkWait
{
    public static Task<bool> WaitAsync(WaitHandle handle, CancellationToken cancellationToken)
    {
        if (cancellationToken.IsCancellationRequested)
        {
            return Task.FromResult(false);
        }
        if (handle.WaitOne(0))
        {
            return Task.FromResult(true);
        }
        TaskCompletionSource<bool> completion = new(TaskCreationOptions.RunContinuationsAsynchronously);
        RegisteredWaitHandle? registered = null;
        CancellationTokenRegistration cancellation = default;
        registered = ThreadPool.RegisterWaitForSingleObject(
            handle,
            (_, timedOut) =>
            {
                cancellation.Dispose();
                completion.TrySetResult(!timedOut);
            },
            null,
            Timeout.Infinite,
            executeOnlyOnce: true);
        if (cancellationToken.CanBeCanceled)
        {
            cancellation = cancellationToken.Register(() =>
            {
                _ = registered.Unregister(null);
                completion.TrySetResult(false);
            });
        }
        return completion.Task;
    }
}
