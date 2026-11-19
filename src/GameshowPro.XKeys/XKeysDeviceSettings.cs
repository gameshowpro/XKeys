using Newtonsoft.Json;

namespace GameshowPro.XKeys;

[method: JsonConstructor]
public class XKeysDeviceSettings(int? pid, IncomingTriggerSettings? incomingTriggerSettings, bool? allowDuplicateTriggerIds, string? nameSuffix, bool? isEnabled, RemoteServiceSettings? remoteServiceSettings) 
    : IncomingTriggerDeviceSettingsBase(incomingTriggerSettings ?? [], allowDuplicateTriggerIds, nameSuffix ?? string.Empty, isEnabled, remoteServiceSettings)
{
    public XKeysDeviceSettings() : this(null, null, null, null, null, null)
    { }
    public XKeysDeviceSettings(int pid) : this(pid, null, null, null, null, null)
    { }

    [JsonProperty]
    public int Pid { get; internal set; } = pid ?? 0;
}
