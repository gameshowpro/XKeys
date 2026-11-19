// Copyright (c) Barjonas LLC 2026.
namespace GameshowPro.XKeys.Hid;

/// <summary>State of an indicator LED, digital output or backlight (vendor "LED State" / "State": 0 off, 1 on, 2 flash).</summary>
public enum XkLedState : byte
{
    Off = 0,
    On = 1,
    /// <summary>Flashes at the frequency set with <see cref="XkLightReports.SetFlashFrequency"/>.</summary>
    Flash = 2,
}

/// <summary>
/// What command 179 ("Index Based Set LED (Flash)") can address. Sources: the vendor SDK pages "X-keys XK-12 Jog &amp; Shuttle Data
/// Report" (LEDIndex: 6 = green, 7 = red) and "X-keys XK-12 XK-3 Switch Interface Data Report" (LEDIndex: 0 = Out 1, 1 = Out 2, 6 =
/// green LED, 7 = red LED).
/// </summary>
public enum XkLedIndex : byte
{
    /// <summary>The Switch Interface's first additional digital output. Not on the Jog &amp; Shuttle.</summary>
    Output1 = 0,
    /// <summary>The Switch Interface's second additional digital output. Not on the Jog &amp; Shuttle.</summary>
    Output2 = 1,
    Green = 6,
    Red = 7,
}

/// <summary>What the vendor documentation says a model's lights can do. See <see cref="XkLights.CapabilitiesForPid"/>.</summary>
/// <param name="Indicators">Green and red indicator LEDs (<see cref="XkLedIndex.Green"/>, <see cref="XkLedIndex.Red"/>).</param>
/// <param name="SetBothIndicatorsCommand">Command 186 (both indicators in one report, no flash) is documented for the model.</param>
/// <param name="Backlights">Two banks of key backlights with intensity, rows, toggle and per-key control.</param>
/// <param name="DigitalOutputs">Two additional digital outputs (<see cref="XkLedIndex.Output1"/>, <see cref="XkLedIndex.Output2"/>).</param>
public sealed record XkLightCapabilities(bool Indicators, bool SetBothIndicatorsCommand, bool Backlights, bool DigitalOutputs);

/// <summary>
/// Exact output reports for the lighting commands, as pure functions: each returns a new array of the report length (36 for every
/// X-keys model this library handles; pass <see cref="XkDevice.OutputLength"/>), index 0 being the report ID 0, index 1 the command,
/// the rest as documented and zero. Nothing is sent and no state is kept. Write one with <see cref="XkDevice.Write"/>, or use
/// <see cref="XkLights"/>.
/// </summary>
/// <remarks>
/// Sources are the vendor's SDK documentation (github.com/piengineering/PI-Engineering-SDK, Documentation/pages): "X-keys XK-12 Jog
/// &amp; Shuttle Data Report" (J&amp;S) and "X-keys XK-12 XK-3 Switch Interface Data Report" (SI), section "Output Report", whose
/// numbering of the commands is quoted below. Every command here is volatile: none is marked by the vendor as writing to the EEPROM
/// (its "e" marker), and a replug or reboot returns the lights to the device's stored state.
/// <para>
/// Deliberately not provided, because they write to the EEPROM (50,000 writes) or change the device's identity or behavior: Set Unit ID
/// (189), Save Backlight State to EEPROM (199), Change PID (204), Reboot (238), Enable Time Stamp (210), and Enable Scroll Lock to
/// Toggle Backlights (183). They remain available as raw reports through <see cref="XkDevice.Write"/>.
/// </para>
/// <para>
/// Byte layouts (index: value): <c>SetLed</c> 0:0 1:179 2:LEDIndex 3:State; <c>SetIndicatorBits</c> 0:0 1:186 2:LEDs (0x40 green,
/// 0x80 red); <c>SetFlashFrequency</c> 0:0 1:180 2:Freq; <c>SetBacklight</c> 0:0 1:181 2:Index 3:State; <c>SetBacklightRows</c> 0:0
/// 1:182 2:Bank 3:Rows; <c>ToggleBacklights</c> 0:0 1:184; <c>SetBacklightIntensity</c> 0:0 1:187 2:Bank1 3:Bank2.
/// </para>
/// </remarks>
public static class XkLightReports
{
    /// <summary>The output report length of every supported model, report ID included.</summary>
    public const int DefaultReportLength = 36;

    public const byte CommandSetLed = 179;
    public const byte CommandSetFlashFrequency = 180;
    public const byte CommandSetBacklight = 181;
    public const byte CommandSetBacklightRows = 182;
    public const byte CommandToggleBacklights = 184;
    public const byte CommandSetIndicatorBits = 186;
    public const byte CommandSetBacklightIntensity = 187;

    /// <summary>
    /// Index Based Set LED (Flash), command 179: report <c>0, 179, LEDIndex, State</c>. J&amp;S report 2, SI report 1. Volatile.
    /// Indexes: green 6 and red 7 on both models; 0 and 1 are the SI's digital outputs.
    /// </summary>
    public static byte[] SetLed(XkLedIndex led, XkLedState state, int reportLength = DefaultReportLength)
    {
        if (!Enum.IsDefined(led))
        {
            throw new ArgumentOutOfRangeException(nameof(led));
        }
        ValidateState(state);
        return Make(reportLength, CommandSetLed, (byte)led, (byte)state);
    }

    /// <summary>
    /// Set LEDs, command 186: report <c>0, 186, LEDs</c>, where bit 7 (0x40) lights the green LED and bit 8 (0x80) the red one, all other
    /// bits 0. Sets both at once, on or off only (use <see cref="SetLed"/> to flash). Documented for the J&amp;S (report 1); the SI page
    /// does not list it. Volatile.
    /// </summary>
    public static byte[] SetIndicatorBits(bool green, bool red, int reportLength = DefaultReportLength)
        => Make(reportLength, CommandSetIndicatorBits, (byte)((green ? 0x40 : 0) | (red ? 0x80 : 0)));

    /// <summary>
    /// Set Frequency of Flash, command 180: report <c>0, 180, Freq</c>, Freq 1 (fastest) to 255 (slowest, about 4 seconds between flashes).
    /// One frequency serves the indicators and the backlights. J&amp;S report 12, SI report 3. Volatile.
    /// </summary>
    public static byte[] SetFlashFrequency(byte frequency, int reportLength = DefaultReportLength)
    {
        if (frequency == 0)
        {
            throw new ArgumentOutOfRangeException(nameof(frequency), "The flash frequency runs from 1 (fastest) to 255.");
        }
        return Make(reportLength, CommandSetFlashFrequency, frequency);
    }

    /// <summary>
    /// Index Based Set Backlights (Flash), command 181: report <c>0, 181, Index, State</c>. J&amp;S report 11. Volatile.
    /// The vendor's index is the key number of its Figure 1 for bank 1 and that plus 32 for bank 2. For the Jog &amp; Shuttle Figure 1
    /// numbers the keys by column then row: top row 0, 8, 16, 24; middle row 1, 9, 17, 25; bottom row 2, 10, 18, 26, the same
    /// numbers as the key inputs. (The page's prose example, "lower left key bank 1 index=5", does not fit that figure; the figure is followed here.)
    /// </summary>
    /// <param name="bank">0 for bank 1, 1 for bank 2 (the two backlight colors).</param>
    /// <param name="keyIndex">0 to 31, the key number of the vendor's Figure 1.</param>
    public static byte[] SetBacklight(int bank, int keyIndex, XkLedState state, int reportLength = DefaultReportLength)
    {
        ValidateBank(bank);
        ArgumentOutOfRangeException.ThrowIfNegative(keyIndex);
        ArgumentOutOfRangeException.ThrowIfGreaterThan(keyIndex, 31);
        ValidateState(state);
        return Make(reportLength, CommandSetBacklight, (byte)((bank * 32) + keyIndex), (byte)state);
    }

    /// <summary>
    /// Turn On/Off Rows of Backlights, command 182: report <c>0, 182, Bank, OnOff</c>. J&amp;S report 10. Volatile; intensities are unaffected.
    /// </summary>
    /// <param name="bank">0 for bank 1, 1 for bank 2.</param>
    /// <param name="rows">Bit 0 = 1st row, bit 1 = 2nd row, ... bit 5 = 6th row; 1 for backlight on. The Jog &amp; Shuttle has 3 rows.</param>
    public static byte[] SetBacklightRows(int bank, byte rows, int reportLength = DefaultReportLength)
    {
        ValidateBank(bank);
        if (rows > 0x3F)
        {
            throw new ArgumentOutOfRangeException(nameof(rows), "Only bits 0 to 5 (six rows) are defined.");
        }
        return Make(reportLength, CommandSetBacklightRows, (byte)bank, rows);
    }

    /// <summary>Toggle Backlights, command 184: report <c>0, 184</c>. J&amp;S report 8. Volatile.</summary>
    public static byte[] ToggleBacklights(int reportLength = DefaultReportLength)
        => Make(reportLength, CommandToggleBacklights);

    /// <summary>
    /// Set Backlight Intensity, command 187: report <c>0, 187, Bank 1 Intensity, Bank 2 Intensity</c>, each 0 (off) to 255 (brightest). J&amp;S report 7. Volatile.
    /// </summary>
    public static byte[] SetBacklightIntensity(byte bank1, byte bank2, int reportLength = DefaultReportLength)
        => Make(reportLength, CommandSetBacklightIntensity, bank1, bank2);

    private static byte[] Make(int reportLength, byte command, byte argument1 = 0, byte argument2 = 0)
    {
        ArgumentOutOfRangeException.ThrowIfLessThan(reportLength, 4);
        byte[] report = new byte[reportLength];
        report[1] = command;
        report[2] = argument1;
        report[3] = argument2;
        return report;
    }

    private static void ValidateState(XkLedState state)
    {
        if (!Enum.IsDefined(state))
        {
            throw new ArgumentOutOfRangeException(nameof(state));
        }
    }

    private static void ValidateBank(int bank)
    {
        if (bank is not (0 or 1))
        {
            throw new ArgumentOutOfRangeException(nameof(bank), "The bank is 0 (bank 1) or 1 (bank 2).");
        }
    }
}

/// <summary>
/// The lights of one open <see cref="XkDevice"/>: each method builds the matching <see cref="XkLightReports"/> report, writes it
/// synchronously with <see cref="XkDevice.Write"/> and returns the <see cref="XkError"/> code (<see cref="XkError.Ok"/> on success;
/// <see cref="XkError.DeviceRemoved"/> if the device has been closed). No state is kept here and nothing runs in the background, so
/// the lights are what the device says they are; a device that is unplugged and reopened comes back in its stored state. Only volatile
/// commands are offered; see <see cref="XkLightReports"/>. Methods do not refuse a command the model does not document (see
/// <see cref="Capabilities"/>); the device decides what it does with it.
/// </summary>
public sealed class XkLights
{
    private readonly XkDevice _device;

    public XkLights(XkDevice device)
    {
        ArgumentNullException.ThrowIfNull(device);
        _device = device;
    }

    /// <summary>What the vendor documentation says this device's model has, or null if the model is not one whose pages were checked.</summary>
    public XkLightCapabilities? Capabilities => CapabilitiesForPid(_device.Pid);

    /// <summary>
    /// Capabilities by PID, from the vendor's SDK pages: the XK-12 Jog &amp; Shuttle (0x0426 to 0x0428) has indicators, command 186 and two
    /// backlight banks; the XK-12 Switch Interface (0x04A8 to 0x04AB) and XK-3 Switch Interface (0x04C5 to 0x04C8) have indicators and two
    /// digital outputs, and no backlights. Null for any other PID.
    /// </summary>
    /// <remarks>The same as <see cref="XkModel.Lights"/> of <see cref="XkModel.ForPid"/>.</remarks>
    public static XkLightCapabilities? CapabilitiesForPid(ushort pid) => XkModel.ForPid(pid).Lights;

    /// <summary>One indicator LED or digital output (command 179).</summary>
    public int SetLed(XkLedIndex led, XkLedState state, int timeoutMilliseconds = DefaultTimeoutMilliseconds)
        => Send(XkLightReports.SetLed(led, state, _device.OutputLength), timeoutMilliseconds);

    /// <summary>Both indicator LEDs, off, on or flashing, as two writes (command 179 twice); returns the first failure.</summary>
    public int SetIndicators(XkLedState green, XkLedState red, int timeoutMilliseconds = DefaultTimeoutMilliseconds)
    {
        int result = SetLed(XkLedIndex.Green, green, timeoutMilliseconds);
        return result != XkError.Ok ? result : SetLed(XkLedIndex.Red, red, timeoutMilliseconds);
    }

    /// <summary>Both indicator LEDs, on or off, in one write (command 186; documented for the Jog &amp; Shuttle).</summary>
    public int SetIndicatorBits(bool green, bool red, int timeoutMilliseconds = DefaultTimeoutMilliseconds)
        => Send(XkLightReports.SetIndicatorBits(green, red, _device.OutputLength), timeoutMilliseconds);

    /// <summary>The flash frequency of indicators and backlights, 1 (fastest) to 255 (command 180).</summary>
    public int SetFlashFrequency(byte frequency, int timeoutMilliseconds = DefaultTimeoutMilliseconds)
        => Send(XkLightReports.SetFlashFrequency(frequency, _device.OutputLength), timeoutMilliseconds);

    /// <summary>One key's backlight in bank 0 or 1 (command 181).</summary>
    public int SetBacklight(int bank, int keyIndex, XkLedState state, int timeoutMilliseconds = DefaultTimeoutMilliseconds)
        => Send(XkLightReports.SetBacklight(bank, keyIndex, state, _device.OutputLength), timeoutMilliseconds);

    /// <summary>Rows of one bank's backlights on or off, bit 0 = first row (command 182).</summary>
    public int SetBacklightRows(int bank, byte rows, int timeoutMilliseconds = DefaultTimeoutMilliseconds)
        => Send(XkLightReports.SetBacklightRows(bank, rows, _device.OutputLength), timeoutMilliseconds);

    /// <summary>Toggle the backlights (command 184).</summary>
    public int ToggleBacklights(int timeoutMilliseconds = DefaultTimeoutMilliseconds)
        => Send(XkLightReports.ToggleBacklights(_device.OutputLength), timeoutMilliseconds);

    /// <summary>The intensity of each backlight bank, 0 to 255 (command 187).</summary>
    public int SetBacklightIntensity(byte bank1, byte bank2, int timeoutMilliseconds = DefaultTimeoutMilliseconds)
        => Send(XkLightReports.SetBacklightIntensity(bank1, bank2, _device.OutputLength), timeoutMilliseconds);

    public const int DefaultTimeoutMilliseconds = 1000;

    private int Send(byte[] report, int timeoutMilliseconds)
    {
        try
        {
            return _device.Write(report, timeoutMilliseconds, out _);
        }
        catch (ObjectDisposedException)
        {
            return XkError.DeviceRemoved;
        }
    }
}
