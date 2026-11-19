// Copyright (c) Barjonas LLC 2026.
using System.Diagnostics;
using GameshowPro.XKeys.Hid;
using Microsoft.Extensions.Logging.Abstractions;

namespace GameshowPro.XKeys.Tests;

/// <summary>
/// How <see cref="XKeysDevice{TTriggerKey}"/> turns decoded reports into trigger state, firing and times. No device needed: the decoded
/// groups are built here, as the device's InputsChanged event would deliver them.
/// </summary>
[TestClass]
public sealed class TriggerTimeTests
{
    private const ushort JogAndShuttlePid = 0x0426;
    private const int KeyOne = 0;        // key 1 (top left)
    private const int KeyTwelve = 26;    // key 12 (bottom right)

    private static XKeysDevice<TestKey> NewDevice(out XKeysIncomingTrigger one, out XKeysIncomingTrigger two)
    {
        XKeysDevice<TestKey> device = new("test", new XKeysDeviceSettings(JogAndShuttlePid), NullLoggerFactory.Instance);
        one = device.Triggers[TestKey.One];
        two = device.Triggers[TestKey.Two];
        foreach ((XKeysIncomingTrigger trigger, int id) in new[] { (one, KeyOne), (two, KeyTwelve) })
        {
            trigger.Setting.Id = id;
            trigger.Setting.IsEnabled = true;
            trigger.Setting.TriggerEdge = true;
            trigger.Setting.DebounceInterval = TimeSpan.Zero;
        }
        return device;
    }

    private static XkInputGroup Group(XkReportKind kind, uint changed, uint pressed, long timestamp, uint sequence, uint? deviceMs)
        => new(XkModel.ForPid(JogAndShuttlePid), new XkReportStamp(timestamp, sequence, deviceMs, WasQueued: false), kind, changed, pressed);

    private static uint Bit(int input) => 1u << input;

    [TestMethod]
    public void InputsChangedInOneReport_FireWithExactlyTheSameTime_TheReportsStampOnTheStopwatchClock()
    {
        XKeysDevice<TestKey> device = NewDevice(out XKeysIncomingTrigger one, out XKeysIncomingTrigger two);
        List<(XKeysIncomingTrigger Trigger, TimeSpan? TimeInHandler)> fired = [];
        one.Triggered += (_, _) => fired.Add((one, one.Time));
        two.Triggered += (_, _) => fired.Add((two, two.Time));
        long stamp = Stopwatch.GetTimestamp();

        device.ApplyInputs(Group(XkReportKind.Input, Bit(KeyOne) | Bit(KeyTwelve), Bit(KeyOne) | Bit(KeyTwelve), stamp, 5, 51_157_083));

        Assert.HasCount(2, fired);
        TimeSpan expected = new XkReportStamp(stamp, 5, null, false).ToTimeSpan();
        Assert.AreEqual(expected, Stopwatch.GetElapsedTime(0, stamp), "the Stopwatch clock: QPC ticks since its origin");
        Assert.IsTrue(fired.All(f => f.TimeInHandler == expected), "Time is set before Triggered is raised, to the report's stamp");
        Assert.AreEqual(one.Time, two.Time, "same report, same time: they cannot be ordered");
        Assert.AreEqual(51_157_083u, one.DeviceMilliseconds);
        Assert.AreEqual(51_157_083u, two.DeviceMilliseconds);
        Assert.IsTrue(one.IsDown && two.IsDown);
        Assert.IsNull(one.Ordinal, "X-keys runs no faceoff");
        Assert.IsFalse(one.IsTied);
    }

    [TestMethod]
    public void InputsFromSeparateReports_GetTheirOwnTimes_InTheOrderTheReportsArrived()
    {
        XKeysDevice<TestKey> device = NewDevice(out XKeysIncomingTrigger one, out XKeysIncomingTrigger two);
        long first = Stopwatch.GetTimestamp();
        long second = first + (Stopwatch.Frequency * 29 / 1000);

        device.ApplyInputs(Group(XkReportKind.Input, Bit(KeyTwelve), Bit(KeyTwelve), first, 5, 1000));
        device.ApplyInputs(Group(XkReportKind.Input, Bit(KeyOne), Bit(KeyOne) | Bit(KeyTwelve), second, 6, 1029));

        Assert.IsLessThan(one.Time!.Value, two.Time!.Value);
        Assert.AreEqual(TimeSpan.FromMilliseconds(29), one.Time.Value - two.Time.Value);
        Assert.AreEqual(1000u, two.DeviceMilliseconds);
        Assert.AreEqual(1029u, one.DeviceMilliseconds);
    }

    [TestMethod]
    public void AReleaseRecordsTheDeviceTimestamp_ButKeepsTheTimeOfTheLastTrigger_AndDoesNotFire()
    {
        XKeysDevice<TestKey> device = NewDevice(out XKeysIncomingTrigger one, out _);
        int fired = 0;
        one.Triggered += (_, _) => fired++;
        long press = Stopwatch.GetTimestamp();
        device.ApplyInputs(Group(XkReportKind.Input, Bit(KeyOne), Bit(KeyOne), press, 1, 100));
        TimeSpan? pressTime = one.Time;

        device.ApplyInputs(Group(XkReportKind.Input, Bit(KeyOne), 0, press + Stopwatch.Frequency, 2, 1100));

        Assert.AreEqual(1, fired);
        Assert.IsFalse(one.IsDown);
        Assert.AreEqual(pressTime, one.Time);
        Assert.AreEqual(1100u, one.DeviceMilliseconds);
    }

    [TestMethod]
    public void StateThatIsNotAPress_IsShown_ButNeverFires()
    {
        XKeysDevice<TestKey> device = NewDevice(out XKeysIncomingTrigger one, out XKeysIncomingTrigger two);
        two.Setting.TriggerEdge = false; // fires on release
        int fired = 0;
        one.Triggered += (_, _) => fired++;
        two.Triggered += (_, _) => fired++;

        // The answer to Generate Data at connection: key 1 and key 12 are down.
        device.ApplyState(Bit(KeyOne) | Bit(KeyTwelve));
        Assert.IsTrue(one.IsDown && two.IsDown);

        // A resynchronization after lost reports: key 1 went up at an unknown time.
        device.ApplyInputs(Group(XkReportKind.Generated, Bit(KeyOne), Bit(KeyTwelve), Stopwatch.GetTimestamp(), 9, null));
        Assert.IsFalse(one.IsDown);

        // The device goes away: everything is released, which would be key 12's configured edge.
        foreach (XKeysIncomingTrigger trigger in device.Triggers.Values)
        {
            trigger.Release();
        }
        Assert.IsFalse(two.IsDown);

        Assert.AreEqual(0, fired);
        Assert.IsNull(one.Time);
        Assert.IsNull(one.DeviceMilliseconds);
    }

    [TestMethod]
    public void AJogPulseBit_IsNotAnInput_SoItNeitherShowsNorFires()
    {
        XKeysDevice<TestKey> device = NewDevice(out XKeysIncomingTrigger one, out _);
        one.Setting.Id = 7; // the jog wheel's clockwise pulse
        int fired = 0;
        one.Triggered += (_, _) => fired++;

        // What the device does with a generated report whose input word has bit 7 set: it masks it with the model's inputs.
        device.ApplyState(Bit(7) & XkModel.ForPid(JogAndShuttlePid).InputMask);

        Assert.IsFalse(one.IsDown);
        Assert.AreEqual(0, fired);
    }

    [TestMethod]
    public void InputNames_AndTheDeviceModel_ComeFromTheCatalogue()
    {
        XKeysDevice<TestKey> device = NewDevice(out XKeysIncomingTrigger one, out XKeysIncomingTrigger two);

        Assert.AreEqual(XkModel.ForPid(JogAndShuttlePid), device.Model);
        Assert.AreEqual("key 1 (top left)", one.InputName);
        Assert.AreEqual("key 12 (bottom right)", two.InputName);
        one.Setting.Id = 30;
        Assert.AreEqual("input 30", one.InputName);
    }

    [TestMethod]
    public void OnlyTheWindowsBuild_ContainsTheWpfResourceDictionary()
    {
        System.Reflection.Assembly library = typeof(XKeysDevice<>).Assembly;
        Type? template = library.GetType("GameshowPro.XKeys.View.Template");
        bool hasResources = library.GetManifestResourceNames().Any(n => n.EndsWith(".g.resources", StringComparison.Ordinal));
#if WPF
        Assert.IsNotNull(template, "the Windows build compiles View/Template.xaml");
        Assert.IsTrue(hasResources);
#else
        Assert.IsNull(template, "the plain build has no WPF parts");
        Assert.IsFalse(hasResources);
#endif
    }
}
