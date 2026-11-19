// Copyright (c) Barjonas LLC 2026.
namespace GameshowPro.XKeys.Hid;

/// <summary>
/// One input that went down or up. Changes with the same <see cref="Stamp"/> (equal <see cref="XkReportStamp.Sequence"/> on one device)
/// arrived in one report and cannot be ordered among themselves: the device polled them together.
/// </summary>
/// <param name="Input">The input number (bit n % 8 of report index 3 + n / 8); <see cref="XkModel.InputName"/> names it.</param>
/// <param name="IsPressed">True for down (the contact closed), false for up.</param>
public readonly record struct XkInputChange(int Input, bool IsPressed, XkReportStamp Stamp);

/// <summary>
/// The inputs that one report changed: a group of simultaneous changes. Everything in it has the report's <see cref="Stamp"/>; the order
/// of <see cref="Changes"/> (ascending input number) says nothing about which happened first, because the device polled them together.
/// </summary>
public sealed class XkInputGroup
{
    internal XkInputGroup(XkModel model, XkReportStamp stamp, XkReportKind kind, uint changedMask, uint pressedMask)
    {
        Model = model;
        Stamp = stamp;
        Kind = kind;
        ChangedMask = changedMask;
        PressedMask = pressedMask;
        XkInputChange[] changes = new XkInputChange[System.Numerics.BitOperations.PopCount(changedMask)];
        int i = 0;
        for (uint bits = changedMask; bits != 0; bits &= bits - 1)
        {
            int input = System.Numerics.BitOperations.TrailingZeroCount(bits);
            changes[i++] = new XkInputChange(input, (pressedMask & (1u << input)) != 0, stamp);
        }
        Changes = changes;
    }

    /// <summary>The model that decoded the report, for <see cref="XkModel.InputName"/>.</summary>
    public XkModel Model { get; }

    /// <summary>When the report arrived; shared by every change in the group.</summary>
    public XkReportStamp Stamp { get; }

    /// <summary><see cref="XkReportKind.Input"/>, or <see cref="XkReportKind.Generated"/> for a resynchronization (see <see cref="IsResynchronization"/>).</summary>
    public XkReportKind Kind { get; }

    /// <summary>
    /// The report was an answer to Generate Data that differed from the state the decoder held, which happens only when reports were lost
    /// (the ring overflowed). The changes are real, but they happened at some unknown time before <see cref="Stamp"/>; do not use the
    /// stamp to order them against other presses.
    /// </summary>
    public bool IsResynchronization => Kind == XkReportKind.Generated;

    /// <summary>The changes, ascending by input number. Not an order in time.</summary>
    public IReadOnlyList<XkInputChange> Changes { get; }

    /// <summary>The changed inputs as bits of the input word.</summary>
    public uint ChangedMask { get; }

    /// <summary>The inputs that are down after this report, as bits of the input word.</summary>
    public uint PressedMask { get; }

    /// <summary>The inputs that are down after this report, ascending.</summary>
    public IReadOnlyList<int> PressedInputs => XkInputDecoder.Inputs(PressedMask);

    public override string ToString() => $"#{Stamp.Sequence}: {string.Join(", ", Changes.Select(c => $"{c.Input} {(c.IsPressed ? "down" : "up")}"))}{(IsResynchronization ? " (resynchronization)" : string.Empty)}";
}

/// <summary>A jog wheel step: one report per click.</summary>
/// <param name="Steps">+1 for a clockwise click, -1 for counter-clockwise (the signed jog byte).</param>
public sealed record XkJogStep(int Steps, XkReportStamp Stamp);

/// <summary>The shuttle ring moved to a new position.</summary>
/// <param name="Position">0 at rest, positive clockwise, negative counter-clockwise (+/-1 to +/-7 on the XK-12 Jog &amp; Shuttle). On release the spring overshoots briefly to the other side (measured: a -1 after a clockwise sweep).</param>
/// <param name="IsResynchronization">The position came from an answer to Generate Data after reports were lost; when it moved there is unknown.</param>
public sealed record XkShuttleMove(int Position, int PreviousPosition, XkReportStamp Stamp, bool IsResynchronization);

/// <summary>What <see cref="XkInputDecoder.Decode(ReadOnlySpan{byte}, XkReportStamp)"/> made of one report.</summary>
/// <param name="Kind">What the report was.</param>
/// <param name="IsBaseline">The report was the first of the session to carry input state: it set the initial state and changed nothing.</param>
/// <param name="Inputs">The inputs the report changed, or null if none did.</param>
/// <param name="Jog">The jog step the report carried, or null.</param>
/// <param name="Shuttle">The shuttle's new position, or null if it did not move.</param>
public readonly record struct XkDecodeResult(XkReportKind Kind, bool IsBaseline, XkInputGroup? Inputs, XkJogStep? Jog, XkShuttleMove? Shuttle);

/// <summary>
/// Turns the consecutive input reports of one device into input changes, jog steps and shuttle moves. Only the bits of the model's
/// <see cref="XkModel.InputMask"/> are inputs: jog pulses, shuttle position bits and anything else a model packs into the input bytes never
/// become input changes. <see cref="XkDevice.Start"/> runs one of these for you; use it directly with the pull API (<see cref="XkDevice.Read"/>).
/// </summary>
/// <remarks>
/// The first report that carries input state is the session's baseline and changes nothing: normally the answer to the Generate Data
/// request sent at start, but if an input report gets there first it serves instead. After the baseline, an answer to Generate Data is
/// compared like any other report; it differs only if reports were lost, and its changes are then real but flagged
/// <see cref="XkInputGroup.IsResynchronization"/> because their time is unknown. Jog steps come only from input reports. Descriptor and
/// custom reports are ignored. Not thread-safe: one device's reports, in order, from one thread at a time.
/// </remarks>
public sealed class XkInputDecoder
{
    public XkInputDecoder(XkModel model)
    {
        ArgumentNullException.ThrowIfNull(model);
        Model = model;
    }

    public XkModel Model { get; }

    /// <summary>Whether the session's initial state has arrived.</summary>
    public bool HasBaseline { get; private set; }

    /// <summary>The inputs that are down as of the last report, as bits of the input word; 0 before the baseline.</summary>
    public uint PressedMask { get; private set; }

    /// <summary>The inputs that are down as of the last report, ascending.</summary>
    public IReadOnlyList<int> PressedInputs => Inputs(PressedMask);

    /// <summary>The shuttle position as of the last report (0 for a model without one, and before the baseline).</summary>
    public int ShuttlePosition { get; private set; }

    /// <summary>Decode a report entry read from a device. An error entry decodes as <see cref="XkReportKind.Unusable"/>.</summary>
    public XkDecodeResult Decode(in XkReportEntry entry)
    {
        if (entry.IsError)
        {
            return new(XkReportKind.Unusable, false, null, null, null);
        }
        Span<byte> data = stackalloc byte[XkReportEntry.MaxDataLength];
        int length = entry.CopyDataTo(data);
        return Decode(data[..length], XkReportStamp.From(entry));
    }

    /// <summary>Decode the next report of the device; <paramref name="stamp"/> is carried into everything it yields.</summary>
    public XkDecodeResult Decode(ReadOnlySpan<byte> report, XkReportStamp stamp)
    {
        XkReportKind kind = XkReport.Classify(report);
        if (kind is not (XkReportKind.Input or XkReportKind.Generated))
        {
            return new(kind, false, null, null, null);
        }
        uint state = XkReport.InputWord(report) & Model.InputMask;
        XkJogStep? jog = null;
        XkShuttleMove? shuttle = null;
        if (Model.JogShuttle is { } layout)
        {
            if (kind == XkReportKind.Input && report.Length > layout.JogIndex && report[layout.JogIndex] != 0)
            {
                jog = new XkJogStep((sbyte)report[layout.JogIndex], stamp);
            }
            if (report.Length > layout.ShuttleIndex)
            {
                int position = (sbyte)report[layout.ShuttleIndex];
                if (HasBaseline && position != ShuttlePosition)
                {
                    shuttle = new XkShuttleMove(position, ShuttlePosition, stamp, kind == XkReportKind.Generated);
                }
                ShuttlePosition = position;
            }
        }
        if (!HasBaseline)
        {
            HasBaseline = true;
            PressedMask = state;
            return new(kind, true, null, jog, null);
        }
        uint changed = state ^ PressedMask;
        PressedMask = state;
        return new(kind, false, changed == 0 ? null : new XkInputGroup(Model, stamp, kind, changed, state), jog, shuttle);
    }

    /// <summary>Forget the state, as when the session ends. Returns the inputs that were down, which the caller may release.</summary>
    public IReadOnlyList<int> Reset()
    {
        IReadOnlyList<int> held = HasBaseline ? Inputs(PressedMask) : [];
        HasBaseline = false;
        PressedMask = 0;
        ShuttlePosition = 0;
        return held;
    }

    internal static int[] Inputs(uint mask)
    {
        int[] inputs = new int[System.Numerics.BitOperations.PopCount(mask)];
        int i = 0;
        for (uint bits = mask; bits != 0; bits &= bits - 1)
        {
            inputs[i++] = System.Numerics.BitOperations.TrailingZeroCount(bits);
        }
        return inputs;
    }
}
