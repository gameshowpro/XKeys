// Copyright (c) Barjonas LLC 2026.
using System.Diagnostics;
using GameshowPro.XKeys.Hid;

namespace GameshowPro.XKeys.Tests;

/// <summary>
/// Decoding input reports into input changes, jog steps and shuttle moves, carried over from TriggerGate's decoder tests. Report layouts
/// are those measured on the XK-12 Jog &amp; Shuttle (PID 0x0426, 33-byte input report) and the XK-12 Switch Interface (PID 0x04AB, 37
/// bytes) on 2026-11-16: index 0 is the report ID, 1 the unit ID, 2 the data type, 3 to 6 the input word; the Jog &amp; Shuttle's jog step is
/// at index 7 (with bit 7 of index 3 or 4 pulsed), its shuttle position at index 8 (with bits 3 to 6 of indexes 3 to 6 and bit 7 of index
/// 5 showing it). No device needed.
/// </summary>
[TestClass]
public sealed unsafe class DecoderTests
{
    private const ushort JogAndShuttlePid = 0x0426;
    private const ushort SwitchInterfacePid = 0x04AB;
    private const int JogAndShuttleLength = 33;
    private const int SwitchInterfaceLength = 37;
    private const byte Input = 0;
    private const byte Generated = 2;

    private static uint s_sequence;

    /// <summary>A stamp with a new sequence number each time.</summary>
    private static XkReportStamp Stamp(uint? deviceMs = null) => new(Stopwatch.GetTimestamp(), s_sequence++, deviceMs, false);

    /// <summary>A report of <paramref name="length"/> bytes with this data type and the given bytes set; every other byte is 0.</summary>
    private static byte[] Report(int length, byte dataType, params (int Index, byte Value)[] bytes)
    {
        byte[] report = new byte[length];
        report[1] = 2; // unit ID, as the measured Jog & Shuttle
        report[2] = dataType;
        foreach ((int index, byte value) in bytes)
        {
            report[index] = value;
        }
        return report;
    }

    /// <summary>The Jog &amp; Shuttle's answer to Generate Data as recorded: shuttle at rest (index 5 bit 7) and a device timestamp at 9 to 12.</summary>
    private static readonly byte[] s_recordedJogAndShuttleGenerated = Recorded(JogAndShuttleLength, (0, "000202000080000000000D1CBE"));

    /// <summary>The Switch Interface's answer to Generate Data as recorded: timestamp at 32 to 35, reboot counter at 36.</summary>
    private static readonly byte[] s_recordedSwitchInterfaceGenerated = Recorded(SwitchInterfaceLength, (0, "000102"), (32, "000CE9BE01"));

    private static byte[] Recorded(int length, params (int Index, string Hex)[] runs)
    {
        byte[] report = new byte[length];
        foreach ((int index, string hex) in runs)
        {
            Convert.FromHexString(hex).CopyTo(report, index);
        }
        return report;
    }

    private static XkInputDecoder DecoderFor(ushort pid) => new(XkModel.ForPid(pid));

    private static XkInputDecoder JogAndShuttleWithBaseline()
    {
        XkInputDecoder decoder = DecoderFor(JogAndShuttlePid);
        _ = decoder.Decode(s_recordedJogAndShuttleGenerated, Stamp());
        return decoder;
    }

    [TestMethod]
    public void RecordedReports_HaveTheMeasuredLengths()
    {
        Assert.HasCount(JogAndShuttleLength, s_recordedJogAndShuttleGenerated);
        Assert.HasCount(SwitchInterfaceLength, s_recordedSwitchInterfaceGenerated);
    }

    [TestMethod]
    public void Classify_ReadsTheDataType()
    {
        Assert.AreEqual(XkReportKind.Input, XkReport.Classify(Report(JogAndShuttleLength, 0)));
        Assert.AreEqual(XkReportKind.Input, XkReport.Classify(Report(JogAndShuttleLength, 1)), "Program switch set");
        Assert.AreEqual(XkReportKind.Generated, XkReport.Classify(Report(JogAndShuttleLength, 2)));
        Assert.AreEqual(XkReportKind.Generated, XkReport.Classify(Report(JogAndShuttleLength, 3)), "Generated, program switch set");
        Assert.AreEqual(XkReportKind.Descriptor, XkReport.Classify(Report(JogAndShuttleLength, 214)));
        Assert.AreEqual(XkReportKind.Other, XkReport.Classify(Report(JogAndShuttleLength, 224)));
        Assert.AreEqual(XkReportKind.Unusable, XkReport.Classify(new byte[6]));
        Assert.AreEqual(XkReportKind.Generated, XkReport.Classify(s_recordedSwitchInterfaceGenerated));
    }

    [TestMethod]
    public void InputWord_IsIndexesThreeToSixLittleEndian()
    {
        Assert.AreEqual(0x4433_2211u, XkReport.InputWord(Report(JogAndShuttleLength, 0, (3, 0x11), (4, 0x22), (5, 0x33), (6, 0x44))));
        Assert.AreEqual(0u, XkReport.InputWord(new byte[6]));
    }

    [TestMethod]
    public void GenerateDataRequest_IsCommand177PaddedToTheOutputLength()
    {
        byte[] request = XkReport.GenerateDataRequest(36);

        Assert.HasCount(36, request);
        Assert.AreEqual(0, request[0]);
        Assert.AreEqual(177, request[1]);
        Assert.IsTrue(request.Skip(2).All(static b => b == 0));
    }

    [TestMethod]
    public void DescriptorReports_AreParsed_AsMeasured()
    {
        // Jog & Shuttle: 00 02 D6 00 20 80 23 20 04 08 00 0A 26 04 ...; Switch Interface: 00 01 D6 03 20 D0 11 00 02 08 40 02 AB 04 ...
        byte[] jogAndShuttle = Recorded(JogAndShuttleLength, (0, "0002D600208023200408000A2604"));
        byte[] switchInterface = Recorded(SwitchInterfaceLength, (0, "0001D60320D0110002084002AB04"));

        Assert.IsTrue(XkDescriptorReport.TryParse(jogAndShuttle, out XkDescriptorReport? a));
        Assert.AreEqual(new XkDescriptorReport(UnitId: 2, PidMode: 0, FirmwareVersion: 10, Pid: 0x0426), a);
        Assert.IsTrue(XkDescriptorReport.TryParse(switchInterface, out XkDescriptorReport? b));
        Assert.AreEqual(new XkDescriptorReport(UnitId: 1, PidMode: 3, FirmwareVersion: 2, Pid: 0x04AB), b);
        Assert.IsFalse(XkDescriptorReport.TryParse(s_recordedJogAndShuttleGenerated, out _), "not a descriptor");
        Assert.IsFalse(XkDescriptorReport.TryParse(jogAndShuttle.AsSpan(0, 13), out _), "too short");
    }

    [TestMethod]
    public void GeneratedReport_IsTheInitialState_NotChanges()
    {
        XkInputDecoder decoder = DecoderFor(JogAndShuttlePid);
        // Key 9 held when the session starts, and the shuttle at rest.
        XkDecodeResult result = decoder.Decode(Report(JogAndShuttleLength, Generated, (4, 0x02), (5, 0x80)), Stamp());

        Assert.IsTrue(result.IsBaseline);
        Assert.AreEqual(XkReportKind.Generated, result.Kind);
        Assert.IsNull(result.Inputs);
        Assert.AreEqual(1u << 9, decoder.PressedMask);
        CollectionAssert.AreEqual(new[] { 9 }, decoder.PressedInputs.ToArray());
        Assert.IsTrue(decoder.HasBaseline);
    }

    [TestMethod]
    public void RecordedGeneratedReports_GiveNoInputDown()
    {
        XkInputDecoder jogAndShuttle = DecoderFor(JogAndShuttlePid);
        XkInputDecoder switchInterface = DecoderFor(SwitchInterfacePid);

        XkDecodeResult a = jogAndShuttle.Decode(s_recordedJogAndShuttleGenerated, Stamp());
        XkDecodeResult b = switchInterface.Decode(s_recordedSwitchInterfaceGenerated, Stamp());

        Assert.IsTrue(a.IsBaseline);
        Assert.AreEqual(0u, jogAndShuttle.PressedMask, "The shuttle-at-rest bit is not an input");
        Assert.IsTrue(b.IsBaseline);
        Assert.AreEqual(0u, switchInterface.PressedMask, "The timestamp and reboot counter are not inputs");
    }

    [TestMethod]
    public void AKeyPressAndRelease_ChangeOnlyThatInput_WithTheReportsStamp()
    {
        XkInputDecoder decoder = JogAndShuttleWithBaseline();
        XkReportStamp pressStamp = Stamp(deviceMs: 0x000D_1D00);
        XkReportStamp releaseStamp = Stamp(deviceMs: 0x000D_1D60);

        XkDecodeResult press = decoder.Decode(Report(JogAndShuttleLength, Input, (5, 0x81)), pressStamp);
        XkDecodeResult release = decoder.Decode(Report(JogAndShuttleLength, Input, (5, 0x80)), releaseStamp);

        Assert.IsFalse(press.IsBaseline);
        Assert.IsNotNull(press.Inputs);
        Assert.AreEqual(new XkInputChange(16, true, pressStamp), press.Inputs.Changes.Single());
        Assert.AreEqual(pressStamp, press.Inputs.Stamp);
        Assert.AreEqual(1u << 16, press.Inputs.PressedMask);
        Assert.IsFalse(press.Inputs.IsResynchronization);
        Assert.AreEqual(new XkInputChange(16, false, releaseStamp), release.Inputs!.Changes.Single());
        Assert.AreEqual(0u, release.Inputs.PressedMask);
        Assert.AreEqual("key 3 (row 1, column 3)", press.Inputs.Model.InputName(16));
        Assert.AreEqual(0x000D_1D00u, press.Inputs.Stamp.DeviceMilliseconds);
    }

    [TestMethod]
    public void JogClicks_AreJogSteps_NeverInputChanges()
    {
        XkInputDecoder decoder = JogAndShuttleWithBaseline();

        // Clockwise: index 7 = +1 with index 3 bit 7; counter-clockwise: index 7 = -1 with index 4 bit 7; then back to rest.
        XkDecodeResult clockwise = decoder.Decode(Report(JogAndShuttleLength, Input, (3, 0x80), (5, 0x80), (7, 0x01)), Stamp());
        XkDecodeResult again = decoder.Decode(Report(JogAndShuttleLength, Input, (3, 0x80), (5, 0x80), (7, 0x01)), Stamp());
        XkDecodeResult rest = decoder.Decode(Report(JogAndShuttleLength, Input, (5, 0x80)), Stamp());
        XkDecodeResult counterClockwise = decoder.Decode(Report(JogAndShuttleLength, Input, (4, 0x80), (5, 0x80), (7, 0xFF)), Stamp());

        foreach (XkDecodeResult result in new[] { clockwise, again, rest, counterClockwise })
        {
            Assert.IsNull(result.Inputs);
            Assert.IsNull(result.Shuttle);
        }
        Assert.AreEqual(1, clockwise.Jog!.Steps);
        Assert.AreEqual(1, again.Jog!.Steps, "one step per report, even if the byte did not change");
        Assert.IsNull(rest.Jog);
        Assert.AreEqual(-1, counterClockwise.Jog!.Steps);
        Assert.AreEqual(0u, decoder.PressedMask);
    }

    [TestMethod]
    public void ShuttleMovement_IsShuttleMoves_NeverInputChanges()
    {
        XkInputDecoder decoder = JogAndShuttleWithBaseline();

        // Through +1, +7, -7, rest and +4: the shuttle bits of every key byte, the at-rest bit and the position byte change.
        (int Index, byte Value)[][] positions =
        [
            [(3, 0x08), (8, 0x01)],
            [(4, 0x10), (8, 0x07)],
            [(5, 0x40), (8, 0xF9)],
            [(5, 0x80)],
            [(6, 0x28), (8, 0x04)],
        ];
        int[] expected = [1, 7, -7, 0, 4];
        int previous = 0;
        for (int i = 0; i < positions.Length; i++)
        {
            XkDecodeResult result = decoder.Decode(Report(JogAndShuttleLength, Input, positions[i]), Stamp());
            Assert.IsNull(result.Inputs);
            Assert.IsNull(result.Jog);
            Assert.IsNotNull(result.Shuttle);
            Assert.AreEqual(expected[i], result.Shuttle.Position);
            Assert.AreEqual(previous, result.Shuttle.PreviousPosition);
            Assert.IsFalse(result.Shuttle.IsResynchronization);
            previous = expected[i];
        }
        Assert.AreEqual(4, decoder.ShuttlePosition);
        Assert.IsNull(decoder.Decode(Report(JogAndShuttleLength, Input, (6, 0x28), (8, 0x04)), Stamp()).Shuttle, "no move, no event");
    }

    [TestMethod]
    public void AKeyAndAJogClickInOneReport_GiveTheKeyAndTheStep_WithOneStamp()
    {
        XkInputDecoder decoder = JogAndShuttleWithBaseline();
        XkReportStamp stamp = Stamp();

        XkDecodeResult result = decoder.Decode(Report(JogAndShuttleLength, Input, (3, 0x81), (5, 0x80), (7, 0x01)), stamp);

        Assert.AreEqual(1u << 0, result.Inputs!.ChangedMask);
        Assert.AreEqual(stamp, result.Jog!.Stamp);
        Assert.AreEqual(stamp, result.Inputs.Stamp);
    }

    [TestMethod]
    public void TwoKeysInOneReport_AreOneGroup_SharingTheStamp()
    {
        XkInputDecoder decoder = JogAndShuttleWithBaseline();
        XkReportStamp stamp = Stamp();

        XkDecodeResult result = decoder.Decode(Report(JogAndShuttleLength, Input, (3, 0x04), (5, 0x80), (6, 0x01)), stamp);

        XkInputGroup group = result.Inputs!;
        Assert.AreEqual((1u << 2) | (1u << 24), group.ChangedMask);
        CollectionAssert.AreEqual(new[] { 2, 24 }, group.Changes.Select(c => c.Input).ToArray());
        Assert.IsTrue(group.Changes.All(c => c.IsPressed && c.Stamp == stamp));
        CollectionAssert.AreEqual(new[] { 2, 24 }, group.PressedInputs.ToArray());
    }

    [TestMethod]
    public void APressAndAReleaseInOneReport_AreOneGroup()
    {
        XkInputDecoder decoder = JogAndShuttleWithBaseline();
        _ = decoder.Decode(Report(JogAndShuttleLength, Input, (3, 0x01), (5, 0x80)), Stamp());

        XkInputGroup group = decoder.Decode(Report(JogAndShuttleLength, Input, (3, 0x02), (5, 0x80)), Stamp()).Inputs!;

        CollectionAssert.AreEqual(new[] { (0, false), (1, true) }, group.Changes.Select(c => (c.Input, c.IsPressed)).ToArray());
    }

    [TestMethod]
    public void SwitchInterface_LastSocketContact_IsInputEleven()
    {
        XkInputDecoder decoder = DecoderFor(SwitchInterfacePid);
        _ = decoder.Decode(s_recordedSwitchInterfaceGenerated, Stamp());

        XkDecodeResult result = decoder.Decode(Report(SwitchInterfaceLength, Input, (4, 0x08), (32, 0x0C), (33, 0xEA), (36, 0x01)), Stamp());

        Assert.AreEqual(1u << 11, result.Inputs!.ChangedMask);
        Assert.IsNull(result.Jog, "no jog on this model");
        Assert.IsNull(result.Shuttle);
    }

    [TestMethod]
    public void BitsOutsideTheModelsInputs_NeverChange()
    {
        XkInputDecoder decoder = DecoderFor(SwitchInterfacePid);
        _ = decoder.Decode(s_recordedSwitchInterfaceGenerated, Stamp());

        // Index 4 bits 4-7 and indexes 5 and 6 are not inputs of the XK-12 Switch Interface.
        XkDecodeResult result = decoder.Decode(Report(SwitchInterfaceLength, Input, (4, 0xF0), (5, 0xFF), (6, 0xFF)), Stamp());

        Assert.IsNull(result.Inputs);
    }

    [TestMethod]
    public void AnUnknownModel_TakesEveryBitOfTheWord()
    {
        XkInputDecoder decoder = DecoderFor(0x1234);
        _ = decoder.Decode(Report(JogAndShuttleLength, Generated), Stamp());

        XkDecodeResult result = decoder.Decode(Report(JogAndShuttleLength, Input, (6, 0x80)), Stamp());

        Assert.AreEqual(1u << 31, result.Inputs!.ChangedMask);
    }

    [TestMethod]
    public void DescriptorAndCustomReports_AreIgnored_AndLeaveTheStateAlone()
    {
        XkInputDecoder decoder = JogAndShuttleWithBaseline();
        _ = decoder.Decode(Report(JogAndShuttleLength, Input, (3, 0x01), (5, 0x80)), Stamp());

        // A descriptor report's index 3 is its mode byte, not keys.
        XkDecodeResult descriptor = decoder.Decode(Report(JogAndShuttleLength, 214, (3, 0x02), (11, 0x0A)), Stamp());
        XkDecodeResult custom = decoder.Decode(Report(JogAndShuttleLength, 224, (3, 0xFF), (8, 0x05)), Stamp());
        XkDecodeResult tooShort = decoder.Decode(new byte[5], Stamp());

        Assert.AreEqual(XkReportKind.Descriptor, descriptor.Kind);
        Assert.AreEqual(XkReportKind.Other, custom.Kind);
        Assert.AreEqual(XkReportKind.Unusable, tooShort.Kind);
        foreach (XkDecodeResult result in new[] { descriptor, custom, tooShort })
        {
            Assert.IsFalse(result.IsBaseline);
            Assert.IsNull(result.Inputs);
            Assert.IsNull(result.Jog);
            Assert.IsNull(result.Shuttle);
        }
        Assert.AreEqual(1u, decoder.PressedMask);
        Assert.AreEqual(0, decoder.ShuttlePosition);
    }

    [TestMethod]
    public void IgnoredReports_BeforeTheBaseline_DoNotSetIt()
    {
        XkInputDecoder decoder = DecoderFor(JogAndShuttlePid);

        _ = decoder.Decode(Report(JogAndShuttleLength, 214, (3, 0x02)), Stamp());

        Assert.IsFalse(decoder.HasBaseline);
    }

    [TestMethod]
    public void AnInputReportBeforeTheGeneratedOne_IsTheBaseline_AndTheGeneratedOneIsCompared()
    {
        XkInputDecoder decoder = DecoderFor(JogAndShuttlePid);

        XkDecodeResult first = decoder.Decode(Report(JogAndShuttleLength, Input, (3, 0x01)), Stamp());
        XkDecodeResult generated = decoder.Decode(Report(JogAndShuttleLength, Generated, (3, 0x01)), Stamp());

        Assert.IsTrue(first.IsBaseline);
        Assert.IsFalse(generated.IsBaseline);
        Assert.IsNull(generated.Inputs);
    }

    [TestMethod]
    public void AGeneratedReportAfterTheBaseline_ShowsWhatLostReportsChanged_AsAResynchronization()
    {
        XkInputDecoder decoder = JogAndShuttleWithBaseline();

        XkDecodeResult resync = decoder.Decode(Report(JogAndShuttleLength, Generated, (4, 0x04), (5, 0x00), (8, 0x03)), Stamp());

        Assert.AreEqual(1u << 10, resync.Inputs!.ChangedMask);
        Assert.IsTrue(resync.Inputs.IsResynchronization);
        Assert.AreEqual(XkReportKind.Generated, resync.Inputs.Kind);
        Assert.IsTrue(resync.Shuttle!.IsResynchronization);
        Assert.AreEqual(3, resync.Shuttle.Position);
        Assert.IsNull(resync.Jog, "jog steps come only from input reports");
    }

    [TestMethod]
    public void Reset_ReturnsTheHeldInputs_AndTheNextReportIsABaselineAgain()
    {
        XkInputDecoder decoder = JogAndShuttleWithBaseline();
        _ = decoder.Decode(Report(JogAndShuttleLength, Input, (3, 0x03), (5, 0x80), (8, 0x02)), Stamp());

        IReadOnlyList<int> held = decoder.Reset();
        XkDecodeResult next = decoder.Decode(Report(JogAndShuttleLength, Generated, (5, 0x80)), Stamp());

        CollectionAssert.AreEqual(new[] { 0, 1 }, held.ToArray());
        Assert.IsTrue(next.IsBaseline);
        Assert.AreEqual(0, decoder.ShuttlePosition);
        Assert.HasCount(0, decoder.Reset(), "Nothing is held after the new baseline");
    }

    [TestMethod]
    public void Reset_BeforeTheBaseline_HoldsNothing()
    {
        Assert.HasCount(0, DecoderFor(JogAndShuttlePid).Reset());
    }

    [TestMethod]
    public void AReportEntry_DecodesWithItsOwnStamp_AndAnErrorEntryIsUnusable()
    {
        XkInputDecoder decoder = JogAndShuttleWithBaseline();
        XkReportEntry entry = new() { Qpc = 123_456_789, DeviceMs = 0x000D_1D00, Sequence = 7, Flags = XkReportFlags.HasDeviceMs | XkReportFlags.Queued, Length = JogAndShuttleLength };
        byte[] report = Report(JogAndShuttleLength, Input, (6, 0x02), (5, 0x80));
        report.CopyTo(new Span<byte>(entry.Data, XkReportEntry.MaxDataLength));

        XkDecodeResult result = decoder.Decode(in entry);

        Assert.AreEqual(new XkReportStamp(123_456_789, 7, 0x000D_1D00, true), result.Inputs!.Stamp);
        Assert.AreEqual(25, result.Inputs.Changes.Single().Input);
        XkReportEntry error = new() { Flags = XkReportFlags.Error, Error = XkError.DeviceRemoved };
        Assert.AreEqual(XkReportKind.Unusable, decoder.Decode(in error).Kind);
        XkReportEntry noDeviceMs = new() { Qpc = 1, Sequence = 8, Length = JogAndShuttleLength };
        Assert.IsNull(XkReportStamp.From(noDeviceMs).DeviceMilliseconds);
    }

    [TestMethod]
    public void TheStamp_ConvertsToTimeSpansOnTheStopwatchClock()
    {
        XkReportStamp earlier = new(Stopwatch.Frequency * 10, 0, null, false);
        XkReportStamp later = new((Stopwatch.Frequency * 10) + (Stopwatch.Frequency / 1000 * 8), 1, null, false);

        Assert.AreEqual(TimeSpan.FromSeconds(10), earlier.ToTimeSpan());
        Assert.AreEqual(TimeSpan.FromMilliseconds(8), XkReportStamp.Between(earlier, later));
        Assert.AreEqual(TimeSpan.FromMilliseconds(-8), XkReportStamp.Between(later, earlier));
        XkReportStamp now = new(Stopwatch.GetTimestamp(), 2, null, false);
        Assert.IsTrue(now.GetElapsedTime() >= TimeSpan.Zero && now.GetElapsedTime() < TimeSpan.FromSeconds(5));
    }
}
