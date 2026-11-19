// Copyright (c) Barjonas LLC 2026.
using GameshowPro.Common.Model;
using GameshowPro.XKeys.Hid;

// The library is Windows-only (xkhid.dll is Windows x64), and says so with [SupportedOSPlatform("windows")]. A consumer that targets plain
// net11.0, as these tests do in the Release configuration, tells the platform analyzer (CA1416) that it runs on Windows too.
[assembly: System.Runtime.Versioning.SupportedOSPlatform("windows")]

namespace GameshowPro.XKeys.Tests;

/// <summary>Two keys, so that <see cref="XKeysDevices{TTriggerKey}"/> can build real devices in tests.</summary>
internal enum TestKey
{
    [TriggerParameters("Key one")]
    One,
    [TriggerParameters("Key two")]
    Two,
}

internal static class TestSupport
{
    /// <summary>Inconclusive, with the reason, when <c>xkhid.dll</c> is not available (no cargo when the tests were built, or built with XkHidSkip).</summary>
    public static void RequireDll()
    {
        if (!XkHid.TryLoad(out string? reason))
        {
            Assert.Inconclusive($"The native library is not available: {reason}");
        }
    }

    /// <summary>
    /// The P.I. Engineering consumer-control interfaces that carry input reports. Inconclusive, not failed, when the DLL is missing or
    /// no device is plugged in (a build machine has none).
    /// </summary>
    public static IReadOnlyList<XkDeviceDescriptor> RequireDevices()
    {
        RequireDll();
        List<XkDeviceDescriptor> devices = [.. XkEnumerator.Enumerate().Where(d => d.InputLength > 0)];
        if (devices.Count == 0)
        {
            Assert.Inconclusive("No P.I. Engineering (X-keys) device is plugged in.");
        }
        return devices;
    }

    /// <summary>
    /// Hardware tests share the devices with whatever else is running (a service that opens them, another test run), which can add a
    /// report of its own. A failed assertion is therefore tried once more before it counts.
    /// </summary>
    public static void RetryOnce(Action test)
    {
        try
        {
            test();
        }
        catch (AssertFailedException)
        {
            Thread.Sleep(500);
            test();
        }
    }

    /// <summary>
    /// Generate Data (P.I. Engineering command 177): the device answers with one input report. It is the only command the hardware
    /// tests send; none of them changes an LED, the unit ID or any other setting.
    /// </summary>
    public static int GenerateData(XkDevice device)
    {
        Span<byte> report = stackalloc byte[device.OutputLength];
        report.Clear();
        report[1] = 177;
        return device.Write(report, 1000, out _);
    }

    public static List<XkReportEntry> ReadAll(XkDevice device)
    {
        List<XkReportEntry> all = [];
        XkReportEntry[] buffer = new XkReportEntry[64];
        int count;
        while ((count = device.Read(buffer)) > 0)
        {
            all.AddRange(buffer.AsSpan(0, count));
        }
        return all;
    }
}

[TestClass]
public static class AssemblySetup
{
    /// <summary>The library's devices build their change filters from a process-wide logger factory that the application normally assigns.</summary>
    [AssemblyInitialize]
    public static void Initialize(TestContext _) => PropertyChangeFilters.AssignLoggerFactory(Microsoft.Extensions.Logging.Abstractions.NullLoggerFactory.Instance);
}
