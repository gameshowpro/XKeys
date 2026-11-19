// Copyright (c) Barjonas LLC 2026.
using GameshowPro.XKeys.Hid;

namespace GameshowPro.XKeys.Tests;

/// <summary>The lighting report builders (exact bytes, no device) and the lights of attached devices.</summary>
[TestClass]
public sealed class LightsTests
{
    private static byte[] Expected(byte command, byte a = 0, byte b = 0, int length = 36)
    {
        byte[] report = new byte[length];
        report[1] = command;
        report[2] = a;
        report[3] = b;
        return report;
    }

    [TestMethod]
    public void SetLedBuildsCommand179WithIndexAndState()
    {
        CollectionAssert.AreEqual(Expected(179, 6, 1), XkLightReports.SetLed(XkLedIndex.Green, XkLedState.On));
        CollectionAssert.AreEqual(Expected(179, 6, 0), XkLightReports.SetLed(XkLedIndex.Green, XkLedState.Off));
        CollectionAssert.AreEqual(Expected(179, 7, 2), XkLightReports.SetLed(XkLedIndex.Red, XkLedState.Flash));
        CollectionAssert.AreEqual(Expected(179, 0, 1), XkLightReports.SetLed(XkLedIndex.Output1, XkLedState.On));
        CollectionAssert.AreEqual(Expected(179, 1, 2), XkLightReports.SetLed(XkLedIndex.Output2, XkLedState.Flash));
    }

    [TestMethod]
    public void SetIndicatorBitsBuildsCommand186WithGreen0x40AndRed0x80()
    {
        CollectionAssert.AreEqual(Expected(186, 0x00), XkLightReports.SetIndicatorBits(false, false));
        CollectionAssert.AreEqual(Expected(186, 0x40), XkLightReports.SetIndicatorBits(true, false));
        CollectionAssert.AreEqual(Expected(186, 0x80), XkLightReports.SetIndicatorBits(false, true));
        CollectionAssert.AreEqual(Expected(186, 0xC0), XkLightReports.SetIndicatorBits(true, true));
    }

    [TestMethod]
    public void SetFlashFrequencyBuildsCommand180AndRefusesZero()
    {
        CollectionAssert.AreEqual(Expected(180, 1), XkLightReports.SetFlashFrequency(1));
        CollectionAssert.AreEqual(Expected(180, 255), XkLightReports.SetFlashFrequency(255));
        _ = Assert.Throws<ArgumentOutOfRangeException>(() => XkLightReports.SetFlashFrequency(0));
    }

    [TestMethod]
    public void SetBacklightBuildsCommand181WithBank2Offset32()
    {
        CollectionAssert.AreEqual(Expected(181, 0, 1), XkLightReports.SetBacklight(0, 0, XkLedState.On));
        CollectionAssert.AreEqual(Expected(181, 26, 2), XkLightReports.SetBacklight(0, 26, XkLedState.Flash));
        CollectionAssert.AreEqual(Expected(181, 34, 0), XkLightReports.SetBacklight(1, 2, XkLedState.Off));
        CollectionAssert.AreEqual(Expected(181, 63, 1), XkLightReports.SetBacklight(1, 31, XkLedState.On));
        _ = Assert.Throws<ArgumentOutOfRangeException>(() => XkLightReports.SetBacklight(2, 0, XkLedState.On));
        _ = Assert.Throws<ArgumentOutOfRangeException>(() => XkLightReports.SetBacklight(0, 32, XkLedState.On));
        _ = Assert.Throws<ArgumentOutOfRangeException>(() => XkLightReports.SetBacklight(0, -1, XkLedState.On));
    }

    [TestMethod]
    public void SetBacklightRowsBuildsCommand182()
    {
        CollectionAssert.AreEqual(Expected(182, 0, 0b000111), XkLightReports.SetBacklightRows(0, 0b000111));
        CollectionAssert.AreEqual(Expected(182, 1, 0b100000), XkLightReports.SetBacklightRows(1, 0b100000));
        _ = Assert.Throws<ArgumentOutOfRangeException>(() => XkLightReports.SetBacklightRows(1, 0x40));
        _ = Assert.Throws<ArgumentOutOfRangeException>(() => XkLightReports.SetBacklightRows(-1, 1));
    }

    [TestMethod]
    public void ToggleAndIntensityBuildCommands184And187()
    {
        CollectionAssert.AreEqual(Expected(184), XkLightReports.ToggleBacklights());
        CollectionAssert.AreEqual(Expected(187, 255, 0), XkLightReports.SetBacklightIntensity(255, 0));
        CollectionAssert.AreEqual(Expected(187, 0, 128), XkLightReports.SetBacklightIntensity(0, 128));
    }

    [TestMethod]
    public void ReportsHaveTheRequestedLengthAndAZeroReportId()
    {
        byte[] report = XkLightReports.SetLed(XkLedIndex.Red, XkLedState.On, 36);
        Assert.AreEqual(36, report.Length);
        Assert.AreEqual(0, report[0]);
        Assert.IsTrue(report.Skip(4).All(b => b == 0));
        Assert.AreEqual(5, XkLightReports.ToggleBacklights(5).Length);
        _ = Assert.Throws<ArgumentOutOfRangeException>(() => XkLightReports.ToggleBacklights(3));
    }

    [TestMethod]
    public void InvalidEnumValuesAreRefused()
    {
        _ = Assert.Throws<ArgumentOutOfRangeException>(() => XkLightReports.SetLed((XkLedIndex)3, XkLedState.On));
        _ = Assert.Throws<ArgumentOutOfRangeException>(() => XkLightReports.SetLed(XkLedIndex.Green, (XkLedState)3));
    }

    [TestMethod]
    public void NoBuilderProducesAnEepromOrIdentityCommand()
    {
        // 189 unit ID, 199 save backlight state, 204 change PID, 238 reboot, 210 time stamp, 183 scroll lock: never built here.
        HashSet<byte> forbidden = [189, 199, 204, 238, 210, 183];
        IEnumerable<byte[]> all =
        [
            XkLightReports.SetLed(XkLedIndex.Green, XkLedState.On), XkLightReports.SetIndicatorBits(true, true),
            XkLightReports.SetFlashFrequency(10), XkLightReports.SetBacklight(0, 0, XkLedState.On),
            XkLightReports.SetBacklightRows(0, 1), XkLightReports.ToggleBacklights(), XkLightReports.SetBacklightIntensity(1, 1),
        ];
        Assert.IsFalse(all.Any(r => forbidden.Contains(r[1])));
    }

    [TestMethod]
    public void CapabilitiesFollowTheVendorPages()
    {
        XkLightCapabilities js = XkLights.CapabilitiesForPid(0x0426)!;
        Assert.IsTrue(js.Indicators && js.Backlights && js.SetBothIndicatorsCommand);
        Assert.IsFalse(js.DigitalOutputs);
        XkLightCapabilities si = XkLights.CapabilitiesForPid(0x04AB)!;
        Assert.IsTrue(si.Indicators && si.DigitalOutputs);
        Assert.IsFalse(si.Backlights);
        Assert.IsNotNull(XkLights.CapabilitiesForPid(0x04C5));
        Assert.IsNull(XkLights.CapabilitiesForPid(0x0001));
    }

    /// <summary>
    /// Writes only volatile light commands: turns the green and red indicator LEDs of each attached device on, then off, through each
    /// of the two ways (command 179 per LED; command 186 where documented), and checks every write succeeds. The lights cannot be
    /// seen from here; a person verifies them with the guided tool. Every light is left off, whatever happens.
    /// </summary>
    [TestMethod]
    public void IndicatorLedsOfEveryDeviceCanBeSwitchedOnAndOff()
    {
        IReadOnlyList<XkDeviceDescriptor> devices = TestSupport.RequireDevices();
        TestSupport.RetryOnce(() =>
        {
            foreach (XkDeviceDescriptor descriptor in devices)
            {
                using XkDevice device = XkDevice.Open(descriptor);
                XkLights lights = device.Lights;
                try
                {
                    Assert.AreEqual(XkError.Ok, lights.SetIndicators(XkLedState.On, XkLedState.On));
                    Thread.Sleep(150);
                    Assert.AreEqual(XkError.Ok, lights.SetIndicators(XkLedState.Off, XkLedState.Off));
                    if (lights.Capabilities?.SetBothIndicatorsCommand == true)
                    {
                        Assert.AreEqual(XkError.Ok, lights.SetIndicatorBits(true, true));
                        Thread.Sleep(150);
                        Assert.AreEqual(XkError.Ok, lights.SetIndicatorBits(false, false));
                    }
                }
                finally
                {
                    _ = lights.SetIndicators(XkLedState.Off, XkLedState.Off);
                    if (lights.Capabilities?.SetBothIndicatorsCommand == true)
                    {
                        _ = lights.SetIndicatorBits(false, false);
                    }
                }
            }
        });
    }

    [TestMethod]
    public void LightsOfAClosedDeviceReportDeviceRemoved()
    {
        XkDeviceDescriptor descriptor = TestSupport.RequireDevices()[0];
        XkDevice device = XkDevice.Open(descriptor);
        XkLights lights = device.Lights;
        device.Dispose();
        Assert.AreEqual(XkError.DeviceRemoved, lights.SetLed(XkLedIndex.Green, XkLedState.Off));
    }
}
