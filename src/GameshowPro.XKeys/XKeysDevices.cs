using GameshowPro.XKeys.Hid;

namespace GameshowPro.XKeys;

/// <summary>
/// Builds the list of <see cref="XKeysDevice{TTriggerKey}"/> an application uses, and keeps it attached to the physical devices as they
/// come and go (see <see cref="GetDevices(IList{XKeysDeviceSettings}, ILoggerFactory)"/>).
/// </summary>
/// <remarks>
/// <para>
/// <b>Order, and choosing a device by index.</b> The list starts with the devices present when it is built, in the library's device order
/// (<see cref="XkDeviceDescriptor.Order"/>: by product ID, then by device interface path, compared ordinally and case-insensitively), so
/// index 0 is the present device that sorts first; it goes on with one disconnected device for each settings object that no present device
/// took. The list is fixed: each device keeps its index for the life of the list, and a device that is unplugged and plugged in again is
/// attached to the same entry (the one last attached to the same interface path). To let a user pick "the second X-keys", take index 1 of
/// this list, or of <see cref="XkDeviceMonitor.PresentDevices"/> or <see cref="XkEnumerator.Enumerate"/>, which use the same order.
/// </para>
/// <para>
/// What is stable: the same devices on the same USB ports give the same order after a restart, a new enumeration or an unplug and replug
/// on the same port, because the path of a USB device without a serial number embeds an instance ID that Windows derives from the port.
/// What is not: adding or removing a device shifts every device that sorts after it (between two builds of the list); moving a device to
/// another port or hub can change its place among devices with the same product ID; changing a device's PID mode changes its product ID
/// and so its place. See <see cref="XkEnumerator.Enumerate"/>.
/// </para>
/// </remarks>
public static class XKeysDevices<TTriggerKey>
    where TTriggerKey : notnull, Enum
{
    private const int DefaultMinDevices = 1;
    private const int DefaultMaxDevices = 5;
    private static readonly LedState[,] s_ledIdentifiers = new LedState[,]
        {
            { LedState.On, LedState.Off },
            { LedState.Off, LedState.On },
            { LedState.On, LedState.On },
            { LedState.Off, LedState.Off }
        };

    /// <summary>
    /// Get a list of wrapped x-keys devices, the first of which will be usable with the supplied list of triggers. Intended for backwards compatibility. Other overload is required for multi-device support.
    /// See the remarks of <see cref="XKeysDevices{TTriggerKey}"/> for the order of the list.
    /// </summary>
    /// <param name="minCount">The minimum number of devices to create. If fewer than this number are discovered, dummy devices will be created.</param>
    /// <param name="deviceSettings">A list of trigger settings which correspond to a discovered device.</param>
    public static ImmutableList<XKeysDevice<TTriggerKey>> GetDevices(int minCount, XKeysDeviceSettings deviceSettings, ILoggerFactory loggerFactory)
        => GetDevices(minCount, Math.Max(minCount, DefaultMaxDevices), [deviceSettings], loggerFactory, XKeysDeviceOptions.Default);

    /// <summary>
    /// Get a list of wrapped x-keys devices, zipping together the list of discovered devices with the corresponding list of trigger lists.
    /// The devices present now come first, in the library's device order, and are connected when this returns (unless one could not be
    /// opened); see the remarks of <see cref="XKeysDevices{TTriggerKey}"/> for the order and what is stable. From then on devices are
    /// attached as they arrive and leave, with nothing polled.
    /// </summary>
    /// <param name="devicesSettings">A list containing an lists of trigger settings which each correspond to a discovered device. Settings are added for a discovered device that matches none.</param>
    public static ImmutableList<XKeysDevice<TTriggerKey>> GetDevices(IList<XKeysDeviceSettings> devicesSettings, ILoggerFactory loggerFactory)
        => GetDevices(DefaultMinDevices, DefaultMaxDevices, devicesSettings, loggerFactory, XKeysDeviceOptions.Default);

    /// <summary>For tests: as the public overloads, with control over what the devices send on their own.</summary>
    internal static ImmutableList<XKeysDevice<TTriggerKey>> GetDevices(IList<XKeysDeviceSettings> devicesSettings, ILoggerFactory loggerFactory, XKeysDeviceOptions options)
        => GetDevices(DefaultMinDevices, DefaultMaxDevices, devicesSettings, loggerFactory, options);

    /// <param name="minCount">The minimum number of devices to create. If fewer than this number are discovered, dummy devices will be created.</param>
    /// <param name="maxCount">The maximum number of devices built from discovered devices. This is mainly intended to protect from runaway settings.</param>
    private static ImmutableList<XKeysDevice<TTriggerKey>> GetDevices(int minCount, int maxCount, IList<XKeysDeviceSettings> devicesSettings, ILoggerFactory loggerFactory, XKeysDeviceOptions options)
    {
        HashSet<XKeysDeviceSettings> settingsStack = [.. devicesSettings];
        ImmutableList<XKeysDevice<TTriggerKey>>.Builder devList = ImmutableList.CreateBuilder<XKeysDevice<TTriggerKey>>();
        ILogger logger = loggerFactory.CreateLogger("XKeysDevices");
        XKeysDeviceManager<TTriggerKey> manager = new(logger);
        IReadOnlyList<XkDeviceDescriptor> discoveredDevices = [];
        string? unavailableReason = null;
        if (!XkHidLibrary.TryLoad(out unavailableReason))
        {
            logger.LogError("X-keys devices will not be found: {reason}", unavailableReason);
        }
        else
        {
            try
            {
                // The monitor enumerates once here, in the library's device order, and from then on opens what arrives.
                discoveredDevices = manager.Start();
            }
            catch (XkHidException ex)
            {
                logger.LogError(ex, "X-keys devices will not be found");
                unavailableReason = ex.Message;
            }
        }
        logger.LogTrace("XKeys enumerated {discoCount} devices", discoveredDevices.Count);
        try
        {
            for (int index = 0; index < discoveredDevices.Count; index++)
            {
                XkDeviceDescriptor dev = discoveredDevices[index];
                string name = XkModel.ForPid(dev.Pid).NameForPid(dev.Pid);
                logger.LogTrace("Device {deviceNumber}: {deviceName}", index, name);
                if (devList.Count >= maxCount)
                {
                    logger.LogTrace("Device {deviceNumber}: ignored because maximum device count has already been reached", index);
                    continue;
                }
                //1st choice - full pid match
                XKeysDeviceSettings? deviceSettings = settingsStack.FirstOrDefault(s => s.Pid == dev.Pid);

                if (deviceSettings == null)
                {
                    //2nd choice - unset pid in settings
                    deviceSettings = settingsStack.FirstOrDefault(s => s.Pid == 0);
                    logger.LogTrace("Device {deviceNumber}: Used settings with generic pid", index);
                }
                else
                {
                    logger.LogTrace("Device {deviceNumber}: Used settings with specific pid", index);
                }
                if (deviceSettings == null)
                {
                    //3rd choice - create a new device
                    deviceSettings = new(dev.Pid);
                    devicesSettings.Add(deviceSettings);
                    logger.LogTrace("Device {deviceNumber}: Created new settings", index);
                }
                else
                {
                    //In case of unset Pid, it will be set for future persistence
                    deviceSettings.Pid = dev.Pid;
                    settingsStack.Remove(deviceSettings);
                }
                // The monitor's session for this interface is attached to this device (by its path) as soon as the list is complete.
                devList.Add(new(name, deviceSettings, loggerFactory) { Options = options, LastPath = dev.Path });
            }
            int requiredForMin = minCount - devList.Count - settingsStack.Count;
            if (requiredForMin > 0)
            {
                logger.LogTrace("{requiredForMin} new settings will be created to fulfil minimum requirement.", requiredForMin);
                for (int i = 0; i < requiredForMin; i++)
                {
                    XKeysDeviceSettings deviceSettings = new();
                    devicesSettings.Add(deviceSettings);
                    settingsStack.Add(deviceSettings);
                }
            }
            logger.LogTrace("{settingsStackCount} extra settings will be allocated to dummy devices.", settingsStack.Count);
            foreach (XKeysDeviceSettings deviceSettings in settingsStack)
            {
                string name = XkModel.ForPid((ushort)deviceSettings.Pid).NameForPid((ushort)deviceSettings.Pid);
                devList.Add(new($"Not found: {name}", deviceSettings, loggerFactory) { Options = options });
            }
            foreach (XKeysDevice<TTriggerKey> device in devList)
            {
                device.UnavailableReason = unavailableReason;
                device.UpdateState();
            }
        }
        finally
        {
            manager.SetDevices(devList.ToImmutable());
        }
        ImmutableList<XKeysDevice<TTriggerKey>> devices = devList.ToImmutable();
        if (!manager.WaitForInitialSessions())
        {
            logger.LogWarning("Not every X-keys device present at start was opened within {Timeout}; they are attached when they open", XKeysDeviceManager<TTriggerKey>.InitialOpenTimeout);
        }
        if (options.Identify)
        {
            // Each discovered device shows which entry it is; applied when it connects if it has not yet.
            for (int i = 0; i < devices.Count && devices[i].LastPath is not null; i++)
            {
                int row = Math.Min(i, s_ledIdentifiers.GetUpperBound(0));
                devices[i].SetLeds(s_ledIdentifiers[row, 0], s_ledIdentifiers[row, 1]);
            }
        }
        return devices;
    }
}
