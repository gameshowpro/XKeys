// Copyright (c) Barjonas LLC 2026.
using GameshowPro.XKeys.Hid;

namespace GameshowPro.XKeys.Tests;

/// <summary>The model catalog: the measured facts of the two test devices (docs/device-measurements.md), the documented ones, and the generic model. No device needed.</summary>
[TestClass]
public sealed class ModelTests
{
    private const ushort JogAndShuttlePid = 0x0426;
    private const ushort SwitchInterfacePid = 0x04AB;

    [TestMethod]
    public void JogAndShuttle_InputsAreTheTwelveMeasuredKeys_NamedByPosition()
    {
        XkModel model = XkModel.ForPid(JogAndShuttlePid);

        Assert.AreEqual("XK-12 Jog & Shuttle", model.Name);
        Assert.IsTrue(model.IsKnown);
        Assert.AreEqual(XkFactSource.Measured, model.InputsSource);
        CollectionAssert.AreEqual(new[] { 0, 1, 2, 8, 9, 10, 16, 17, 18, 24, 25, 26 }, model.Inputs.Select(i => i.Number).ToArray());
        Assert.AreEqual(0x0707_0707u, model.InputMask);
        Assert.IsTrue(model.Inputs.All(i => i.Kind == XkInputKind.Key));
        // Top row 0, 8, 16, 24; middle 1, 9, 17, 25; bottom 2, 10, 18, 26 (the backlight figure, confirmed by a guided session).
        Assert.AreEqual("key 1 (top left)", model.InputName(0));
        Assert.AreEqual("key 4 (top right)", model.InputName(24));
        Assert.AreEqual("key 6 (row 2, column 2)", model.InputName(9));
        Assert.AreEqual("key 9 (bottom left)", model.InputName(2));
        Assert.AreEqual("key 12 (bottom right)", model.InputName(26));
        Assert.AreEqual(6, model.FindInput(24)!.ReportIndex);
        Assert.AreEqual(0, model.FindInput(24)!.Bit);
    }

    [TestMethod]
    public void JogAndShuttle_JogPulsesAndShuttleBits_AreNonInputBits()
    {
        XkModel model = XkModel.ForPid(JogAndShuttlePid);

        foreach (int bit in new[] { 7, 15, 3, 4, 5, 6, 11, 12, 13, 14, 19, 20, 21, 22, 23, 27, 29 })
        {
            Assert.IsTrue(model.IsNonInputBit(bit), $"bit {bit}");
            Assert.AreEqual(0u, model.InputMask & (1u << bit), $"Bit {bit} must not be an input");
        }
        Assert.AreEqual(XkFactSource.Measured, model.NonInputBits.Single(b => b.Number == 7).Source);
        Assert.AreEqual(XkFactSource.Measured, model.NonInputBits.Single(b => b.Number == 15).Source);
        Assert.AreEqual(new XkJogShuttleLayout(7, 8, 7, XkFactSource.Measured), model.JogShuttle);
    }

    [TestMethod]
    public void JogAndShuttle_TimingTimestampLengthAndLights_AreTheMeasuredValues()
    {
        XkModel model = XkModel.ForPid(JogAndShuttlePid);

        Assert.AreEqual(TimeSpan.FromMilliseconds(8), model.ReportInterval);
        Assert.AreEqual(XkFactSource.Measured, model.ReportIntervalSource);
        Assert.AreEqual(TimeSpan.FromMilliseconds(29), model.GeneratedReportInterval);
        Assert.AreEqual(9, model.TimestampOffset);
        Assert.AreEqual(XkFactSource.Measured, model.TimestampOffsetSource);
        Assert.AreEqual(33, model.InputReportLength);
        Assert.AreEqual(new XkLightCapabilities(true, true, true, false), model.Lights);
        Assert.AreEqual(XkFactSource.Measured, model.LightsSource);
    }

    [TestMethod]
    public void JogAndShuttle_KeyReportSpacing_IsASeparateMeasuredFact_FromSevenObservations()
    {
        XkModel model = XkModel.ForPid(JogAndShuttlePid);

        XkObservedSpacing? spacing = model.MinimumKeyReportSpacing;
        Assert.IsNotNull(spacing);
        Assert.AreEqual(TimeSpan.FromMilliseconds(29), spacing.DeviceClock);
        Assert.AreEqual(TimeSpan.FromMilliseconds(24), spacing.HostMinimum);
        Assert.AreEqual(TimeSpan.FromMilliseconds(32), spacing.HostMaximum);
        Assert.AreEqual(7, spacing.Observations);
        Assert.AreEqual(XkFactSource.Measured, spacing.Source);
        StringAssert.Contains(spacing.Note, "small sample");
        Assert.AreEqual(TimeSpan.FromMilliseconds(8), model.ReportInterval, "the polling interval stays a separate fact");
        Assert.IsNull(XkModel.ForPid(SwitchInterfacePid).MinimumKeyReportSpacing, "not measured on the Switch Interface");
    }

    [TestMethod]
    public void SwitchInterface_InputsAreTwelveSocketContacts_RingEvenTipOdd()
    {
        XkModel model = XkModel.ForPid(SwitchInterfacePid);

        Assert.AreEqual("XK-12 Switch Interface", model.Name);
        Assert.AreEqual(0x0FFFu, model.InputMask);
        Assert.AreEqual(XkInputKind.SocketRing, model.FindInput(4)!.Kind);
        Assert.AreEqual(XkInputKind.SocketTip, model.FindInput(5)!.Kind);
        Assert.AreEqual("socket 3 tip (button)", model.InputName(5));
        StringAssert.StartsWith(model.InputName(4), "socket 3 ring");
        Assert.AreEqual(TimeSpan.FromMilliseconds(1), model.ReportInterval);
        Assert.AreEqual(32, model.TimestampOffset);
        Assert.AreEqual(37, model.InputReportLength);
        Assert.IsNull(model.JogShuttle);
        Assert.AreEqual(0, model.NonInputBits.Count);
    }

    [TestMethod]
    public void Xk3SwitchInterface_FollowsTheVendorPage_WithPlugDetectAndSwitchThreeAtBitFour()
    {
        XkModel model = XkModel.ForPid(0x04C5);

        Assert.AreEqual(XkFactSource.VendorDocumentation, model.InputsSource);
        CollectionAssert.AreEqual(new[] { 0, 1, 2, 4 }, model.Inputs.Select(i => i.Number).ToArray());
        Assert.AreEqual(XkInputKind.PlugDetect, model.FindInput(2)!.Kind);
        Assert.AreEqual("switch 3", model.InputName(4));
        Assert.IsNull(model.ReportInterval, "not measured");
    }

    [TestMethod]
    public void OtherPidsOfAModel_CarryTheirInstanceNumber()
    {
        Assert.AreEqual("XK-12 Jog & Shuttle", XkModel.ForPid(0x0426).NameForPid(0x0426));
        Assert.AreEqual("XK-12 Jog & Shuttle #3", XkModel.ForPid(0x0428).NameForPid(0x0428));
        Assert.AreEqual("XK-12 Switch Interface #4", XkModel.ForPid(SwitchInterfacePid).NameForPid(SwitchInterfacePid));
        Assert.AreSame(XkModel.ForPid(0x0426), XkModel.ForPid(0x0428), "one catalog entry per model");
    }

    [TestMethod]
    public void UnknownPid_GivesAGenericModel_WithEveryBitOfTheWordAsAnInput()
    {
        XkModel model = XkModel.ForPid(0x1234);

        Assert.IsFalse(model.IsKnown);
        Assert.AreEqual("Unknown Device (0x1234)", model.Name);
        Assert.AreEqual("Unknown Device (0x1234)", model.NameForPid(0x1234));
        Assert.HasCount(32, model.Inputs);
        Assert.AreEqual(uint.MaxValue, model.InputMask);
        Assert.AreEqual("input 31", model.InputName(31));
        Assert.AreEqual(-1, model.TimestampOffset);
        Assert.IsNull(model.Lights);
        Assert.AreEqual(XkFactSource.Unconfirmed, model.InputsSource);
    }

    [TestMethod]
    public void TheNamesAreThoseOfThe20Tables()
    {
        // The WPF library's and TriggerGate's name tables, which the catalog replaces.
        (ushort Pid, string Name)[] expected =
        [
            (0x04A8, "XK-12 Switch Interface"), (0x0403, "XK-24"), (0x04D3, "XK-24 (KVM)"), (0x0426, "XK-12 Jog & Shuttle"),
            (0x04C5, "XK-3 Switch Interface"), (0x0429, "XK-12 Joystick"), (0x0514, "XK-3 Switch Interface (KVM)"),
            (0x0516, "XK-12 Switch Interface (KVM)"),
        ];
        foreach ((ushort pid, string name) in expected)
        {
            Assert.AreEqual(name, XkModel.ForPid(pid).Name);
        }
        Assert.HasCount(expected.Length, XkModel.All);
    }

    [TestMethod]
    public void EveryModel_HasDistinctInputsInsideTheWord_DisjointFromItsNonInputBits_AndNoOverlappingPids()
    {
        foreach (XkModel model in XkModel.All)
        {
            int[] numbers = [.. model.Inputs.Select(i => i.Number)];
            CollectionAssert.AllItemsAreUnique(numbers, model.Name);
            CollectionAssert.AreEqual(numbers.Order().ToArray(), numbers, $"{model.Name}: ascending");
            Assert.IsTrue(numbers.All(n => n is >= 0 and < 32), model.Name);
            Assert.IsTrue(model.NonInputBits.All(b => (model.InputMask & (1u << b.Number)) == 0), model.Name);
            Assert.IsTrue(model.Inputs.All(i => !string.IsNullOrWhiteSpace(i.Name)), model.Name);
            Assert.IsTrue(model.FirstPid <= model.LastPid, model.Name);
            foreach (XkModel other in XkModel.All.Where(o => o != model))
            {
                Assert.IsTrue(model.LastPid < other.FirstPid || other.LastPid < model.FirstPid, $"{model.Name} overlaps {other.Name}");
            }
        }
    }

    [TestMethod]
    public void LightCapabilities_AreTheSameThroughTheLightsClass()
    {
        foreach (ushort pid in new ushort[] { 0x0426, 0x0428, 0x04A8, 0x04AB, 0x04C5, 0x04C8, 0x0403, 0x0514, 0x0001 })
        {
            Assert.AreEqual(XkModel.ForPid(pid).Lights, XkLights.CapabilitiesForPid(pid), $"0x{pid:X4}");
        }
    }

    /// <summary>The catalog's timestamp offsets and the native library's table (which XkDevice.Open uses by default) must agree for every PID.</summary>
    [TestMethod]
    public void TimestampOffsets_AgreeWithTheNativeTable()
    {
        TestSupport.RequireDll();
        foreach (XkModel model in XkModel.All)
        {
            for (int pid = model.FirstPid; pid <= model.LastPid; pid++)
            {
                Assert.AreEqual(XkHidLibrary.TimestampOffsetForPid((ushort)pid), model.TimestampOffset, $"{model.Name} 0x{pid:X4}");
            }
        }
        Assert.AreEqual(XkHidLibrary.TimestampOffsetForPid(0x1234), XkModel.ForPid(0x1234).TimestampOffset);
    }
}
