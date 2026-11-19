// Copyright (c) Barjonas LLC 2026.
namespace GameshowPro.XKeys.Hid;

/// <summary>Where a fact in <see cref="XkModel"/> comes from.</summary>
public enum XkFactSource
{
    /// <summary>Measured on a real device (a test machine, 2026-11-15 and 2026-11-16, see docs/device-measurements.md: an XK-12 Jog &amp; Shuttle, PID 0x0426, and an XK-12 Switch Interface, PID 0x04AB).</summary>
    Measured,
    /// <summary>From P.I. Engineering's SDK documentation (github.com/piengineering/PI-Engineering-SDK, Documentation/pages); not measured.</summary>
    VendorDocumentation,
    /// <summary>Neither measured nor checked against the vendor's pages: carried over from the 2.0 tables of this library and TriggerGate, or a default for an unknown model.</summary>
    Unconfirmed,
}

/// <summary>What kind of contact an <see cref="XkInput"/> is.</summary>
public enum XkInputKind
{
    /// <summary>A key on the device.</summary>
    Key,
    /// <summary>The tip contact of a Switch Interface socket: the button of a mono (tip-sleeve) plug.</summary>
    SocketTip,
    /// <summary>The ring contact of a Switch Interface socket: on for as long as a mono plug is in (the sleeve shorts it); a second button with a stereo plug.</summary>
    SocketRing,
    /// <summary>A switch input of a Switch Interface whose sockets have one contact each.</summary>
    Switch,
    /// <summary>On while anything is plugged in. A real input, but it does not report a press.</summary>
    PlugDetect,
    /// <summary>A bit of an unknown model's input bytes; it may not be an input at all.</summary>
    Unknown,
}

/// <summary>
/// One input of a model. Input <see cref="Number"/> n is bit (n % 8) of input report index 3 + n / 8 (index 0 being the report ID), the
/// numbering this library, its trigger IDs (<c>IncomingTriggerSetting.Id</c>) and TriggerGate have always used.
/// </summary>
/// <param name="Number">0 to 31.</param>
/// <param name="Name">A readable name, for example "key 1 (top left)" or "socket 3 tip (button)".</param>
public sealed record XkInput(int Number, string Name, XkInputKind Kind)
{
    /// <summary>The input report index that carries this input (index 0 is the report ID).</summary>
    public int ReportIndex => XkReport.FirstInputIndex + (Number / 8);

    /// <summary>The bit of <see cref="ReportIndex"/> that carries this input.</summary>
    public int Bit => Number % 8;
}

/// <summary>A bit of the input bytes that has a meaning but must never be treated as an input (it is not a press).</summary>
/// <param name="Number">The bit, numbered as inputs are (bit n % 8 of report index 3 + n / 8).</param>
/// <param name="Meaning">What the bit says, for example "jog clockwise pulse".</param>
public sealed record XkNonInputBit(int Number, string Meaning, XkFactSource Source);

/// <summary>Where a model's jog wheel and shuttle ring report. Decoded into <see cref="XkJogStep"/> and <see cref="XkShuttleMove"/>.</summary>
/// <param name="JogIndex">The report index whose signed byte is the jog step of that report: +1 clockwise, -1 counter-clockwise, 0 none.</param>
/// <param name="ShuttleIndex">The report index whose signed byte is the shuttle position: 0 at rest, +1 to +<paramref name="MaxShuttlePosition"/> clockwise, negative counter-clockwise.</param>
public sealed record XkJogShuttleLayout(int JogIndex, int ShuttleIndex, int MaxShuttlePosition, XkFactSource Source);

/// <summary>A spacing between two reports that was seen on a real device: what it was by the device's own clock and at the host, and how often it was seen.</summary>
/// <param name="DeviceClock">The spacing by the device's own millisecond timestamp (the difference of the two reports' timestamps).</param>
/// <param name="HostMinimum">The smallest spacing of the two reports' host stamps (<see cref="XkReportStamp.Timestamp"/>).</param>
/// <param name="HostMaximum">The largest spacing of the two reports' host stamps.</param>
/// <param name="Observations">How many pairs of reports were seen.</param>
/// <param name="Note">Where and how it was seen, and how far to trust it.</param>
public sealed record XkObservedSpacing(TimeSpan DeviceClock, TimeSpan HostMinimum, TimeSpan HostMaximum, int Observations, XkFactSource Source, string Note);

/// <summary>
/// Everything this library knows about one X-keys model, in one immutable place: name, product IDs, which input bits are real inputs and
/// what to call them, which bits must never be treated as inputs, report length, timing, device timestamp and lights. Each fact that
/// varies in certainty carries its <see cref="XkFactSource"/>. Get one with <see cref="ForPid"/>; <see cref="All"/> lists the catalog.
/// </summary>
/// <remarks>
/// Input numbers follow the report: input n is bit (n % 8) of report index 3 + n / 8, so the four bytes at indexes 3 to 6 form a 32-bit
/// input word (<see cref="InputMask"/>). Sources: this repository's <c>docs/device-measurements.md</c>, TriggerGate's 2.0 model table, this library's 2.0 device name table (which this catalog replaces), and the vendor's SDK pages.
/// </remarks>
public sealed class XkModel
{
    private readonly uint _nonInputMask;

    private XkModel(
        string name,
        ushort firstPid,
        ushort lastPid,
        bool isKnown,
        IReadOnlyList<XkInput> inputs,
        XkFactSource inputsSource,
        IReadOnlyList<XkNonInputBit> nonInputBits,
        XkJogShuttleLayout? jogShuttle,
        int? inputReportLength,
        XkFactSource inputReportLengthSource,
        int timestampOffset,
        XkFactSource timestampOffsetSource,
        TimeSpan? reportInterval,
        XkFactSource reportIntervalSource,
        TimeSpan? generatedReportInterval,
        XkFactSource generatedReportIntervalSource,
        XkLightCapabilities? lights,
        XkFactSource lightsSource,
        XkObservedSpacing? minimumKeyReportSpacing = null)
    {
        Name = name;
        FirstPid = firstPid;
        LastPid = lastPid;
        IsKnown = isKnown;
        Inputs = inputs;
        InputsSource = inputsSource;
        NonInputBits = nonInputBits;
        JogShuttle = jogShuttle;
        InputReportLength = inputReportLength;
        InputReportLengthSource = inputReportLengthSource;
        TimestampOffset = timestampOffset;
        TimestampOffsetSource = timestampOffsetSource;
        ReportInterval = reportInterval;
        ReportIntervalSource = reportIntervalSource;
        GeneratedReportInterval = generatedReportInterval;
        GeneratedReportIntervalSource = generatedReportIntervalSource;
        Lights = lights;
        LightsSource = lightsSource;
        MinimumKeyReportSpacing = minimumKeyReportSpacing;
        foreach (XkInput input in inputs)
        {
            InputMask |= 1u << input.Number;
        }
        foreach (XkNonInputBit bit in nonInputBits)
        {
            _nonInputMask |= 1u << bit.Number;
        }
    }

    /// <summary>The model's name, for example "XK-12 Jog &amp; Shuttle"; for an unknown model "Unknown Device (0x1234)". See <see cref="NameForPid"/>.</summary>
    public string Name { get; }

    /// <summary>The first product ID of the model (its PID #1).</summary>
    public ushort FirstPid { get; }

    /// <summary>The last product ID of the model. The model's PIDs are <see cref="FirstPid"/> to this, inclusive; which one a device uses is its PID mode.</summary>
    public ushort LastPid { get; }

    /// <summary>False for the generic model <see cref="ForPid"/> returns for a PID that is not in the catalog.</summary>
    public bool IsKnown { get; }

    /// <summary>The real inputs, ascending by <see cref="XkInput.Number"/>. For an unknown model, every bit of the input word.</summary>
    public IReadOnlyList<XkInput> Inputs { get; }

    public XkFactSource InputsSource { get; }

    /// <summary><see cref="Inputs"/> as bits of the input word (report indexes 3 to 6, little-endian): bit n is input n.</summary>
    public uint InputMask { get; }

    /// <summary>Bits of the input word with a known meaning that is not a press (jog pulses, shuttle position). Never in <see cref="InputMask"/>.</summary>
    public IReadOnlyList<XkNonInputBit> NonInputBits { get; }

    /// <summary>Where the jog and shuttle report, or null if the model has none.</summary>
    public XkJogShuttleLayout? JogShuttle { get; }

    /// <summary>The input report length on Windows, report ID included, or null if not known.</summary>
    public int? InputReportLength { get; }

    public XkFactSource InputReportLengthSource { get; }

    /// <summary>
    /// Index of the 4-byte big-endian device timestamp (milliseconds since plug-in) in the input report, or -1 if the model has none or it
    /// is not known. The same table as <see cref="XkHidLibrary.TimestampOffsetForPid"/>, which <see cref="XkDevice.Open"/> uses by default.
    /// </summary>
    public int TimestampOffset { get; }

    public XkFactSource TimestampOffsetSource { get; }

    /// <summary>
    /// The smallest spacing of two input reports: the interval at which Windows polls the device's interrupt endpoint (the largest power of
    /// two not above its <c>bInterval</c>). Two presses closer together than this always arrive in one report and cannot be ordered. The
    /// firmware may space key reports further apart than this: see <see cref="MinimumKeyReportSpacing"/>. Null if not known.
    /// </summary>
    public TimeSpan? ReportInterval { get; }

    public XkFactSource ReportIntervalSource { get; }

    /// <summary>The smallest spacing of two answers to Generate Data (command 177), a limit of the firmware; null if not known.</summary>
    public TimeSpan? GeneratedReportInterval { get; }

    public XkFactSource GeneratedReportIntervalSource { get; }

    /// <summary>What the model's lights can do, or null if not known.</summary>
    public XkLightCapabilities? Lights { get; }

    public XkFactSource LightsSource { get; }

    /// <summary>
    /// The smallest spacing seen between two consecutive key reports (a change in one report, the next change in the next), separate from
    /// <see cref="ReportInterval"/>, the USB polling interval, which is a lower bound the firmware need not reach. Null where nobody has
    /// measured it. Two presses that the firmware reports in consecutive reports are this far apart in the stamps even if they were closer
    /// in reality, so it bounds how finely presses on one device can be ordered.
    /// </summary>
    public XkObservedSpacing? MinimumKeyReportSpacing { get; }

    /// <summary>Whether <paramref name="pid"/> is one of this model's product IDs.</summary>
    public bool CoversPid(ushort pid) => pid >= FirstPid && pid <= LastPid;

    /// <summary>
    /// The name of a device of this model with product ID <paramref name="pid"/>: <see cref="Name"/>, followed by " #n" when the PID is the
    /// model's n-th rather than its first (for example "XK-12 Switch Interface #4" for 0x04AB). The format of the 2.0 name tables.
    /// </summary>
    public string NameForPid(ushort pid)
    {
        int instance = pid - FirstPid;
        return IsKnown && instance > 0 && pid <= LastPid ? $"{Name} #{instance + 1}" : Name;
    }

    /// <summary>The input with this number, or null if it is not one of the model's inputs.</summary>
    public XkInput? FindInput(int number)
    {
        foreach (XkInput input in Inputs)
        {
            if (input.Number == number)
            {
                return input;
            }
        }
        return null;
    }

    /// <summary>The name of input <paramref name="number"/>: from <see cref="Inputs"/>, else "input n".</summary>
    public string InputName(int number) => FindInput(number)?.Name ?? $"input {number}";

    /// <summary>Whether bit <paramref name="number"/> of the input word has a known meaning that is not a press.</summary>
    public bool IsNonInputBit(int number) => number is >= 0 and < XkReport.InputBitCount && (_nonInputMask & (1u << number)) != 0;

    public override string ToString() => IsKnown ? $"{Name} (0x{FirstPid:X4}-0x{LastPid:X4})" : Name;

    /// <summary>The catalog: every model this library knows, in no particular order.</summary>
    public static IReadOnlyList<XkModel> All { get; } = BuildCatalog();

    /// <summary>The model with product ID <paramref name="pid"/>, or, for a PID not in the catalog, a generic model (<see cref="IsKnown"/> false) named "Unknown Device (0x1234)" whose inputs are every bit of the input word.</summary>
    public static XkModel ForPid(ushort pid)
    {
        foreach (XkModel model in All)
        {
            if (model.CoversPid(pid))
            {
                return model;
            }
        }
        return Generic(pid);
    }

    private static XkModel[] BuildCatalog()
    {
        // Answers to Generate Data come at most once per 29 ms of the device's clock on both units (measured); a firmware limit.
        TimeSpan generatedInterval = TimeSpan.FromMilliseconds(29);
        XkLightCapabilities jogShuttleLights = new(Indicators: true, SetBothIndicatorsCommand: true, Backlights: true, DigitalOutputs: false);
        XkLightCapabilities switchInterfaceLights = new(Indicators: true, SetBothIndicatorsCommand: false, Backlights: false, DigitalOutputs: true);
        return
        [
            // Six sockets of two contacts each, index 3 bits 0-7 and index 4 bits 0-3: measured with a guided session (2026-11-16; docs/device-measurements.md, "Measured with `xkprobe guide`").
            // Polling interval 1 ms (bInterval 1), timestamp at 32-35 and 37-byte reports measured on PID 0x04AB. Lights: indicators seen
            // working; the two digital outputs are documented but untested (nothing was connected), so the capabilities count as documented.
            new("XK-12 Switch Interface", 0x04A8, 0x04AB, true, Sockets(6), XkFactSource.Measured, [], null,
                37, XkFactSource.Measured, 32, XkFactSource.Measured,
                TimeSpan.FromMilliseconds(1), XkFactSource.Measured, generatedInterval, XkFactSource.Measured,
                switchInterfaceLights, XkFactSource.VendorDocumentation),
            new("XK-24", 0x0403, 0x0405, true, KeyMatrix(columns: 4, rows: 6), XkFactSource.Unconfirmed, [], null,
                null, XkFactSource.Unconfirmed, -1, XkFactSource.Unconfirmed,
                null, XkFactSource.Unconfirmed, null, XkFactSource.Unconfirmed,
                null, XkFactSource.Unconfirmed),
            new("XK-24 (KVM)", 0x04D3, 0x04D4, true, KeyMatrix(columns: 4, rows: 6), XkFactSource.Unconfirmed, [], null,
                null, XkFactSource.Unconfirmed, -1, XkFactSource.Unconfirmed,
                null, XkFactSource.Unconfirmed, null, XkFactSource.Unconfirmed,
                null, XkFactSource.Unconfirmed),
            // Four keys per row in three rows, each row position a report byte and each row a bit: measured with a guided session (docs/device-measurements.md).
            // Polling interval 8 ms (bInterval 10), timestamp at 9-12 (not 10-13), 33-byte reports, Generate Data limit: measured.
            // Lights (indicators, command 186, both backlight banks): exercised with the guided lights session.
            new("XK-12 Jog & Shuttle", 0x0426, 0x0428, true, KeyMatrix(columns: 4, rows: 3), XkFactSource.Measured, JogShuttleBits(),
                new XkJogShuttleLayout(JogIndex: 7, ShuttleIndex: 8, MaxShuttlePosition: 7, XkFactSource.Measured),
                33, XkFactSource.Measured, 9, XkFactSource.Measured,
                TimeSpan.FromMilliseconds(8), XkFactSource.Measured, generatedInterval, XkFactSource.Measured,
                jogShuttleLights, XkFactSource.Measured,
                // Seven pairs of consecutive key reports in a guided timing session on 2026-11-16 (docs/device-measurements.md):
                // 29 ms (five times) or 30 ms (twice) apart by the device's clock, 24 to 32 ms by the host stamps, never closer.
                new XkObservedSpacing(TimeSpan.FromMilliseconds(29), TimeSpan.FromMilliseconds(24), TimeSpan.FromMilliseconds(32), 7, XkFactSource.Measured,
                    "Seven pairs of consecutive key reports (two keys pressed or released together) in one guided session on 2026-11-16: 29 to 30 ms apart by the device's clock, 24 to 32 ms by the host stamps (on the 8 ms polling grid), never closer. A small sample: the firmware may allow less.")),
            // SW2 at bit 0, SW1 at bit 1, "something plugged in" at bit 2, SW3 at bit 4 (vendor page). The 2.0 tables used inputs 0-2,
            // which took the plug-detect bit as a switch and missed switch 3.
            new("XK-3 Switch Interface", 0x04C5, 0x04C8, true, Xk3Switches(), XkFactSource.VendorDocumentation, [], null,
                37, XkFactSource.VendorDocumentation, 32, XkFactSource.VendorDocumentation,
                null, XkFactSource.Unconfirmed, null, XkFactSource.Unconfirmed,
                switchInterfaceLights, XkFactSource.VendorDocumentation),
            // Same key layout as the Jog & Shuttle according to the vendor's page; the joystick itself is not decoded.
            new("XK-12 Joystick", 0x0429, 0x042B, true, KeyMatrix(columns: 4, rows: 3), XkFactSource.VendorDocumentation, [], null,
                33, XkFactSource.VendorDocumentation, 13, XkFactSource.VendorDocumentation,
                null, XkFactSource.Unconfirmed, null, XkFactSource.Unconfirmed,
                null, XkFactSource.Unconfirmed),
            new("XK-3 Switch Interface (KVM)", 0x0514, 0x0515, true, Xk3Switches(), XkFactSource.Unconfirmed, [], null,
                null, XkFactSource.Unconfirmed, -1, XkFactSource.Unconfirmed,
                null, XkFactSource.Unconfirmed, null, XkFactSource.Unconfirmed,
                null, XkFactSource.Unconfirmed),
            new("XK-12 Switch Interface (KVM)", 0x0516, 0x0517, true, Sockets(6), XkFactSource.Unconfirmed, [], null,
                null, XkFactSource.Unconfirmed, -1, XkFactSource.Unconfirmed,
                null, XkFactSource.Unconfirmed, null, XkFactSource.Unconfirmed,
                null, XkFactSource.Unconfirmed),
        ];
    }

    private static XkModel Generic(ushort pid)
    {
        XkInput[] inputs = new XkInput[XkReport.InputBitCount];
        for (int i = 0; i < inputs.Length; i++)
        {
            inputs[i] = new XkInput(i, $"input {i}", XkInputKind.Unknown);
        }
        return new($"Unknown Device (0x{pid:x2})", pid, pid, false, inputs, XkFactSource.Unconfirmed, [], null,
            null, XkFactSource.Unconfirmed, -1, XkFactSource.Unconfirmed,
            null, XkFactSource.Unconfirmed, null, XkFactSource.Unconfirmed,
            null, XkFactSource.Unconfirmed);
    }

    /// <summary>
    /// Keys in <paramref name="rows"/> rows of <paramref name="columns"/>: each column is one report byte and each row one bit of it, so the
    /// key in row r, column c is input c * 8 + r. Keys are numbered from 1, left to right and top to bottom, as the backlight figure numbers them.
    /// </summary>
    private static XkInput[] KeyMatrix(int columns, int rows)
    {
        List<XkInput> inputs = [];
        for (int column = 0; column < columns; column++)
        {
            for (int row = 0; row < rows; row++)
            {
                string? vertical = row == 0 ? "top" : row == rows - 1 ? "bottom" : null;
                string? horizontal = column == 0 ? "left" : column == columns - 1 ? "right" : null;
                string position = vertical is not null && horizontal is not null ? $"{vertical} {horizontal}" : $"row {row + 1}, column {column + 1}";
                inputs.Add(new XkInput((column * 8) + row, $"key {(row * columns) + column + 1} ({position})", XkInputKind.Key));
            }
        }
        return [.. inputs.OrderBy(i => i.Number)];
    }

    /// <summary>Sockets of two contacts: socket n uses input 2(n-1) for its ring (the vendor's "nR") and 2(n-1)+1 for its tip ("nL").</summary>
    private static XkInput[] Sockets(int count)
    {
        XkInput[] inputs = new XkInput[count * 2];
        for (int socket = 1; socket <= count; socket++)
        {
            int ring = (socket - 1) * 2;
            inputs[ring] = new XkInput(ring, $"socket {socket} ring (plug detect with a mono plug)", XkInputKind.SocketRing);
            inputs[ring + 1] = new XkInput(ring + 1, $"socket {socket} tip (button)", XkInputKind.SocketTip);
        }
        return inputs;
    }

    private static XkInput[] Xk3Switches() =>
    [
        new(0, "switch 2", XkInputKind.Switch),
        new(1, "switch 1", XkInputKind.Switch),
        new(2, "plug detect (on while anything is plugged in)", XkInputKind.PlugDetect),
        new(4, "switch 3", XkInputKind.Switch),
    ];

    /// <summary>
    /// The Jog &amp; Shuttle's bits 3 to 7 of the key bytes. Jog pulses (bit 7 of indexes 3 and 4) were measured; the shuttle bits are
    /// from the vendor's page, except the at-rest bit (index 5 bit 7), which every recorded report shows.
    /// </summary>
    private static XkNonInputBit[] JogShuttleBits() =>
    [
        new(3, "shuttle +1", XkFactSource.VendorDocumentation),
        new(4, "shuttle +5", XkFactSource.VendorDocumentation),
        new(5, "shuttle -1", XkFactSource.VendorDocumentation),
        new(6, "shuttle -5", XkFactSource.VendorDocumentation),
        new(7, "jog clockwise pulse", XkFactSource.Measured),
        new(11, "shuttle +2", XkFactSource.VendorDocumentation),
        new(12, "shuttle +6", XkFactSource.VendorDocumentation),
        new(13, "shuttle -2", XkFactSource.VendorDocumentation),
        new(14, "shuttle -6", XkFactSource.VendorDocumentation),
        new(15, "jog counter-clockwise pulse", XkFactSource.Measured),
        new(19, "shuttle +3", XkFactSource.VendorDocumentation),
        new(20, "shuttle +7", XkFactSource.VendorDocumentation),
        new(21, "shuttle -3", XkFactSource.VendorDocumentation),
        new(22, "shuttle -7", XkFactSource.VendorDocumentation),
        new(23, "shuttle at rest", XkFactSource.Measured),
        new(27, "shuttle +4", XkFactSource.VendorDocumentation),
        new(29, "shuttle -4", XkFactSource.VendorDocumentation),
    ];
}
