// Copyright (c) Barjonas LLC 2026.
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using GameshowPro.XKeys.Hid;
using Microsoft.Extensions.Logging.Abstractions;

namespace GameshowPro.XKeys.Tests;

/// <summary>The binding to xkhid.dll: layouts, ABI check and what happens when the DLL is missing or wrong. No device needed.</summary>
[TestClass]
public sealed unsafe class BindingTests
{
    // Layout of the C# mirrors, against the layout asserted by the crate's unit tests (report.rs) and the sizes the DLL reports.

    [TestMethod]
    public void ReportEntryLayoutMatchesTheCrate()
    {
        XkReportEntry e = default;
        byte* b = (byte*)&e;
        Assert.AreEqual(96, sizeof(XkReportEntry));
        Assert.AreEqual(0, (byte*)&e.Qpc - b);
        Assert.AreEqual(8, (byte*)&e.DeviceMs - b);
        Assert.AreEqual(12, (byte*)&e.Sequence - b);
        Assert.AreEqual(16, (byte*)&e.Flags - b);
        Assert.AreEqual(20, (byte*)&e.Error - b);
        Assert.AreEqual(24, (byte*)&e.Length - b);
        Assert.AreEqual(26, (byte*)&e.Reserved16 - b);
        Assert.AreEqual(28, (byte*)&e.Win32Error - b);
        Assert.AreEqual(32, e.Data - b);
    }

    [TestMethod]
    public void DeviceInfoLayoutMatchesTheCrate()
    {
        XkDeviceInfo i = default;
        byte* b = (byte*)&i;
        Assert.AreEqual(1296, sizeof(XkDeviceInfo));
        Assert.AreEqual(0, (byte*)i.Path - b);
        Assert.AreEqual(1024, (byte*)&i.Vid - b);
        Assert.AreEqual(1026, (byte*)&i.Pid - b);
        Assert.AreEqual(1028, (byte*)&i.Version - b);
        Assert.AreEqual(1030, (byte*)&i.UsagePage - b);
        Assert.AreEqual(1032, (byte*)&i.Usage - b);
        Assert.AreEqual(1034, (byte*)&i.InputLen - b);
        Assert.AreEqual(1036, (byte*)&i.OutputLen - b);
        Assert.AreEqual(1038, (byte*)&i.FeatureLen - b);
        Assert.AreEqual(1040, (byte*)i.Manufacturer - b);
        Assert.AreEqual(1168, (byte*)i.Product - b);
    }

    [TestMethod]
    public void TypesAreBlittable()
    {
        // With DisableRuntimeMarshalling only blittable types may cross the boundary; a struct that is not blittable cannot be pinned.
        Assert.IsTrue(RuntimeHelpers.IsReferenceOrContainsReferences<XkReportEntry>() == false);
        Assert.IsTrue(RuntimeHelpers.IsReferenceOrContainsReferences<XkDeviceInfo>() == false);
        Assert.IsTrue(typeof(XkHid).Assembly.GetCustomAttributes(typeof(DisableRuntimeMarshallingAttribute), false).Length == 1);
    }

    [TestMethod]
    public void DllReportsTheSizesAndVersionTheBindingWasWrittenFor()
    {
        TestSupport.RequireDll();
        Assert.AreEqual(XkHid.AbiVersion, XkHid.AbiVersionExport());
        Assert.AreEqual((uint)sizeof(XkReportEntry), XkHid.EntrySizeExport());
        Assert.AreEqual((uint)sizeof(XkDeviceInfo), XkHid.InfoSizeExport());
        Assert.IsNull(XkHid.CheckVersions(XkHid.AbiVersionExport(), XkHid.EntrySizeExport(), XkHid.InfoSizeExport()));
    }

    [TestMethod]
    public void DllHasEveryExportTheBindingImports()
    {
        TestSupport.RequireDll();
        nint library = XkHid.TryLoadFrom(XkHid.DefaultSearchDirectories(), out string? reason);
        Assert.AreNotEqual(0, library, reason);
        NativeLibrary.Free(library);
    }

    [TestMethod]
    public void DllKnowsTheTimestampOffsetsMeasuredOnHardware()
    {
        TestSupport.RequireDll();
        Assert.AreEqual(9, XkHid.TimestampOffset(0x0426));
        Assert.AreEqual(32, XkHid.TimestampOffset(0x04AB));
        Assert.AreEqual(-1, XkHid.TimestampOffset(0x0001));
    }

    [TestMethod]
    public void AWatchCanBeMadeAndDestroyed()
    {
        TestSupport.RequireDll();
        using XkWatch watch = XkWatch.Create();
        Assert.IsFalse(watch.WaitHandle.WaitOne(0), "nothing has arrived or left");
        _ = watch.Generation;
    }

    [TestMethod]
    public void EnumeratingNeedsNoDeviceAndFindsNothingForAnotherVendor()
    {
        TestSupport.RequireDll();
        Assert.AreEqual(0, XkEnumerator.Enumerate(vid: 0xFFFF).Count);
    }

    // Version and size mismatches.

    [TestMethod]
    public void AMismatchingAbiVersionIsRefusedWithAMessageNamingTheDll()
    {
        string? reason = XkHid.CheckVersions(XkHid.AbiVersion + 1, 96, 1296);
        Assert.IsNotNull(reason);
        StringAssert.Contains(reason, "xkhid.dll");
        StringAssert.Contains(reason, "ABI version");
    }

    [TestMethod]
    public void AMismatchingEntrySizeIsRefused()
    {
        string? reason = XkHid.CheckVersions(XkHid.AbiVersion, 128, 1296);
        Assert.IsNotNull(reason);
        StringAssert.Contains(reason, "xkhid.dll");
        StringAssert.Contains(reason, "128");
    }

    [TestMethod]
    public void AMismatchingInfoSizeIsRefused()
    {
        string? reason = XkHid.CheckVersions(XkHid.AbiVersion, 96, 1300);
        Assert.IsNotNull(reason);
        StringAssert.Contains(reason, "1300");
    }

    // The DLL missing or unusable. These never reach the process-wide state set up by a successful load.

    [TestMethod]
    public void AMissingDllIsReportedWithItsNameAndWhereItWasLookedFor()
    {
        string empty = NewTempDirectory();
        try
        {
            string? reason = XkHid.Load([empty]);
            Assert.IsNotNull(reason);
            StringAssert.Contains(reason, "xkhid.dll");
            StringAssert.Contains(reason, "was not found");
            StringAssert.Contains(reason, empty);
        }
        finally
        {
            Directory.Delete(empty, true);
        }
    }

    [TestMethod]
    public void NoDirectoriesAtAllIsAlsoReported()
    {
        nint library = XkHid.TryLoadFrom([], out string? reason);
        Assert.AreEqual(0, library);
        StringAssert.Contains(reason, "xkhid.dll");
    }

    [TestMethod]
    public void AFileThatIsNotALibraryIsReported()
    {
        string dir = NewTempDirectory();
        try
        {
            File.WriteAllText(Path.Combine(dir, "xkhid.dll"), "this is not a PE file");
            nint library = XkHid.TryLoadFrom([dir], out string? reason);
            Assert.AreEqual(0, library);
            StringAssert.Contains(reason, "xkhid.dll");
            StringAssert.Contains(reason, "could not be loaded");
        }
        finally
        {
            Directory.Delete(dir, true);
        }
    }

    [TestMethod]
    public void ALibraryWithoutTheExportsIsReported()
    {
        string dir = NewTempDirectory();
        try
        {
            // Any real DLL that is not ours: loaded under our name from a temporary copy.
            File.Copy(Path.Combine(Environment.SystemDirectory, "version.dll"), Path.Combine(dir, "xkhid.dll"));
            nint library = XkHid.TryLoadFrom([dir], out string? reason);
            Assert.AreEqual(0, library);
            StringAssert.Contains(reason, "has no export xkhid_abi_version");
        }
        finally
        {
            Directory.Delete(dir, true);
        }
    }

    [TestMethod]
    public void ARealDllLaterInTheListIsFoundAfterAnEmptyDirectory()
    {
        TestSupport.RequireDll();
        string empty = NewTempDirectory();
        try
        {
            nint library = XkHid.TryLoadFrom([empty, .. XkHid.DefaultSearchDirectories()], out string? reason);
            Assert.AreNotEqual(0, library, reason);
            NativeLibrary.Free(library);
        }
        finally
        {
            Directory.Delete(empty, true);
        }
    }

    [TestMethod]
    public void TheExceptionForAnUnavailableLibraryCarriesTheReason()
    {
        XkHidException ex = new("xkhid.dll was not found (looked in somewhere)");
        StringAssert.Contains(ex.Message, "xkhid.dll");
        Assert.AreEqual(-1, ex.Code);
        XkHidException native = new("open", XkError.NotFound);
        StringAssert.Contains(native.Message, "NOT_FOUND");
        StringAssert.Contains(native.Message, "xkhid.dll");
    }

    /// <summary>
    /// When the DLL really is absent (a build with XkHidSkip, or without cargo), the library does not throw at start-up: devices come
    /// back disconnected, with the reason as the state's detail. Inconclusive when the DLL is present.
    /// </summary>
    [TestMethod]
    public void WithoutTheDllDevicesAreDisconnectedAndSayWhy()
    {
        if (XkHid.TryLoad(out _))
        {
            Assert.Inconclusive("xkhid.dll is present in this run; this test is for a build without it.");
        }
        XkHid.TryLoad(out string? reason);
        Assert.Throws<XkHidException>(XkHid.Require);
        var devices = XKeysDevices<TestKey>.GetDevices(1, new XKeysDeviceSettings(), NullLoggerFactory.Instance);
        Assert.AreEqual(1, devices.Count);
        Assert.IsFalse(devices[0].IsConnected);
        Assert.AreEqual(reason, devices[0].ServiceState.Detail);
    }

    private static string NewTempDirectory()
    {
        string dir = Path.Combine(Path.GetTempPath(), "xkeys-tests-" + Guid.NewGuid().ToString("N"));
        _ = Directory.CreateDirectory(dir);
        return dir;
    }

    [TestMethod]
    public async Task AWatchWaitCanBeCanceled()
    {
        TestSupport.RequireDll();
        using XkWatch watch = XkWatch.Create();
        using CancellationTokenSource cts = new(100);
        Assert.IsFalse(await watch.WaitAsync(cts.Token), "canceled, not signaled");
    }

    [TestMethod]
    public void ThePublicLibraryClassAgreesWithTheLoader()
    {
        TestSupport.RequireDll();
        Assert.IsTrue(XkHidLibrary.TryLoad(out string? reason), reason);
        XkHidLibrary.Require();
        Assert.AreEqual(9, XkHidLibrary.TimestampOffsetForPid(0x0426));
        Assert.IsTrue(XkHidLibrary.FileName.EndsWith(".dll"));
    }
}
