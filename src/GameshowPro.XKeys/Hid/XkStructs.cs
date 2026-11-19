// Copyright (c) Barjonas LLC 2026.
using System.Runtime.InteropServices;

namespace GameshowPro.XKeys.Hid;

/// <summary>Flag bits of <see cref="XkReportEntry.Flags"/>.</summary>
public static class XkReportFlags
{
    /// <summary><see cref="XkReportEntry.DeviceMs"/> holds the device's own timestamp.</summary>
    public const uint HasDeviceMs = 1u << 0;
    /// <summary>A synthetic entry that ends the session: <see cref="XkReportEntry.Error"/> says why and <see cref="XkReportEntry.Length"/> is 0.</summary>
    public const uint Error = 1u << 1;
    /// <summary>The report was already waiting in the HID class driver's queue when the read was issued, so its stamp is late by an unknown amount.</summary>
    public const uint Queued = 1u << 2;
}

/// <summary>Options for <see cref="XkDevice.TryOpen"/>.</summary>
[Flags]
public enum XkOpenFlags : uint
{
    None = 0,
    /// <summary>Discard a report byte-identical to the previous one, comparing every byte except the device timestamp.</summary>
    SuppressDuplicates = 1u << 0,
    /// <summary>Register the read thread with MMCSS.</summary>
    Mmcss = 1u << 1,
}

/// <summary>Result codes of the native library (<c>XKHID_E_*</c>).</summary>
public static class XkError
{
    public const int Ok = 0;
    public const int BadArgument = 1;
    public const int NotFound = 2;
    public const int OpenReadFailed = 3;
    public const int OpenWriteFailed = 4;
    public const int AlreadyOpen = 5;
    public const int DeviceRemoved = 6;
    public const int ReadFailed = 7;
    public const int NoInputReport = 8;
    public const int WriteLength = 9;
    public const int WriteTimeout = 10;
    public const int WriteFailed = 11;
    public const int BufferTooSmall = 12;
    public const int OutOfMemory = 13;
    public const int ReportTooLong = 14;
    public const int EnumerationFailed = 15;

    /// <summary>The name of a result code, for logs and exception messages.</summary>
    public static string Name(int code) => code switch
    {
        Ok => "OK",
        BadArgument => "BAD_ARGUMENT",
        NotFound => "NOT_FOUND",
        OpenReadFailed => "OPEN_READ_FAILED",
        OpenWriteFailed => "OPEN_WRITE_FAILED",
        AlreadyOpen => "ALREADY_OPEN",
        DeviceRemoved => "DEVICE_REMOVED",
        ReadFailed => "READ_FAILED",
        NoInputReport => "NO_INPUT_REPORT",
        WriteLength => "WRITE_LENGTH",
        WriteTimeout => "WRITE_TIMEOUT",
        WriteFailed => "WRITE_FAILED",
        BufferTooSmall => "BUFFER_TOO_SMALL",
        OutOfMemory => "OUT_OF_MEMORY",
        ReportTooLong => "REPORT_TOO_LONG",
        EnumerationFailed => "ENUMERATION_FAILED",
        _ => $"unknown code {code}",
    };
}

/// <summary>
/// One input report, or the end of a session, as <c>xkhid.dll</c> queued it. Mirrors <c>ReportEntry</c> in
/// <c>GameshowPro.XKeys.Native/src/report.rs</c> byte for byte (96 bytes); a change to either side must change both and
/// <see cref="XkHid.AbiVersion"/>.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
public unsafe struct XkReportEntry
{
    internal const int Size = 96;
    /// <summary>Capacity of <see cref="Data"/>.</summary>
    public const int MaxDataLength = 64;

    /// <summary><c>QueryPerformanceCounter</c> ticks taken the moment the overlapped read completed, on the native read thread: the clock of <see cref="System.Diagnostics.Stopwatch.GetTimestamp"/>.</summary>
    public long Qpc;
    /// <summary>The device's timestamp in milliseconds since plug-in, big-endian decoded; valid only if <see cref="XkReportFlags.HasDeviceMs"/> is set.</summary>
    public uint DeviceMs;
    /// <summary>Per-device counter incremented for every entry produced, including dropped ones, so a gap shows a drop.</summary>
    public uint Sequence;
    public uint Flags;
    /// <summary>0, or an <see cref="XkError"/> code when <see cref="XkReportFlags.Error"/> is set.</summary>
    public int Error;
    /// <summary>Valid bytes in <see cref="Data"/>: the device's input report length (report ID included).</summary>
    public ushort Length;
    internal ushort Reserved16;
    /// <summary>The Win32 error behind an error entry, else 0.</summary>
    public uint Win32Error;
    /// <summary>The report as read, index 0 being the report ID; bytes from <see cref="Length"/> on are zero.</summary>
    internal fixed byte Data[MaxDataLength];

    public readonly bool IsError => (Flags & XkReportFlags.Error) != 0;
    public readonly bool HasDeviceMs => (Flags & XkReportFlags.HasDeviceMs) != 0;
    public readonly bool WasQueued => (Flags & XkReportFlags.Queued) != 0;

    /// <summary>Copy the valid report bytes to <paramref name="destination"/>, as many as fit.</summary>
    /// <returns>The number of bytes copied.</returns>
    public readonly int CopyDataTo(Span<byte> destination)
    {
        int length = Math.Min(Math.Min((int)Length, MaxDataLength), destination.Length);
        fixed (byte* p = Data)
        {
            new ReadOnlySpan<byte>(p, length).CopyTo(destination);
        }
        return length;
    }
}

/// <summary>
/// One HID interface <c>xkhid_enumerate</c> found. Mirrors <c>DeviceInfo</c> in <c>GameshowPro.XKeys.Native/src/report.rs</c>
/// byte for byte (1296 bytes).
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct XkDeviceInfo
{
    public const int Size = 1296;
    public const int PathChars = 512;
    public const int StringChars = 64;

    /// <summary>The NUL-terminated device interface path, to pass to open.</summary>
    public fixed char Path[PathChars];
    public ushort Vid;
    public ushort Pid;
    public ushort Version;
    public ushort UsagePage;
    public ushort Usage;
    /// <summary>The input report length (report ID included); 0 if the interface has no input report.</summary>
    public ushort InputLen;
    /// <summary>The output report length (report ID included).</summary>
    public ushort OutputLen;
    public ushort FeatureLen;
    public fixed char Manufacturer[StringChars];
    public fixed char Product[StringChars];
}
