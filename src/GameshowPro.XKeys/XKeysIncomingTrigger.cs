using GameshowPro.XKeys.Hid;

namespace GameshowPro.XKeys;

/// <summary>
/// One trigger of an <see cref="XKeysDevice{TTriggerKey}"/>: input <c>Setting.Id</c> of the device (bit <c>Id % 8</c> of report index
/// <c>3 + Id / 8</c>, the numbering of <see cref="XkInput.Number"/>).
/// </summary>
/// <remarks>
/// <para>
/// <b>Time.</b> When the trigger fires because an input report changed its input, <see cref="IncomingTrigger.Time"/> is set, before
/// <see cref="IncomingTrigger.Triggered"/> is raised, to the moment that report arrived: its native performance-counter stamp
/// (<see cref="XkReportStamp.Timestamp"/>) as a <see cref="TimeSpan"/> on the <see cref="System.Diagnostics.Stopwatch"/> clock (the time
/// since that clock's origin, usually boot; <see cref="XkReportStamp.ToTimeSpan"/>). The unit is the one other GameshowPro devices use for
/// <see cref="IncomingTrigger.Time"/> (a high-precision <see cref="TimeSpan"/>), but the origin is the Stopwatch clock's, not the start of a
/// faceoff: the difference between two triggers' times is how far apart their reports arrived, comparable across every X-keys device in
/// the process. Inputs that changed in the same report get exactly the same time, and inputs from different reports different times
/// (the stamp has 100 ns resolution), so, as with TriggerGate's faceoff times, equal times mean "arrived together, cannot be ordered".
/// X-keys devices run no faceoff, so <see cref="IncomingTrigger.Ordinal"/> and <see cref="IncomingTrigger.IsTied"/> are never set; a
/// consumer that ranks presses compares times. A trigger that fires for any other reason (the WPF simulate command) sets the time to null.
/// </para>
/// <para>
/// <b>State without a press.</b> <see cref="IncomingTrigger.IsDown"/> also follows state that is not an input change (the device's answer to
/// Generate Data at connection and after <see cref="XKeysDevice{TTriggerKey}.RequestUpdate"/>, and the release of every input when the device
/// goes away), but such state never fires the trigger: only an input report does.
/// </para>
/// </remarks>
public class XKeysIncomingTrigger : IncomingTrigger
{
    private readonly IncomingTriggerSetting _setting;
    private int _byteIndex;
    private byte _bitMask;
    // Set while a change from an input report is applied, so that OnVerifiedTrigger can stamp the trigger.
    private TimeSpan? _pendingTime;
    // Set while state that is not an input change is applied, so that the base class does not fire the trigger.
    private bool _applyingState;

    public XKeysIncomingTrigger(IncomingTriggerSetting setting, IIncomingTriggerDeviceBase parent, ILogger logger) : base(setting, parent, logger)
    {
        setting.TriggerFilter = TriggerFilter.All;
        _setting = setting;
        UpdateFromId();
        _setting.PropertyChanged += (s, e) => UpdateFromId();
    }

    /// <summary>The base class fires the trigger on a change of <see cref="IncomingTrigger.IsDown"/>, except while state that is not an input change is applied.</summary>
    public override bool BaseClassDetectsEdges => !_applyingState;

    /// <summary>
    /// The device's own millisecond timestamp (<see cref="XkReportStamp.DeviceMilliseconds"/>) of the report that last changed this input, or
    /// null before the first change or when the device's reports carry none. Set on every change, press or release, whether or not it fires
    /// the trigger. It counts from when the device was plugged in and runs slightly slow against the host clock, so it orders reports of one
    /// device over short spans but is not comparable across devices; <see cref="IncomingTrigger.Time"/> is.
    /// </summary>
    public uint? DeviceMilliseconds
    {
        get;
        private set { _ = SetProperty(ref field, value); }
    }

    /// <summary>
    /// The readable name of this trigger's input on the device's model (<see cref="XkModel.InputName"/>), for example "key 1 (top left)" or
    /// "socket 3 tip (button)"; "input n" for an input the model does not list.
    /// </summary>
    public string InputName => (ModelProvider?.Invoke() ?? s_genericModel).InputName(_setting.Id);

    private static readonly XkModel s_genericModel = XkModel.ForPid(0);

    /// <summary>Gives the model of the device this trigger belongs to; set by <see cref="XKeysDevice{TTriggerKey}"/>.</summary>
    internal Func<XkModel>? ModelProvider { get; set; }

    private void UpdateFromId()
    {
        _byteIndex = (_setting.Id / 8) + XkReport.FirstInputIndex;
        _bitMask = (byte)(1 << (_setting.Id % 8));
    }

    /// <summary>
    /// Set <see cref="IncomingTrigger.IsDown"/> from a raw input report (index 0 being the report ID), without a time. The device no longer
    /// calls this: it applies decoded changes, with their times (see the remarks of <see cref="XKeysIncomingTrigger"/>). A change made here
    /// fires the trigger as before, with <see cref="IncomingTrigger.Time"/> null.
    /// </summary>
    public void UpdateFromData(byte[] data)
    {
        if (_byteIndex >= data.Length)
        {
            // A trigger ID beyond the report cannot be pressed.
            return;
        }
        IsDown = (data[_byteIndex] & _bitMask) > 0;
    }

    /// <summary>Apply a change that an input report made: record its device timestamp, and stamp the trigger with the report's time if it fires.</summary>
    internal void ApplyChange(bool isPressed, XkReportStamp stamp)
    {
        DeviceMilliseconds = stamp.DeviceMilliseconds;
        _pendingTime = stamp.ToTimeSpan();
        try
        {
            IsDown = isPressed;
        }
        finally
        {
            _pendingTime = null;
        }
    }

    /// <summary>Set <see cref="IncomingTrigger.IsDown"/> to state that is not an input change (initial state, a resynchronization, a release because the device went away). Never fires the trigger.</summary>
    internal void ApplyState(bool isDown)
    {
        _applyingState = true;
        try
        {
            IsDown = isDown;
        }
        finally
        {
            _applyingState = false;
        }
    }

    /// <summary>Mark the input as released, for when the device is gone and can no longer say so. Does not fire the trigger.</summary>
    internal void Release() => ApplyState(false);

    protected override void OnVerifiedTrigger(object? source, TriggerArgs args)
    {
        // Set before Triggered is raised, so that its handlers see the time of this trigger.
        Time = _pendingTime;
        base.OnVerifiedTrigger(source, args);
    }
}
