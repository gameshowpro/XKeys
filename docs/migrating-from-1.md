# Migrating an application from GameshowPro.XKeys 1.x

Version 2 replaces the vendor's `PIEHidNetCore.dll` with our own native HID layer (see the [README](../README.md) for why), and
from 2.1 the library comes as two packages built from the same code:

- **GameshowPro.XKeys.Windows**: for WPF applications. It has the WPF resource dictionary
  (`GameshowPro.XKeys;component/View/Template.xaml`) and depends on the Windows flavor of GameshowPro.Common
  (`GameshowPro.Common.JsonNet.Windows`).
- **GameshowPro.XKeys**: the same library without WPF, for any other .NET application (a service, a console program, an ASP.NET
  app). It depends on the cross-platform flavor of GameshowPro.Common (`GameshowPro.Common.JsonNet`).

A 1.x application used the library with WPF, so it moves to **GameshowPro.XKeys.Windows 2.1 or later**. For an application that
uses the library the way it was designed to be used (`XKeysDevices<T>.GetDevices`, `XKeysDevice<T>`, the triggers and the WPF
template), the migration is a change of package and a rebuild. This page lists what to change, what behaves differently, and what to
check afterwards.

## Requirements

| | 1.x | 2.1 |
|---|---|---|
| Package | `GameshowPro.XKeys` | `GameshowPro.XKeys.Windows` (WPF), or `GameshowPro.XKeys` (no WPF) |
| Target framework | `net10.0-windows` (earlier 1.x: .NET 9, 8) | `net11.0-windows` for the WPF package; `net11.0` or later for the other |
| Operating system and process | Windows, any architecture | **Windows x64 only**, with either package: the native library `xkhid.dll` is built for Windows x64 |
| GameshowPro.Common | 1.4.10 | 1.4.14 or later, in the flavor that matches the package |
| Files beside the application | `PIEHidNetCore.dll` | `GameshowPro.XKeys.dll` and `runtimes\win-x64\native\xkhid.dll` (or `xkhid.dll` beside the executable when published for `win-x64`) |

An application that cannot move to .NET 11 stays on the 1.x version it already references; 1.x keeps working but keeps the
vendor library's memory leak.

## Steps

1. Change the package reference to `<PackageReference Include="GameshowPro.XKeys.Windows" Version="2.1.*" />` (a WPF
   application), or keep the ID `GameshowPro.XKeys` and raise it to 2.1 for an application without WPF (see "Without WPF" below).
2. If the application references `GameshowPro.Common` packages directly, raise them to at least 1.4.14, in the same flavor (the
   `.Windows` packages with `GameshowPro.XKeys.Windows`), or NuGet reports a downgrade or a conflict.
3. Remove anything that mentions `PIEHidNetCore`: `using PIEHidNetCore;`, a `<Reference>` to the DLL, copy steps, installer or
   packaging entries for `PIEHidNetCore.dll`.
4. Make the process 64-bit (`<PlatformTarget>x64</PlatformTarget>` or a `win-x64` runtime identifier) if it is not already.
5. Make sure installers and packaging pick up the native file. A normal build or publish copies it; a hand-written file list
   will miss it. Without `xkhid.dll` the devices appear as "Not found" with a status text that names the missing DLL; nothing
   throws.
6. Rebuild. The compile errors, if any, will be from the API changes below.

### Without WPF

`GameshowPro.XKeys` has no WPF in it, but it is still Windows-only at run time, and its assembly says so with
`[SupportedOSPlatform("windows")]`. A project that targets plain `net11.0` therefore gets platform warnings (CA1416) wherever it
uses the library, unless it declares that it runs on Windows too. Either target `net11.0-windows`, or add to the project:

```xml
<ItemGroup>
  <AssemblyAttribute Include="System.Runtime.Versioning.SupportedOSPlatformAttribute">
    <_Parameter1>windows</_Parameter1>
  </AssemblyAttribute>
</ItemGroup>
```

(or `[assembly: System.Runtime.Versioning.SupportedOSPlatform("windows")]` in a source file), or guard each use with
`OperatingSystem.IsWindows()`. A project that also builds for another operating system (TriggerGate's service also builds for
Linux on ARM, for example) references the package only in its Windows build.

## API changes

| Member | 1.x | 2.1 | What to do |
|---|---|---|---|
| `XKeysDevice<T>.Pid` | `byte`, and it held the wrong value (the PID mode, e.g. 0 or 3) | `ushort`, the real USB product ID (e.g. `0x0426`) | Change the type of anything that stores it; do not compare it with values saved by 1.x |
| `XKeysDevice<T>.PidMode` | did not exist | `byte`: which of the model's product IDs the device is using | Use this if the old `Pid` value was what you wanted |
| `XKeysDevice<T>.HandlePIEHidError(PIEDevice, int)` | public | removed | Nothing should have called it |
| `XKeysDevice<T>.Lights` | did not exist | `XkLights?`: every indicator light and backlight; null while disconnected | Optional. `SetLeds` still works as before |
| `XKeysDevice<T>.Model`, `.Descriptor` | did not exist | the catalogue entry of the device's model (`XkModel`: name, inputs and their names, measured timing) and the interface it is attached to | Optional |
| `XKeysIncomingTrigger.Time` (from `IncomingTrigger`) | never set | the time of the report that fired the trigger; see "Times on triggers" below | Optional |
| `XKeysIncomingTrigger.DeviceMilliseconds`, `.InputName` | did not exist | the device's own millisecond timestamp of the input's last change (nullable), and the model's name for the input | Optional |
| Types from `PIEHidNetCore` (`PIEDevice`, `PIEDataHandler`, ...) | reachable | gone | For raw device access use the namespace `GameshowPro.XKeys.Hid`, in the same package (`XkDeviceMonitor`, `XkDevice`, `XkEnumerator`, `XkModel`, `XkLights`) |

Unchanged: `XKeysDevices<T>.GetDevices` (both overloads), `XKeysDeviceSettings` and its saved format, `SetLeds`, `SetUnitId`,
`RequestUpdate`, the WPF resource dictionary path (`GameshowPro.XKeys;component/View/Template.xaml`, in
`GameshowPro.XKeys.Windows`) and its template keys, and the meaning of trigger IDs: trigger `n` is still bit `n % 8` of report byte
`3 + n / 8`, so saved trigger assignments keep working.

## Times on triggers

When a trigger fires because the device reported a change of its input, `Time` is set (before `Triggered` is raised) to the moment
that report arrived: the performance-counter stamp the native layer takes on its own thread as the read completes, as a `TimeSpan`
on the `Stopwatch` clock. Subtract two triggers' times to see how far apart their presses reached the computer; this works across
devices. Inputs that changed in the same report get exactly the same time: the device polled them together and their order cannot
be known. `Ordinal` and `IsTied` are not set (X-keys devices run no faceoff). A press is never reported more finely than the device
reports it; see the device facts below.

## Behavior changes

- **Nothing polls.** The library sleeps on Windows device notifications and on each open device's reports; it runs no timer.
- **Unplugging is survived.** A device that is unplugged goes to Disconnected, its held keys are released, and it is reopened, as the
  same entry of the device list, when it is plugged back in (its LED state is sent again).
- **Devices plugged in after start-up are found.**
- **The device list is in a fixed order**: the present devices by product ID, then by interface path (see "Choosing a device by
  index" in the README). 1.x used the order in which Windows listed them.
- **`GetDevices` enumerates on every call** (1.x enumerated once per trigger-key type, when the type was first used).
- **State that is not a press does not fire a trigger.** A key held down, or a Switch Interface socket with a plug in, when the
  device connects shows as down without firing; the release of every input when a device goes away does not fire a trigger set to
  fire on release either. Only a change the device reports fires a trigger.
- **The jog and shuttle of the XK-12 Jog & Shuttle never fire triggers.** Their bits share the key bytes; the library knows which
  bits are keys and ignores the rest. A trigger assigned to one of those bits (for example ID 7 or 15) never fires.
- **`SetLeds`, `SetUnitId` and `RequestUpdate` no longer throw** `InvalidOperationException` on a device that has never been
  connected. They log and do nothing.
- **Writes are synchronous** with a one-second timeout. 1.x queued them and could spin forever if the device stopped accepting
  them.
- **Duplicate reports are suppressed correctly.** Reports that differ only in the device's timestamp no longer count as changes.
- **A trigger ID that points beyond the report is ignored** instead of throwing.
- **Interfaces that cannot send input are skipped.** Some models expose an output-only interface under their second and third
  product IDs; 1.x could pick it.
- **No memory growth from enumeration.**

## Things worth knowing about specific devices

Measured on real devices in November 2026 (details in the TriggerGate repository's `docs/accuracy-expectations.md`; the library's
catalogue, `XkModel`, carries the same facts with their sources):

- **XK-12 Jog & Shuttle**: the twelve keys are trigger IDs 0, 8, 16, 24 (top row), 1, 9, 17, 25 and 2, 10, 18, 26. Turning the jog
  dial pulses IDs 7 and 15, and the shuttle ring uses other bits of the same bytes; the library does not treat those as keys. Windows
  polls the device every 8 ms, but two key changes that the device puts in consecutive reports were never seen closer than 29 ms by
  the device's clock (24 to 32 ms at the computer; seven observations, a small sample).
- **XK-12 Switch Interface**: socket `n` (1 to 6) is trigger IDs `2(n-1)` and `2(n-1)+1`. With a mono (tip-sleeve) plug the even ID
  turns on when the plug goes in and stays on; the odd ID is the button. Windows polls it every 1 ms; how closely its reports of two
  separate presses can follow each other has not been measured.

## What to check after migrating

1. The application starts with no X-keys device attached and shows its devices as not found, without errors.
2. With a device attached: it connects, the identification lights behave as before, and each assigned trigger fires from the
   expected key.
3. Unplug the device with the application running, then plug it back in: it reconnects and triggers fire again.
4. The installed or published application (not only the debug build) contains `xkhid.dll`.
5. The process is 64-bit.
