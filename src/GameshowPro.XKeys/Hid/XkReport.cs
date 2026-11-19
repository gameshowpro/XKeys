// Copyright (c) Barjonas LLC 2026.
using System.Diagnostics;

namespace GameshowPro.XKeys.Hid;

/// <summary>What an X-keys input report is, from its data-type byte (index 2).</summary>
public enum XkReportKind
{
    /// <summary>Too short to hold the data type and the four input bytes.</summary>
    Unusable,
    /// <summary>Sent because an input changed: data type 0, or 1 on models with a program switch that is set.</summary>
    Input,
    /// <summary>The answer to Generate Data (command 177): data type 2, or 3 with the program switch set. Carries the current state.</summary>
    Generated,
    /// <summary>The answer to Get Descriptor (command 214); see <see cref="XkDescriptorReport"/>.</summary>
    Descriptor,
    /// <summary>Anything else (for example custom data, 224); carries no input state.</summary>
    Other,
}

/// <summary>The layout of X-keys input reports and the one request this library builds itself. Index 0 is the report ID.</summary>
public static class XkReport
{
    public const int UnitIdIndex = 1;
    public const int DataTypeIndex = 2;
    /// <summary>The first of the four bytes (indexes 3 to 6) that form the 32-bit input word: input n is bit n.</summary>
    public const int FirstInputIndex = 3;
    public const int InputByteCount = 4;
    public const int InputBitCount = InputByteCount * 8;
    public const byte DescriptorDataType = 214;
    /// <summary>Generate Data: the device answers with one input report of data type 2 (or 3) carrying its current state. Changes nothing on the device.</summary>
    public const byte GenerateDataCommand = 177;
    /// <summary>Get Descriptor: the device answers with a report of data type 214.</summary>
    public const byte GetDescriptorCommand = 214;
    private const byte GeneratedFlag = 0x02;
    private const byte HighestInputDataType = 3;

    /// <summary>What <paramref name="report"/> is, from its length and data type.</summary>
    public static XkReportKind Classify(ReadOnlySpan<byte> report)
    {
        if (report.Length < FirstInputIndex + InputByteCount)
        {
            return XkReportKind.Unusable;
        }
        byte dataType = report[DataTypeIndex];
        if (dataType == DescriptorDataType)
        {
            return XkReportKind.Descriptor;
        }
        if (dataType > HighestInputDataType)
        {
            return XkReportKind.Other;
        }
        return (dataType & GeneratedFlag) != 0 ? XkReportKind.Generated : XkReportKind.Input;
    }

    /// <summary>Report indexes 3 to 6 as one little-endian word, input n being bit n; 0 if the report is too short.</summary>
    public static uint InputWord(ReadOnlySpan<byte> report)
        => report.Length < FirstInputIndex + InputByteCount
            ? 0
            : report[FirstInputIndex]
                | ((uint)report[FirstInputIndex + 1] << 8)
                | ((uint)report[FirstInputIndex + 2] << 16)
                | ((uint)report[FirstInputIndex + 3] << 24);

    /// <summary>The Generate Data output report: <c>0, 177</c> and zeros, <paramref name="length"/> bytes (the device's output report length).</summary>
    public static byte[] GenerateDataRequest(int length = XkLightReports.DefaultReportLength)
    {
        ArgumentOutOfRangeException.ThrowIfLessThan(length, 2);
        byte[] report = new byte[length];
        report[1] = GenerateDataCommand;
        return report;
    }
}

/// <summary>
/// The answer to Get Descriptor (command 214), as far as it was measured on the XK-12 Jog &amp; Shuttle and XK-12 Switch Interface
/// (2026-11-16) and matches the vendor's page: index 1 unit ID, 3 PID mode, 11 firmware version, 12-13 product ID (low byte first).
/// </summary>
/// <param name="PidMode">Which of the model's product IDs the device is using (0 = its first).</param>
public sealed record XkDescriptorReport(byte UnitId, byte PidMode, byte FirmwareVersion, ushort Pid)
{
    /// <summary>The shortest report that holds every field.</summary>
    public const int MinimumLength = 14;

    /// <summary>Read a descriptor report; false if <paramref name="report"/> is not one or is too short.</summary>
    public static bool TryParse(ReadOnlySpan<byte> report, [System.Diagnostics.CodeAnalysis.NotNullWhen(true)] out XkDescriptorReport? descriptor)
    {
        if (report.Length < MinimumLength || report[XkReport.DataTypeIndex] != XkReport.DescriptorDataType)
        {
            descriptor = null;
            return false;
        }
        descriptor = new XkDescriptorReport(report[XkReport.UnitIdIndex], report[3], report[11], (ushort)(report[12] | (report[13] << 8)));
        return true;
    }
}

/// <summary>
/// When and in what order one input report arrived. Everything decoded from one report carries the same stamp: those changes arrived
/// together and cannot be ordered among themselves.
/// </summary>
/// <param name="Timestamp">
/// <c>QueryPerformanceCounter</c> ticks taken on the native read thread the moment the read completed: the clock of
/// <see cref="Stopwatch.GetTimestamp"/>, so it compares directly with stamps from other devices and with <see cref="Stopwatch"/> readings.
/// </param>
/// <param name="Sequence">The report's number in its session, from 0; a gap means reports were dropped. On one device, equal sequence numbers mean the same report.</param>
/// <param name="DeviceMilliseconds">The device's own millisecond count since it was plugged in, or null if the report has none. Good for ordering reports of one device over short spans; it runs 0.2-0.7 % slow against the host clock (measured), so it is not a clock and not comparable across devices.</param>
/// <param name="WasQueued">The report was already waiting in the HID driver's queue when the read was issued, so <paramref name="Timestamp"/> is late by an unknown amount.</param>
public readonly record struct XkReportStamp(long Timestamp, uint Sequence, uint? DeviceMilliseconds, bool WasQueued)
{
    /// <summary>The stamp of a report entry.</summary>
    public static XkReportStamp From(in XkReportEntry entry)
        => new(entry.Qpc, entry.Sequence, entry.HasDeviceMs ? entry.DeviceMs : null, entry.WasQueued);

    /// <summary><see cref="Timestamp"/> as a <see cref="TimeSpan"/> on the <see cref="Stopwatch"/> clock (time since its origin, usually boot).</summary>
    public TimeSpan ToTimeSpan() => Stopwatch.GetElapsedTime(0, Timestamp);

    /// <summary>How long ago the report arrived, by the <see cref="Stopwatch"/> clock.</summary>
    public TimeSpan GetElapsedTime() => Stopwatch.GetElapsedTime(Timestamp);

    /// <summary>The time from <paramref name="earlier"/> to <paramref name="later"/> by their <see cref="Timestamp"/>s (negative if <paramref name="later"/> is earlier).</summary>
    public static TimeSpan Between(XkReportStamp earlier, XkReportStamp later) => Stopwatch.GetElapsedTime(earlier.Timestamp, later.Timestamp);
}
